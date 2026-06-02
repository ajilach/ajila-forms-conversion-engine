use dioxus::prelude::*;

use crate::db::{self, SessionInfo};

#[component]
pub fn FileUploadSection(
    is_processing: bool,
    profiles: Vec<String>,
    selected_profile: Signal<Option<String>>,
    on_process: EventHandler<Vec<(String, Vec<u8>)>>,
    on_continue: EventHandler<String>,
) -> Element {
    let mut uploaded_files = use_signal(Vec::<(String, Vec<u8>)>::new);
    // Previous editing sessions for the currently selected document set.
    let mut previous_sessions = use_signal(Vec::<SessionInfo>::new);
    // All sessions across documents, for the "load previous session" browser.
    let mut all_sessions = use_signal(db::list_all_sessions);
    let mut session_query = use_signal(String::new);
    // Whether the previous-session browser is shown inside the upload container.
    let mut sessions_open = use_signal(|| false);

    // Auto-select the first profile if none is selected yet
    if selected_profile.read().is_none()
        && let Some(first) = profiles.first()
    {
        selected_profile.set(Some(first.clone()));
    }

    // Filter the global session list by the search query (matches file names).
    let query = session_query.read().to_lowercase();
    let filtered_sessions: Vec<SessionInfo> = all_sessions
        .read()
        .iter()
        .filter(|s| query.is_empty() || s.label.to_lowercase().contains(&query))
        .cloned()
        .collect();

    let has_sessions = !all_sessions.read().is_empty();

    rsx! {
        // Profile selector (outside upload area)
        if !profiles.is_empty() {
            div { class: "profile-selector",
                label { r#for: "profile-select", "Profile" }
                select {
                    id: "profile-select",
                    disabled: is_processing,
                    onchange: move |evt: Event<FormData>| {
                        selected_profile.set(Some(evt.value()));
                    },
                    for name in profiles.iter() {
                        option {
                            value: "{name}",
                            selected: selected_profile.read().as_deref() == Some(name.as_str()),
                            "{name}"
                        }
                    }
                }
            }
        }

        div { class: "upload-dropzone",

            h2 { "Upload Files" }
            p { class: "upload-hint",
                "Select PDF files in different languages or an AEM content package ZIP"
            }

            div { class: "upload-actions",
                label {
                    class: "btn btn-primary btn-sm",
                    r#for: "file-input",
                    onclick: move |_| sessions_open.set(false),
                    "Choose Files"
                }
                if has_sessions {
                    button {
                        class: "btn btn-secondary btn-sm",
                        disabled: is_processing,
                        onclick: move |_| {
                            let open = *sessions_open.read();
                            sessions_open.set(!open);
                        },
                        "Load Previous Session"
                    }
                }
            }

            input {
                id: "file-input",
                class: "upload-input-hidden",
                r#type: "file",
                multiple: true,
                accept: ".pdf,.zip",
                disabled: is_processing,
                onchange: move |evt: Event<FormData>| {
                    async move {
                        let mut files_data = Vec::new();
                        for file in evt.files() {
                            if let Ok(bytes) = file.read_bytes().await {
                                files_data.push((file.name(), bytes.to_vec()));
                            }
                        }
                        // Look up any previous editing sessions for this document set.
                        let sessions = if files_data.is_empty() {
                            Vec::new()
                        } else {
                            let hash = db::document_hash(&files_data);
                            db::list_sessions(&hash)
                        };
                        previous_sessions.set(sessions);
                        uploaded_files.set(files_data);
                    }
                },
            }

            // Previous-session browser, toggled by "Load Previous Session".
            if *sessions_open.read() {
                div { class: "continue-editing",
                    p { class: "upload-hint", "Resume editing a document you worked on before." }
                    input {
                        class: "session-search",
                        r#type: "text",
                        placeholder: "Search by file name...",
                        value: "{session_query}",
                        oninput: move |evt| session_query.set(evt.value()),
                    }
                    if filtered_sessions.is_empty() {
                        p { class: "upload-hint", "No sessions match your search." }
                    } else {
                        ul { class: "session-list session-list-scroll",
                            for session in filtered_sessions.iter() {
                                li { class: "session-item",
                                    div { class: "session-meta",
                                        span { class: "session-label", "{session.label}" }
                                        div { class: "session-submeta",
                                            span { class: "session-time",
                                                "{db::format_timestamp(&session.created_at)}"
                                            }
                                            span { class: "session-count",
                                                "{session.edit_count} edit(s)"
                                            }
                                            if let Some(profile) = session.profile.as_ref() {
                                                span { class: "session-profile", "{profile}" }
                                            }
                                        }
                                    }
                                    div { class: "session-actions",
                                        button {
                                            class: "btn btn-secondary btn-sm",
                                            disabled: is_processing,
                                            onclick: {
                                                let session_id = session.session_id.clone();
                                                move |_| on_continue.call(session_id.clone())
                                            },
                                            "Load"
                                        }
                                        button {
                                            class: "btn btn-danger btn-sm",
                                            disabled: is_processing,
                                            title: "Delete this session",
                                            onclick: {
                                                let session_id = session.session_id.clone();
                                                move |_| {
                                                    db::delete_session(&session_id);
                                                    all_sessions.set(db::list_all_sessions());
                                                }
                                            },
                                            "Delete"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !uploaded_files.read().is_empty() {
                div { class: "file-list",
                    h3 { "Selected Files:" }
                    ul {
                        for (name , _bytes) in uploaded_files.read().iter() {
                            li { "{name}" }
                        }
                    }

                    button {
                        class: "btn btn-primary btn-sm",
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

            // Previously edited document: offer to continue an earlier session.
            if !previous_sessions.read().is_empty() {
                div { class: "continue-editing",
                    h3 { "Continue Editing" }
                    p { class: "upload-hint",
                        "This document was edited before. Resume a previous session:"
                    }
                    ul { class: "session-list",
                        for session in previous_sessions.read().iter() {
                            li { class: "session-item",
                                div { class: "session-meta",
                                    span { class: "session-label", "{session.label}" }
                                    div { class: "session-submeta",
                                        span { class: "session-time",
                                            "{db::format_timestamp(&session.created_at)}"
                                        }
                                        span { class: "session-count", "{session.edit_count} edit(s)" }
                                        if let Some(profile) = session.profile.as_ref() {
                                            span { class: "session-profile", "{profile}" }
                                        }
                                    }
                                }
                                button {
                                    class: "btn btn-secondary btn-sm",
                                    disabled: is_processing,
                                    onclick: {
                                        let session_id = session.session_id.clone();
                                        move |_| on_continue.call(session_id.clone())
                                    },
                                    "Continue"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
