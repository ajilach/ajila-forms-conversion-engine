use dioxus::prelude::*;

#[component]
pub fn FileUploadSection(
    is_processing: bool,
    on_process: EventHandler<Vec<(String, Vec<u8>)>>,
) -> Element {
    let mut uploaded_files = use_signal(Vec::<(String, Vec<u8>)>::new);

    rsx! {
        div { class: "upload-dropzone",

            h2 { "Upload PDF Files" }
            p { class: "upload-hint", "Select multiple PDF files in different languages" }

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
