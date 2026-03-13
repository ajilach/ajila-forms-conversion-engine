use dioxus::prelude::*;

#[component]
pub fn FileUploadSection(
    is_processing: bool,
    profiles: Vec<String>,
    selected_profile: Signal<Option<String>>,
    on_process: EventHandler<Vec<(String, Vec<u8>)>>,
) -> Element {
    let mut uploaded_files = use_signal(Vec::<(String, Vec<u8>)>::new);

    // Auto-select the first profile if none is selected yet
    if selected_profile.read().is_none()
        && let Some(first) = profiles.first()
    {
        selected_profile.set(Some(first.clone()));
    }

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

            h2 { "Upload PDF Files" }
            p { class: "upload-hint", "Select multiple PDF files in different languages" }

            label { class: "btn btn-primary btn-sm", r#for: "file-input", "Choose PDF Files" }

            input {
                id: "file-input",
                class: "upload-input-hidden",
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
        }
    }
}
