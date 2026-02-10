# Blueprint UI

A desktop UI for the blueprint PDF processing crate, built with Dioxus.

## Features

- **Multi-PDF Upload**: Upload multiple PDF files in different languages
- **Visual Progress Tracking**: See real-time progress through 5 processing stages:
  1. **Parsing**: Extract XFA data from PDFs
  2. **Exhaustive Searching**: Explore all form states
  3. **Flattening**: Generate plain PNG images per state
  4. **Structuring**: Generate labeled PNG images per state
  5. **Merging**: Combine all translations into bilingual structure
- **Image Preview**: View plain and labeled state images during processing
- **JSON Export**: Download the merged bilingual structure as JSON
- **HTML Preview**: View the generated HTML form in your browser

## Building

```bash
cargo build --release
```

## Running

```bash
cargo run --release
```

## Usage

1. **Upload PDFs**: Click the file input and select one or more PDF files
2. **Start Processing**: Click "Start Processing" to begin the pipeline
3. **Monitor Progress**: Watch the progress indicators as each stage completes
4. **View Images**: See plain and labeled state images as they're generated
5. **Download Results**: 
   - Click "Download Merged JSON" to save the bilingual structure
   - Click "Preview HTML Form" to open the generated HTML in your browser

## Architecture

The UI is built on top of the `blueprint` crate and provides:
- Async processing with Tokio
- Reactive UI updates with Dioxus signals
- PNG image encoding for state visualizations
- Native file system integration for downloads

## Dependencies

- `dioxus` - Desktop UI framework
- `blueprint` - PDF processing library (from ../blueprint)
- `tokio` - Async runtime
- `serde_json` - JSON serialization
- `image` - PNG encoding
- `base64` - Image data encoding for display
