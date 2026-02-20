use dioxus::prelude::*;
use base64::Engine;
use std::collections::HashMap;

#[cfg(not(target_arch = "wasm32"))]
use image::ImageEncoder;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ProcessingStep {
    #[default]
    Idle,
    Parsing,
    ExhaustiveSearching,
    Flattening,
    Structuring,
    Merging,
    Complete,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProcessingState {
    step: ProcessingStep,
    available_states: Vec<String>, // List of state names that can be rendered
    plain_images: HashMap<String, Vec<u8>>, // state_name -> PNG bytes
    labelled_images: HashMap<String, Vec<u8>>, // state_name -> PNG bytes
    merged_json: Option<String>,
    html_preview: Option<String>,
    aem_package: Option<Vec<u8>>,
    error: Option<String>,
}



impl ProcessingState {
    fn new() -> Self {
        Self::default()
    }
}

// ── Server-side session store for incremental progress ───────────────

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
static SESSIONS: std::sync::LazyLock<std::sync::Mutex<HashMap<String, ProcessingState>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn next_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("s{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

// ── Core blueprint processing pipeline (native only) ─────────────────

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn run_blueprint_pipeline(
    files: &[(String, Vec<u8>)],
    on_progress: impl Fn(&ProcessingState),
) -> ProcessingState {
    use blueprint::{Blueprint, HtmlConfig, AemConfig, MergeInput, RecursiveMerger};

    let mut state = ProcessingState::new();
    let mut all_envelopes = Vec::new();

    for (filename, bytes) in files {
        // Parsing
        state.step = ProcessingStep::Parsing;
        on_progress(&state);

        let mut bp = match Blueprint::from_pdf_bytes(bytes) {
            Ok(bp) => bp,
            Err(e) => {
                state.error = Some(format!("Failed to parse {filename}: {e}"));
                on_progress(&state);
                return state;
            }
        };

        let language = bp.language().to_string();

        // Exhaustive Searching
        state.step = ProcessingStep::ExhaustiveSearching;
        on_progress(&state);

        let form_states = match bp.states() {
            Ok(s) => s,
            Err(e) => {
                state.error = Some(format!("Failed to explore states: {e}"));
                on_progress(&state);
                return state;
            }
        };

        let context = bp.context();

        // Flattening – render plain images
        state.step = ProcessingStep::Flattening;
        on_progress(&state);

        for (state_idx, form_state) in form_states.iter().enumerate() {
            let state_name = format!("{language}_{state_idx}");
            if let Ok(img) = form_state.render_plain(1.5) {
                let mut png_bytes = Vec::new();
                if encode_rgba_to_png(&img, &mut png_bytes).is_ok() {
                    state.plain_images.insert(state_name, png_bytes);
                }
            }
        }
        on_progress(&state);

        // Structuring – render labelled images & extract structured data
        state.step = ProcessingStep::Structuring;
        on_progress(&state);

        let mut structured_outputs = Vec::new();
        for (state_idx, form_state) in form_states.iter().enumerate() {
            let state_name = format!("{language}_{state_idx}");
            if let Ok(img) = form_state.render_labelled(1.5) {
                let mut png_bytes = Vec::new();
                if encode_rgba_to_png(&img, &mut png_bytes).is_ok() {
                    state.labelled_images.insert(state_name, png_bytes);
                }
            }
            let envelope = form_state.structured(context.clone());
            structured_outputs.push((form_state.selections.clone(), envelope.content));
        }
        on_progress(&state);

        // Merge exhaustive states for this document
        if !structured_outputs.is_empty() {
            let merge_inputs: Vec<MergeInput> = structured_outputs
                .into_iter()
                .map(|(selections, nodes)| MergeInput::new(selections, nodes))
                .collect();

            let merger = RecursiveMerger::new(merge_inputs);
            let merged_states = merger.merge();

            let merged_envelope = blueprint::DocumentEnvelope {
                context: context.clone(),
                content: merged_states,
            };
            all_envelopes.push(merged_envelope);
        }

        let _ = std::fs::remove_file(&temp_path);
    }

    // Merging
    state.step = ProcessingStep::Merging;
    on_progress(&state);

    let merged = if all_envelopes.is_empty() {
        state.error = Some("No envelopes to merge".into());
        on_progress(&state);
        return state;
    } else if files.len() > 1 && all_envelopes.len() > 1 {
        match blueprint::merge_translations(all_envelopes) {
            Ok(m) => m,
            Err(e) => {
                state.error = Some(format!("Failed to merge translations: {e}"));
                on_progress(&state);
                return state;
            }
        }
    } else {
        all_envelopes.into_iter().next().unwrap()
    };

    let json = match serde_json::to_string_pretty(&merged) {
        Ok(j) => j,
        Err(e) => {
            state.error = Some(format!("Failed to serialize JSON: {e}"));
            on_progress(&state);
            return state;
        }
    };
    let html = blueprint::to_html(&merged.content, &HtmlConfig::default());
    let aem_zip = blueprint::to_aem_package(&merged.content, &AemConfig::default());

    state.step = ProcessingStep::Complete;
    state.merged_json = Some(json);
    state.html_preview = Some(html);
    state.aem_package = Some(aem_zip);
    on_progress(&state);

    state
}

// ── Server functions (fullstack) ─────────────────────────────────────

#[server]
async fn start_processing(
    files: Vec<(String, Vec<u8>)>,
) -> Result<String, ServerFnError> {
    let session_id = next_session_id();
    SESSIONS.lock().unwrap().insert(
        session_id.clone(),
        ProcessingState {
            step: ProcessingStep::Parsing,
            ..ProcessingState::new()
        },
    );

    let sid = session_id.clone();
    std::thread::spawn(move || {
        let final_state = run_blueprint_pipeline(&files, |state| {
            SESSIONS.lock().unwrap().insert(sid.clone(), state.clone());
        });
        SESSIONS.lock().unwrap().insert(sid, final_state);
    });

    Ok(session_id)
}

#[server]
async fn poll_progress(session_id: String) -> Result<ProcessingState, ServerFnError> {
    let state = SESSIONS
        .lock()
        .unwrap()
        .get(&session_id)
        .cloned()
        .ok_or_else(|| ServerFnError::new("Session not found"))?;

    // Clean up completed sessions
    if state.step == ProcessingStep::Complete || state.error.is_some() {
        SESSIONS.lock().unwrap().remove(&session_id);
    }

    Ok(state)
}

// ── Platform-agnostic async sleep ────────────────────────────────────

async fn async_sleep_ms(ms: u32) {
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::TimeoutFuture::new(ms).await;
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

fn main() {
    #[cfg(feature = "desktop")]
    {
        dioxus::LaunchBuilder::desktop()
            .with_cfg(
                dioxus::desktop::Config::new().with_window(
                    dioxus::desktop::WindowBuilder::new().with_title("Blueprint"),
                ),
            )
            .launch(App);
    }

    #[cfg(not(feature = "desktop"))]
    {
        dioxus::launch(App);
    }
}

#[component]
fn App() -> Element {
    let mut processing_state = use_signal(ProcessingState::new);
    let mut is_processing = use_signal(|| false);
    let mut enlarged_image = use_signal(|| None::<(String, String)>);

    let mut on_process = move |file_data: Vec<(String, Vec<u8>)>| {
        is_processing.set(true);
        processing_state.set(ProcessingState {
            step: ProcessingStep::Parsing,
            ..ProcessingState::new()
        });

        spawn(async move {
            #[cfg(feature = "desktop")]
            {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProcessingState>();
                tokio::task::spawn_blocking(move || {
                    run_blueprint_pipeline(&file_data, |state| {
                        let _ = tx.send(state.clone());
                    })
                });
                while let Some(state) = rx.recv().await {
                    let done =
                        state.step == ProcessingStep::Complete || state.error.is_some();
                    processing_state.set(state);
                    if done {
                        break;
                    }
                }
            }

            #[cfg(not(feature = "desktop"))]
            {
                match start_processing(file_data).await {
                    Ok(session_id) => {
                        loop {
                            async_sleep_ms(200).await;
                            match poll_progress(session_id.clone()).await {
                                Ok(state) => {
                                    let done = state.step == ProcessingStep::Complete
                                        || state.error.is_some();
                                    processing_state.set(state);
                                    if done {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    processing_state.set(ProcessingState {
                                        error: Some(format!("{e}")),
                                        ..ProcessingState::new()
                                    });
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        processing_state.set(ProcessingState {
                            error: Some(format!("{e}")),
                            ..ProcessingState::new()
                        });
                    }
                }
            }
            is_processing.set(false);
        });
    };

    rsx! {
        div { style: "padding: 20px; font-family: system-ui; max-width: 1400px; margin: 0 auto;",

            style {
                "
                .thumbnail-image:hover {{
                    transform: scale(1.05);
                }}
            "
            }

            // File Upload Section
            FileUploadSection {
                is_processing: *is_processing.read(),
                on_process: move |files: Vec<(String, Vec<u8>)>| {
                    on_process(files);
                },
            }

            // Progress Display
            if *is_processing.read() || processing_state.read().step != ProcessingStep::Idle {
                ProgressDisplay {
                    state: processing_state.read().clone(),
                    on_image_click: move |(name, data)| enlarged_image.set(Some((name, data))),
                }
            }

            // Results Section
            if processing_state.read().step == ProcessingStep::Complete {
                ResultsSection { state: processing_state.read().clone() }
            }

            // Image Modal Overlay
            if let Some((name, data)) = enlarged_image.read().as_ref() {
                ImageModal {
                    name: name.clone(),
                    data: data.clone(),
                    on_close: move |_| enlarged_image.set(None),
                }
            }
        }
    }
}

#[component]
fn FileUploadSection(
    is_processing: bool,
    on_process: EventHandler<Vec<(String, Vec<u8>)>>,
) -> Element {
    let mut uploaded_files = use_signal(Vec::<(String, Vec<u8>)>::new);

    rsx! {
        div { style: "border: 2px dashed #ccc; padding: 30px; margin-bottom: 20px; border-radius: 8px;",

            h2 { "Upload PDF Files" }
            p { style: "color: #666;", "Select multiple PDF files in different languages" }

            input {
                r#type: "file",
                multiple: true,
                accept: ".pdf",
                disabled: is_processing,
                onchange: move |evt| {
                    async move {
                        if let Some(file_engine) = evt.files() {
                            let mut files_data = Vec::new();
                            for filename in file_engine.files() {
                                if let Some(bytes) = file_engine.read_file(&filename).await {
                                    files_data.push((filename, bytes));
                                }
                            }
                            uploaded_files.set(files_data);
                        }
                    }
                },
            }

            if !uploaded_files.read().is_empty() {
                div { style: "margin-top: 15px;",
                    h3 { "Selected Files:" }
                    ul {
                        for (name , _bytes) in uploaded_files.read().iter() {
                            li { "{name}" }
                        }
                    }

                    button {
                        style: "padding: 10px 20px; background-color: #007bff; color: white; border: none; border-radius: 4px; cursor: pointer; font-size: 16px;",
                        disabled: is_processing,
                        onclick: {
                            let files = uploaded_files.read().clone();
                            move |_| {
                                on_process.call(files.clone());
                            }
                        },
                        if is_processing {
                            "Processing..."
                        } else {
                            "Start Processing"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ProgressDisplay(
    state: ProcessingState,
    on_image_click: EventHandler<(String, String)>,
) -> Element {
    rsx! {
        div { style: "margin-top: 30px; padding: 20px; background-color: #f5f5f5; border-radius: 8px;",

            h2 { "Progress" }

            div { style: "margin-top: 20px;",

                StepIndicator {
                    name: "1. Parsing",
                    is_current: state.step == ProcessingStep::Parsing,
                    is_complete: matches!(
                        state.step,
                        ProcessingStep::ExhaustiveSearching
                        | ProcessingStep::Flattening
                        | ProcessingStep::Structuring
                        | ProcessingStep::Merging
                        | ProcessingStep::Complete
                    ),
                }

                StepIndicator {
                    name: "2. Exhaustive Searching",
                    is_current: state.step == ProcessingStep::ExhaustiveSearching,
                    is_complete: matches!(
                        state.step,
                        ProcessingStep::Flattening
                        | ProcessingStep::Structuring
                        | ProcessingStep::Merging
                        | ProcessingStep::Complete
                    ),
                }

                StepIndicator {
                    name: "3. Flattening",
                    is_current: state.step == ProcessingStep::Flattening,
                    is_complete: matches!(
                        state.step,
                        ProcessingStep::Structuring | ProcessingStep::Merging | ProcessingStep::Complete
                    ),
                }

                // Show plain images after flattening
                if !state.plain_images.is_empty() {
                    ImageGrid {
                        title: "Plain State Images",
                        images: state.plain_images.clone(),
                        on_image_click,
                    }
                }

                StepIndicator {
                    name: "4. Structuring",
                    is_current: state.step == ProcessingStep::Structuring,
                    is_complete: matches!(state.step, ProcessingStep::Merging | ProcessingStep::Complete),
                }

                // Show labelled images after structuring
                if !state.labelled_images.is_empty() {
                    ImageGrid {
                        title: "Labelled State Images",
                        images: state.labelled_images.clone(),
                        on_image_click,
                    }
                }

                StepIndicator {
                    name: "5. Merging",
                    is_current: state.step == ProcessingStep::Merging,
                    is_complete: state.step == ProcessingStep::Complete,
                }
            }

            if let Some(error) = &state.error {
                div { style: "margin-top: 20px; padding: 15px; background-color: #fee; border: 1px solid #fcc; border-radius: 4px; color: #c00;",
                    strong { "Error: " }
                    "{error}"
                }
            }
        }
    }
}

#[component]
fn StepIndicator(name: String, is_current: bool, is_complete: bool) -> Element {
    let style = if is_complete {
        "padding: 15px; margin: 10px 0; background-color: #d4edda; border-left: 4px solid #28a745; border-radius: 4px;"
    } else if is_current {
        "padding: 15px; margin: 10px 0; background-color: #d1ecf1; border-left: 4px solid #17a2b8; border-radius: 4px; font-weight: bold;"
    } else {
        "padding: 15px; margin: 10px 0; background-color: #e9ecef; border-left: 4px solid #6c757d; border-radius: 4px; color: #666;"
    };
    
    rsx! {
        div { style: "{style}",
            "{name}"
            if is_complete {
                span { style: "float: right;", "✓" }
            }
            if is_current {
                span { style: "float: right;", "●" }
            }
        }
    }
}

#[component]
fn ImageGrid(
    title: String,
    images: HashMap<String, Vec<u8>>,
    on_image_click: EventHandler<(String, String)>,
) -> Element {
    rsx! {
        div { style: "margin: 20px 0;",
            h3 { "{title}" }
            div { style: "display: flex; overflow-x: auto; gap: 15px; margin-top: 15px; padding-bottom: 10px;",
                for (state_name , image_bytes) in images.iter() {
                    div { style: "border: 1px solid #ddd; border-radius: 4px; padding: 10px; background-color: white; width: 150px; max-width: 150px; flex-shrink: 0;",
                        div { style: "font-size: 11px; font-weight: bold; margin-bottom: 5px; color: #333; text-overflow: ellipsis; overflow: hidden; white-space: nowrap;",
                            "{state_name}"
                        }
                        img {
                            src: "data:image/png;base64,{base64::prelude::BASE64_STANDARD.encode(image_bytes)}",
                            style: "width: 100%; height: auto; border-radius: 4px; cursor: pointer; transition: transform 0.2s; max-height: 180px; object-fit: contain;",
                            class: "thumbnail-image",
                            alt: "{state_name}",
                            onclick: {
                                let name = state_name.clone();
                                let data = base64::prelude::BASE64_STANDARD.encode(image_bytes);
                                move |_| on_image_click.call((name.clone(), data.clone()))
                            },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ImageModal(
    name: String,
    data: String,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        div {
            style: "position: fixed; top: 0; left: 0; right: 0; bottom: 0; background-color: rgba(0, 0, 0, 0.85); display: flex; align-items: center; justify-content: center; z-index: 1000; padding: 20px;",
            onclick: move |_| on_close.call(()),

            div {
                style: "position: relative; max-width: 95vw; max-height: 95vh; background-color: white; border-radius: 8px; padding: 20px; box-shadow: 0 4px 20px rgba(0, 0, 0, 0.5);",
                onclick: move |evt| evt.stop_propagation(),

                button {
                    style: "position: absolute; top: 10px; right: 10px; background-color: #dc3545; color: white; border: none; border-radius: 50%; width: 32px; height: 32px; font-size: 20px; cursor: pointer; display: flex; align-items: center; justify-content: center; z-index: 1001;",
                    onclick: move |_| on_close.call(()),
                    "×"
                }

                div { style: "font-size: 16px; font-weight: bold; margin-bottom: 15px; color: #333;",
                    "{name}"
                }

                img {
                    src: "data:image/png;base64,{data}",
                    style: "max-width: 100%; max-height: calc(95vh - 100px); width: auto; height: auto; display: block; border-radius: 4px;",
                    alt: "{name}",
                }
            }
        }
    }
}

#[component]
fn ResultsSection(state: ProcessingState) -> Element {
    rsx! {
        div { style: "margin-top: 30px; padding: 20px; background-color: #e7f4e7; border-radius: 8px;",

            h2 { "✓ Processing Complete!" }

            div { style: "margin-top: 20px; display: flex; gap: 15px; flex-wrap: wrap;",

                // HTML Preview button
                if let Some(ref html_preview) = state.html_preview {
                    button {
                        style: "padding: 12px 24px; background-color: #007bff; color: white; border: none; border-radius: 4px; cursor: pointer; font-size: 16px;",
                        onclick: {
                            let html_preview = html_preview.clone();
                            move |_| {
                                show_html_preview(html_preview.clone());
                            }
                        },
                        "Preview as HTML Form"
                    }
                }

                // Download JSON button
                if let Some(ref json_data) = state.merged_json {
                    button {
                        style: "padding: 12px 24px; background-color: #28a745; color: white; border: none; border-radius: 4px; cursor: pointer; font-size: 16px;",
                        onclick: {
                            let json_data = json_data.clone();
                            move |_| {
                                download_file(
                                    json_data.as_bytes(),
                                    "merged_structure.json",
                                    "application/json",
                                );
                            }
                        },
                        "Download Structure JSON"
                    }
                }

                // AEM Package Download button
                if let Some(ref aem_data) = state.aem_package {
                    button {
                        style: "padding: 12px 24px; background-color: #28a745; color: white; border: none; border-radius: 4px; cursor: pointer; font-size: 16px;",
                        onclick: {
                            let aem_data = aem_data.clone();
                            move |_| {
                                download_file(&aem_data, "aem_forms_package.zip", "application/zip");
                            }
                        },
                        "Download AEM Package"
                    }
                }
            }
        }
    }
}

// Helper function to encode RGBA image to PNG bytes
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn encode_rgba_to_png(img: &blueprint::RgbaImage, output: &mut Vec<u8>) -> Result<(), String> {
    use image::codecs::png::PngEncoder;
    use image::ExtendedColorType;
    
    let (width, height) = img.dimensions();
    let encoder = PngEncoder::new(output);
    
    encoder
        .write_image(
            img.as_raw(),
            width,
            height,
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("PNG encoding error: {}", e))
}

// ── File download / preview helpers (platform-aware) ──────────────────

#[cfg(target_arch = "wasm32")]
fn download_file(data: &[u8], filename: &str, mime_type: &str) {
    use wasm_bindgen::JsCast;
    use js_sys::{Array, Uint8Array};
    use web_sys::{Blob, BlobPropertyBag, Url, HtmlAnchorElement};

    let uint8_array = Uint8Array::from(data);
    let array = Array::new();
    array.push(&uint8_array.buffer());

    let mut options = BlobPropertyBag::new();
    options.set_type(mime_type);

    let blob = Blob::new_with_buffer_source_sequence_and_options(&array, &options).unwrap();
    let url = Url::create_object_url_with_blob(&blob).unwrap();

    let window = web_sys::window().unwrap();
    let document = window.document().unwrap();
    let a: HtmlAnchorElement = document
        .create_element("a")
        .unwrap()
        .dyn_into()
        .unwrap();
    a.set_href(&url);
    a.set_download(filename);
    a.click();

    let _ = Url::revoke_object_url(&url);
}

#[cfg(not(target_arch = "wasm32"))]
fn download_file(data: &[u8], filename: &str, _mime_type: &str) {
    match dirs::home_dir() {
        Some(home) => {
            let download_path = home.join("Downloads").join(filename);
            match std::fs::write(&download_path, data) {
                Ok(_) => {
                    println!("✓ File saved to: {}", download_path.display());
                    reveal_in_file_explorer(&download_path);
                }
                Err(e) => {
                    eprintln!("✗ Failed to save file to {}: {}", download_path.display(), e);
                }
            }
        }
        None => {
            eprintln!("✗ Failed to determine home directory for saving file");
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn show_html_preview(html: String) {
    download_file(html.as_bytes(), "form_preview.html", "text/html");
}

#[cfg(not(target_arch = "wasm32"))]
fn show_html_preview(html: String) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("✗ Failed to determine home directory for saving preview");
            return;
        }
    };
    
    let preview_path = home.join("Downloads").join("form_preview.html");
    if let Err(e) = std::fs::write(&preview_path, html) {
        eprintln!("✗ Failed to save preview to {}: {}", preview_path.display(), e);
        return;
    }
    
    println!("✓ Preview saved to: {}", preview_path.display());
    
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg(&preview_path)
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(&preview_path)
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(&["/C", "start", "", &preview_path.to_string_lossy()])
            .spawn();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn reveal_in_file_explorer(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(parent) = path.parent() {
            let _ = std::process::Command::new("xdg-open")
                .arg(parent)
                .spawn();
        }
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .args(&["/select,", &path.to_string_lossy()])
            .spawn();
    }
}
