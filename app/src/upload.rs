//! Upload adaptation: turn the browser/desktop `FileData` handles a drop or
//! file-picker yields into plain `(name, bytes)` pairs, filtered to the formats
//! the conversion engine accepts.

use dioxus::html::FileData;

/// The upload formats the conversion agent can actually consume: source PDFs,
/// and an AEM content-package ZIP to pre-load as its working tree.
fn is_supported_upload_file(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.ends_with(".pdf") || name.ends_with(".zip")
}

pub(crate) async fn read_upload_files(files: Vec<FileData>) -> Vec<(String, Vec<u8>)> {
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

#[cfg(test)]
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
            // Nothing consumes a structured-JSON upload any more, so it must not
            // be accepted and then silently ignored.
            FileData::new(TestFileData {
                name: "structure.json".to_string(),
                bytes: vec![7, 8, 9],
            }),
        ];

        let files_data = read_upload_files(files).await;

        assert_eq!(files_data, vec![("form.pdf".to_string(), vec![1, 2, 3])]);
    }
}
