mod agent_runner;
mod components;
mod files;
mod mcp_install;
mod models;
mod upload;

// The headless engine layer (edit-history store, reference store, AEM client)
// lives in the `agent` crate; the LLM transport and the operator settings live
// in `runner`, shared with the CLI. Re-export both under the historical
// `crate::*` paths so the rest of the app is unchanged.
pub use agent::{aem_client, db, references, session};
pub use runner::{llm, settings};

use dioxus::prelude::*;

use components::{AgentFlow, ReferencesPage, SettingsPage};
use models::{ProcessingState, ProcessingStep};
use settings::AppSettings;

fn main() {
    let saved = AppSettings::load();
    saved.apply_runtime_config();
    let mut config = dioxus::desktop::Config::new().with_window(
        dioxus::desktop::WindowBuilder::new()
            .with_always_on_top(saved.always_on_top)
            // The agent box fills the window, so the size only has to fit the
            // content itself — not a centred column plus a backdrop.
            .with_inner_size(dioxus::desktop::LogicalSize::new(880.0, 720.0))
            .with_title("Ajila Forms Conversion Engine"),
    );

    // Window/taskbar icon (no-op on macOS, used on Windows/Linux).
    if let Some(icon) = load_window_icon() {
        config = config.with_icon(icon);
    }

    dioxus::LaunchBuilder::new().with_cfg(config).launch(App);
}

/// Decode the bundled PNG into a `tao` window icon.
fn load_window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    let rgba = image::load_from_memory(include_bytes!("../icons/icon.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(rgba.into_raw(), width, height).ok()
}

#[component]
fn App() -> Element {
    let mut processing_state = use_signal(ProcessingState::default);
    let mut is_processing = use_signal(|| false);
    // The profile list is baked into the binary, so read it once and start on
    // the first entry rather than re-deriving the default during every render.
    let profiles = use_hook(blueprint::list_profiles);
    let selected_profile = use_signal(|| profiles.first().cloned());
    // What the next conversion run produces. Chosen before the run starts,
    // because it decides what the agent authors, not just which file is offered
    // at the end.
    let selected_target = use_signal(blueprint::OutputTarget::default);
    let mut settings_open = use_signal(|| false);
    // Whether the full-page reference-forms manager is open.
    let mut references_open = use_signal(|| false);
    let mut app_settings = use_signal(AppSettings::load);
    // Edit-history session id for the currently loaded document.
    let mut current_session = use_signal(|| None::<String>);
    // Source PDF bytes of the currently loaded document, retained so a feedback
    // re-run can resume the conversion from the same sources.
    let mut source_pdfs = use_signal(Vec::<(String, Vec<u8>)>::new);
    // A hook, so it has to be called here rather than inside the settings
    // handler that uses it.
    let window = dioxus::desktop::use_window();
    // Shared with the running conversion so the Abort button can stop it. One
    // per app rather than per run: the button and the run must hold the same
    // cell, and a run clears it before it starts.
    // Held in a signal so the run-starting closures stay `Copy`; the flag itself
    // never changes identity, so nothing subscribes to it.
    let abort = use_signal(models::AbortFlag::default);

    // Both entry points below start a run the same way: capture the user's
    // choices, flip the flow into its running phase, and let the agent drive.
    let run_config = move || agent_runner::RunConfig {
        profile: selected_profile.read().clone(),
        target: *selected_target.read(),
        settings: app_settings.read().clone(),
        abort: abort.peek().clone(),
    };
    let mut begin_run = move || {
        // A previous run may have left it set.
        abort.peek().reset();
        is_processing.set(true);
        processing_state.set(ProcessingState {
            step: ProcessingStep::Running,
            ..ProcessingState::default()
        });
    };

    // ── AI processing ───────────────────────────────────────────────────────
    // Hand the whole conversion to the autonomous agent: it drives the engine
    // via tools (extract → structure → convert → AEM → package → upload/verify),
    // versioning each step, and finalizes the result. The full file set is passed
    // so an attached content-package ZIP can be pre-loaded as the agent's
    // editable working tree.
    let mut on_ai_process = move |file_data: Vec<(String, Vec<u8>)>| {
        let pdfs: Vec<(String, Vec<u8>)> = file_data
            .iter()
            .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".pdf"))
            .cloned()
            .collect();
        // An AEM content-package ZIP may be attached as an editable template for
        // the agent's working tree. Proceed with PDFs, a template, or both.
        let has_template = file_data
            .iter()
            .any(|(_, bytes)| blueprint::detect_aem_zip(bytes));
        if pdfs.is_empty() && !has_template {
            return;
        }

        current_session.set(None);
        // Retain the PDF sources so a feedback re-run can reuse them.
        source_pdfs.set(pdfs);

        let config = run_config();
        begin_run();

        spawn(async move {
            let session_label = file_data
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>()
                .join(", ");

            agent_runner::run_agent(
                file_data,
                config,
                session_label,
                processing_state,
                current_session,
            )
            .await;
            is_processing.set(false);
        });
    };

    // ── Agent feedback re-run ─────────────────────────────────────────────────
    // From the agent "done" screen the user can submit feedback; this resumes
    // the agent in the same session to refine the result and returns the flow
    // to the in-progress (running) phase.
    let mut on_ai_feedback = move |feedback: String| {
        let Some(session) = current_session.read().clone() else {
            return;
        };
        let pdfs = source_pdfs.read().clone();
        if pdfs.is_empty() {
            return;
        }

        let config = run_config();
        begin_run();

        spawn(async move {
            agent_runner::run_agent_feedback(
                feedback,
                pdfs,
                config,
                session,
                processing_state,
                current_session,
            )
            .await;
            is_processing.set(false);
        });
    };

    // ── Render ────────────────────────────────────────────────────────────────
    rsx! {
        document::Stylesheet { href: asset!("/assets/styles.css") }

        // App Header
        header { class: "app-header",
            img {
                class: "app-header-logo",
                src: asset!("/assets/company-logo.webp"),
                alt: "Ajila Company Logo",
            }
            div { class: "app-header-right",
                h1 { class: "app-header-title", "Forms Conversion Engine" }
                span { class: "app-header-version", "v{env!(\"CARGO_PKG_VERSION\")}" }
            }
            button {
                class: "settings-btn",
                title: "Settings",
                onclick: move |_| settings_open.set(true),
                "⚙"
            }
        }

        // Settings, the references manager, or the agent flow — full-page views
        // under the persistent header.
        if *settings_open.read() {
            SettingsPage {
                on_close: move |_| settings_open.set(false),
                settings: app_settings,
                on_settings_changed: move |new_settings: AppSettings| {
                    new_settings.save();
                    new_settings.apply_runtime_config();
                    window.set_always_on_top(new_settings.always_on_top);
                    app_settings.set(new_settings);
                },
                on_open_references: move |_| {
                    settings_open.set(false);
                    references_open.set(true);
                },
            }
        } else if *references_open.read() {
            // Reference-forms manager (full page view)
            ReferencesPage {
                profile: selected_profile.read().clone(),
                settings: app_settings,
                on_close: move |_| references_open.set(false),
            }
        } else {
            // The agent flow: upload → live timeline → done.
            AgentFlow {
                processing_state,
                is_processing,
                profiles,
                selected_profile,
                selected_target,
                abort: abort.peek().clone(),
                ai_available: !app_settings.read().active_api_key().is_empty(),
                aem_connection: app_settings.read().aem_connection(),
                on_ai_process: move |files: Vec<(String, Vec<u8>)>| {
                    on_ai_process(files);
                },
                on_feedback: move |text: String| {
                    on_ai_feedback(text);
                },
                on_reset: move |_| {
                    is_processing.set(false);
                    current_session.set(None);
                    source_pdfs.set(Vec::new());
                    processing_state.set(ProcessingState::default());
                },
            }
        }
    }
}
