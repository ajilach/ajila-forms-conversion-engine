use dioxus::prelude::*;
use std::path::PathBuf;
use base64::Engine;
use image::ImageEncoder;
use std::sync::mpsc::{channel, Receiver};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
enum ProcessingStep {
    Idle,
    Parsing,
    ExhaustiveSearching,
    Flattening,
    Structuring,
    Merging,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
struct ProcessingState {
    step: ProcessingStep,
    available_states: Vec<String>, // List of state names that can be rendered
    plain_images: HashMap<String, Vec<u8>>, // state_name -> PNG bytes
    labelled_images: HashMap<String, Vec<u8>>, // state_name -> PNG bytes
    merged_json: Option<String>,
    html_preview: Option<String>,
    error: Option<String>,
}



impl ProcessingState {
    fn new() -> Self {
        Self {
            step: ProcessingStep::Idle,
            available_states: Vec::new(),
            plain_images: HashMap::new(),
            labelled_images: HashMap::new(),
            merged_json: None,
            html_preview: None,
            error: None,
        }
    }
}

fn main() {
    dioxus::LaunchBuilder::desktop()
        .with_cfg(
            dioxus::desktop::Config::new()
                .with_window(
                    dioxus::desktop::WindowBuilder::new()
                        .with_title("Blueprint")
                )
        )
        .launch(App);
}

#[component]
fn App() -> Element {
    let mut uploaded_files = use_signal(Vec::<PathBuf>::new);
    let mut processing_state = use_signal(ProcessingState::new);
    let mut is_processing = use_signal(|| false);
    let mut update_receiver = use_signal(|| None::<Receiver<ProcessingState>>);
    let mut enlarged_image = use_signal(|| None::<(String, String)>); // (name, base64_data)

    // Continuously poll for updates from the background thread
    use_future(move || async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            
            let should_clear = if let Some(ref rx) = *update_receiver.read() {
                if let Ok(new_state) = rx.try_recv() {
                    let is_done = new_state.step == ProcessingStep::Complete || new_state.error.is_some();
                    processing_state.set(new_state);
                    if is_done {
                        is_processing.set(false);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            
            if should_clear {
                update_receiver.set(None);
            }
        }
    });

    rsx! {
        div { style: "padding: 20px; font-family: system-ui; max-width: 1400px; margin: 0 auto;",

            style {
                "
                .thumbnail-image:hover {{
                    transform: scale(1.05);
                }}
            "
            }

            //h1 { "Blueprint" }

            // File Upload Section
            FileUploadSection {
                uploaded_files,
                is_processing: is_processing.read().clone(),
                on_files_selected: move |files| {
                    uploaded_files.set(files);
                },
                on_process: move |_| {
                    is_processing.set(true);
                    processing_state.set(ProcessingState::new());
                    let files = uploaded_files.read().clone();

                    // Create channel for updates
                    let (state_tx, state_rx) = channel();

                    update_receiver.set(Some(state_rx));

                    // Spawn a background thread since Blueprint structs are not Send
                    std::thread::spawn(move || {
                        process_files_blocking(files, state_tx);
                    });
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
    uploaded_files: Signal<Vec<PathBuf>>,
    is_processing: bool,
    on_files_selected: EventHandler<Vec<PathBuf>>,
    on_process: EventHandler<()>,
) -> Element {
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
                    if let Some(file_engine) = evt.files() {
                        let files: Vec<PathBuf> = file_engine
                            .files()
                            .into_iter()
                            .filter_map(|name| { Some(PathBuf::from(name)) }) // In a real implementation, we'd get the actual file path
                            .collect();
                        on_files_selected.call(files);
                    }
                },
            }

            if !uploaded_files.read().is_empty() {
                div { style: "margin-top: 15px;",
                    h3 { "Selected Files:" }
                    ul {
                        for file in uploaded_files.read().iter() {
                            li { "{file.display()}" }
                        }
                    }

                    button {
                        style: "padding: 10px 20px; background-color: #007bff; color: white; border: none; border-radius: 4px; cursor: pointer; font-size: 16px;",
                        disabled: is_processing,
                        onclick: move |_| on_process.call(()),
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

                // Download JSON button
                if let Some(ref json_data) = state.merged_json {
                    button {
                        style: "padding: 12px 24px; background-color: #28a745; color: white; border: none; border-radius: 4px; cursor: pointer; font-size: 16px;",
                        onclick: {
                            let json_data = json_data.clone();
                            move |_| {
                                download_json(json_data.clone());
                            }
                        },
                        "Download Structure JSON"
                    }
                }

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
            }
        }
    }
}

// Blocking function to process files through the blueprint pipeline
// Runs in a background thread since Blueprint structs are not Send
// Sends updates via channel to avoid needing Send for Signals
fn process_files_blocking(
    files: Vec<PathBuf>,
    state_tx: std::sync::mpsc::Sender<ProcessingState>,
) {
    use blueprint::{Blueprint, HtmlConfig};
    
    // Process each PDF file
    let mut all_envelopes = Vec::new();
    let mut plain_images = HashMap::new();
    let mut labelled_images = HashMap::new();
    
    for (_file_idx, pdf_path) in files.iter().enumerate() {
        // Step 1: Parsing
        let mut current_state = ProcessingState {
            step: ProcessingStep::Parsing,
            available_states: Vec::new(),
            plain_images: plain_images.clone(),
            labelled_images: labelled_images.clone(),
            merged_json: None,
            html_preview: None,
            error: None,
        };
        let _ = state_tx.send(current_state.clone());
        
        let mut bp = match Blueprint::from_pdf(pdf_path) {
            Ok(bp) => bp,
            Err(e) => {
                current_state.error = Some(format!("Failed to parse {}: {}", pdf_path.display(), e));
                let _ = state_tx.send(current_state);
                return;
            }
        };
        
        let language = bp.language().to_string();
        
        // Step 2: Exhaustive Searching
        current_state.step = ProcessingStep::ExhaustiveSearching;
        let _ = state_tx.send(current_state.clone());
        
        let form_states = match bp.states() {
            Ok(states) => states,
            Err(e) => {
                current_state.error = Some(format!("Failed to explore states: {}", e));
                let _ = state_tx.send(current_state);
                return;
            }
        };
        
        let context = bp.context();
        
        // Step 3: Flattening - render plain images
        current_state.step = ProcessingStep::Flattening;
        let _ = state_tx.send(current_state.clone());
        
        // Render plain images for all states
        for state_idx in 0..form_states.len() {
            if let Some(form_state) = form_states.iter().nth(state_idx) {
                let state_name = format!("{}_{}", language, state_idx);
                if let Ok(img) = form_state.render_plain(1.5) {
                    let mut png_bytes = Vec::new();
                    if encode_rgba_to_png(&img, &mut png_bytes).is_ok() {
                        plain_images.insert(state_name, png_bytes);
                    }
                }
            }
        }
        
        current_state.plain_images = plain_images.clone();
        let _ = state_tx.send(current_state.clone());
        
        // Step 4: Structuring - render labelled images and get structured data
        current_state.step = ProcessingStep::Structuring;
        let _ = state_tx.send(current_state.clone());
        
        // Collect structured outputs for all states of this document
        let mut structured_outputs = Vec::new();
        for state_idx in 0..form_states.len() {
            if let Some(form_state) = form_states.iter().nth(state_idx) {
                // Render labelled image
                let state_name = format!("{}_{}", language, state_idx);
                if let Ok(img) = form_state.render_labelled(1.5) {
                    let mut png_bytes = Vec::new();
                    if encode_rgba_to_png(&img, &mut png_bytes).is_ok() {
                        labelled_images.insert(state_name, png_bytes);
                    }
                }
                
                // Get structured envelope
                let envelope = form_state.structured(context.clone());
                structured_outputs.push((form_state.selections.clone(), envelope.content));
            }
        }
        
        current_state.labelled_images = labelled_images.clone();
        let _ = state_tx.send(current_state.clone());
        
        // Merge exhaustive states for this document
        if !structured_outputs.is_empty() {
            use blueprint::{MergeInput, RecursiveMerger};
            
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
            
            // Store the merged envelope for this document (one per language)
            all_envelopes.push(merged_envelope);
        }
    }
    
    // Step 5: Merging - merge translations only if multiple documents
    let mut current_state = ProcessingState {
        step: ProcessingStep::Merging,
        available_states: Vec::new(),
        plain_images: plain_images.clone(),
        labelled_images: labelled_images.clone(),
        merged_json: None,
        html_preview: None,
        error: None,
    };
    let _ = state_tx.send(current_state.clone());
    
    let merged = if all_envelopes.is_empty() {
        current_state.error = Some("No envelopes to merge".to_string());
        let _ = state_tx.send(current_state);
        return;
    } else if files.len() > 1 && all_envelopes.len() > 1 {
        // Multiple documents - merge translations
        match blueprint::merge_translations(all_envelopes) {
            Ok(merged) => merged,
            Err(e) => {
                current_state.error = Some(format!("Failed to merge translations: {}", e));
                let _ = state_tx.send(current_state);
                return;
            }
        }
    } else {
        // Single document - just use the merged exhaustive states
        all_envelopes.into_iter().next().unwrap()
    };
    
    // Serialize to JSON
    let json = match serde_json::to_string_pretty(&merged) {
        Ok(json) => json,
        Err(e) => {
            current_state.error = Some(format!("Failed to serialize JSON: {}", e));
            let _ = state_tx.send(current_state);
            return;
        }
    };
    
    // Generate HTML preview
    let html = blueprint::to_html(&merged.content, &HtmlConfig::default());
    
    // Complete
    current_state.step = ProcessingStep::Complete;
    current_state.merged_json = Some(json);
    current_state.html_preview = Some(html);
    let _ = state_tx.send(current_state);
}

// Helper function to encode RGBA image to PNG bytes
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

fn download_json(json_data: String) {
    // In a desktop app, we need to use native file dialog
    // This is a simplified version - you may need to use rfd crate for file dialogs
    use std::fs;
    
    match dirs::home_dir() {
        Some(home) => {
            let download_path = home.join("Downloads").join("merged_structure.json");
            match fs::write(&download_path, json_data) {
                Ok(_) => {
                    println!("✓ File saved to: {}", download_path.display());
                    // TODO: Show success notification to user in UI
                }
                Err(e) => {
                    eprintln!("✗ Failed to save file to {}: {}", download_path.display(), e);
                    // TODO: Show error notification to user in UI
                }
            }
        }
        None => {
            eprintln!("✗ Failed to determine home directory for saving file");
            // TODO: Show error notification to user in UI
        }
    }
}

fn show_html_preview(html: String) {
    // For desktop apps, you could:
    // 1. Open in system browser
    // 2. Show in an embedded webview
    // 3. Display in a modal dialog
    
    use std::fs;
    
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("✗ Failed to determine home directory for saving preview");
            return;
        }
    };
    
    let preview_path = home.join("Downloads").join("form_preview.html");
    if let Err(e) = fs::write(&preview_path, html) {
        eprintln!("✗ Failed to save preview to {}: {}", preview_path.display(), e);
        return;
    }
    
    println!("✓ Preview saved to: {}", preview_path.display());
    
    // Open in system browser
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = std::process::Command::new("open")
            .arg(&preview_path)
            .spawn()
        {
            eprintln!("✗ Failed to open preview in browser: {}", e);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = std::process::Command::new("xdg-open")
            .arg(&preview_path)
            .spawn()
        {
            eprintln!("✗ Failed to open preview in browser: {}", e);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Err(e) = std::process::Command::new("cmd")
            .args(&["/C", "start", "", &preview_path.to_string_lossy()])
            .spawn()
        {
            eprintln!("✗ Failed to open preview in browser: {}", e);
        }
    }
}
