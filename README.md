# Blueprint

Decodes PDFs and extracts structured data for automated forms conversion.

## Supported Formats

**Input:**
- PDF (AcroForm)
- PDF (XFA)

**Output:**
- Structured JSON representation
- Standalone HTML
- AEM Adaptive Forms package

## Project Structure

| Crate | Description |
|---|---|
| `core` | Core library — PDF parsing, XFA processing, analysis pipeline, and all output renderers. |
| `cli` | Command-line interface for processing PDFs. |
| `app` | Dioxus web/desktop application with drag-and-drop upload and live preview. |

## Prerequisites

- [Rust](https://rustup.rs/) (edition 2024)
- [Dioxus CLI](https://dioxuslabs.com/learn/0.6/getting_started) — only needed for the web app

```sh
cargo install dioxus-cli
```

## Running Tests

```sh
# Run the full test suite (--release is recommended for speed)
cargo test --release
```

## Running Benchmarks

Benchmarks live in `core/benches/` and use [Criterion](https://github.com/bheisler/criterion.rs). They automatically discover all PDFs in `core/input/`.

```sh
cargo bench -p blueprint
```

## CLI

The CLI binary is defined in the `cli` crate.

```sh
# Basic analysis (no file output)
cargo run --release -p blueprint-cli -- path/to/form.pdf

# Export structured JSON
cargo run --release -p blueprint-cli -- path/to/form.pdf --structured

# Export standalone HTML
cargo run --release -p blueprint-cli -- path/to/form.pdf --html

# Export AEM Adaptive Forms package (XFA PDFs only)
cargo run --release -p blueprint-cli -- path/to/form.pdf --aem

# Use a profile for output-specific configuration
cargo run --release -p blueprint-cli -- path/to/form.pdf --aem --profile profiles/ubs

# Render images (modes: plain, labelled, annotated; repeatable)
cargo run --release -p blueprint-cli -- path/to/form.pdf --render plain --render labelled

# Custom render scale (default 1.5)
cargo run --release -p blueprint-cli -- path/to/form.pdf --render plain --scale 2.0

# Multilingual merge (pass multiple language variants)
cargo run --release -p blueprint-cli -- form_DE.pdf form_EN.pdf --structured --html

# Dump raw XFA XML and exit
cargo run --release -p blueprint-cli -- path/to/form.pdf --dump-xfa
```

## Web App

The app is built with [Dioxus](https://dioxuslabs.com/) and supports web (WASM + server) and desktop targets.

### Development

```sh
cd app
dx serve --platform web --fullstack
```

### Production build

```sh
cd app
dx build --release --platform web --fullstack
```

### Docker

A Docker image is published to GitHub Container Registry with every release.

```sh
docker pull ghcr.io/ajilach/blueprint-app:latest
docker run -p 8080:8080 ghcr.io/ajilach/blueprint-app:latest
```

Then open http://localhost:8080.

## Library Documentation

```sh
cargo doc -p blueprint --open
```
