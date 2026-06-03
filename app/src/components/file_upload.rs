use dioxus::html::{FileData, HasFileData};
use dioxus::prelude::*;

use crate::db::{self, SessionInfo};

fn is_supported_upload_file(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.ends_with(".pdf") || name.ends_with(".zip")
}

async fn read_upload_files(files: Vec<FileData>) -> Vec<(String, Vec<u8>)> {
    let mut files_data = Vec::new();
    for file in files {
        let name = file.name();
        if !is_supported_upload_file(&name) {
            continue;
        }

        if let Ok(bytes) = file.read_bytes().await {
            files_data.push((name, bytes.to_vec()));
        }
    }
    files_data
}

async fn set_uploaded_files(
    files: Vec<FileData>,
    mut uploaded_files: Signal<Vec<(String, Vec<u8>)>>,
    mut previous_sessions: Signal<Vec<SessionInfo>>,
) {
    let files_data = read_upload_files(files).await;
    if files_data.is_empty() {
        previous_sessions.set(Vec::new());
        uploaded_files.set(Vec::new());
        return;
    }

    let hash = db::document_hash(&files_data);
    let sessions = db::list_sessions(&hash);
    previous_sessions.set(sessions);
    uploaded_files.set(files_data);
}

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
    let mut is_dragging = use_signal(|| false);
    let mut drag_depth = use_signal(|| 0usize);

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

        div {
            class: if !is_processing && *is_dragging.read() { "upload-dropzone upload-dropzone-dragging" } else { "upload-dropzone" },
            ondragenter: move |evt: Event<DragData>| {
                evt.prevent_default();
                if !is_processing {
                    let next_depth = *drag_depth.read() + 1;
                    drag_depth.set(next_depth);
                    is_dragging.set(true);
                }
            },
            ondragover: move |evt: Event<DragData>| {
                evt.prevent_default();
                if !is_processing {
                    is_dragging.set(true);
                }
            },
            ondragleave: move |evt: Event<DragData>| {
                evt.prevent_default();
                let next_depth = (*drag_depth.read()).saturating_sub(1);
                drag_depth.set(next_depth);
                if next_depth == 0 {
                    is_dragging.set(false);
                }
            },
            ondrop: move |evt: Event<DragData>| {
                evt.prevent_default();
                drag_depth.set(0);
                is_dragging.set(false);
                let files = if is_processing { Vec::new() } else { evt.files() };
                async move {
                    if !files.is_empty() {
                        sessions_open.set(false);
                        set_uploaded_files(files, uploaded_files, previous_sessions).await;
                    }
                }
            },

            h2 { "Upload Files" }
            p { class: "upload-hint",
                "Select or drop PDF files in different languages, an AEM content package ZIP, or a structured document JSON"
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
                accept: ".pdf,.zip,.json",
                disabled: is_processing,
                onchange: move |evt: Event<FormData>| {
                    let files = evt.files();
                    async move {
                        set_uploaded_files(files, uploaded_files, previous_sessions).await;
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use dioxus::html::{NativeFileData, bytes::Bytes};
    use std::{future::Future, path::PathBuf, pin::Pin};

    struct TestFileData {
        name: String,
        bytes: Vec<u8>,
    }

    impl NativeFileData for TestFileData {
        fn name(&self) -> String {
            self.name.clone()
        }

        fn size(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn last_modified(&self) -> u64 {
            0
        }

        fn path(&self) -> PathBuf {
            PathBuf::from(&self.name)
        }

        fn content_type(&self) -> Option<String> {
            Some("application/pdf".to_string())
        }

        fn read_bytes(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<Bytes, dioxus::CapturedError>> + 'static>> {
            let bytes = self.bytes.clone();
            Box::pin(async move { Ok(Bytes::from(bytes)) })
        }

        fn byte_stream(
            &self,
        ) -> Pin<
            Box<
                dyn futures_util::Stream<Item = Result<Bytes, dioxus::CapturedError>>
                    + 'static
                    + Send,
            >,
        > {
            let bytes = self.bytes.clone();
            Box::pin(futures_util::stream::once(
                async move { Ok(Bytes::from(bytes)) },
            ))
        }

        fn read_string(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<String, dioxus::CapturedError>> + 'static>>
        {
            let bytes = self.bytes.clone();
            Box::pin(async move { Ok(String::from_utf8(bytes)?) })
        }

        fn inner(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn read_upload_files_collects_supported_file_bytes() {
        let files = vec![
            FileData::new(TestFileData {
                name: "form.pdf".to_string(),
                bytes: vec![1, 2, 3],
            }),
            FileData::new(TestFileData {
                name: "notes.txt".to_string(),
                bytes: vec![4, 5, 6],
            }),
        ];

        let files_data = read_upload_files(files).await;

        assert_eq!(files_data, vec![("form.pdf".to_string(), vec![1, 2, 3])]);
    }
}
