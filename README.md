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
| `app` | Dioxus desktop application with drag-and-drop upload and live preview. |
| `judge` | Evaluates translation quality of multi-language PDF forms and writes scores to CSV. |
| `teacher` | Runs the pipeline and an LLM smart-edit pass, then prints suggested changes. |

## Prerequisites

- [Rust](https://rustup.rs/) (edition 2024)
- [Dioxus CLI](https://dioxuslabs.com/learn/0.6/getting_started) — only needed for the desktop app

Dioxus can easily be installed using cargo-binstall:

```sh
cargo install cargo-binstall
cargo binstall dioxus-cli@0.7.3
```

In order to version large files we need the git lfs extension

```sh
brew install git-lfs
git lfs install
git lfs pull
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

# Export AEM Adaptive Forms JCR content XML (XFA PDFs only)
cargo run --release -p blueprint-cli -- path/to/form.pdf --aem

# Export XSD (XML Schema Definition)
cargo run --release -p blueprint-cli -- path/to/form.pdf --xsd

# Use a profile for output-specific configuration
cargo run --release -p blueprint-cli -- path/to/form.pdf --aem --profile ubs

# Export GraphViz DOT decision flow
cargo run --release -p blueprint-cli -- path/to/form.pdf --graphviz

# Render images (modes: plain, labelled, annotated; repeatable)
cargo run --release -p blueprint-cli -- path/to/form.pdf --render plain --render labelled

# Custom render scale (default 1.5)
cargo run --release -p blueprint-cli -- path/to/form.pdf --render plain --scale 2.0

# Enable analysis modules
cargo run --release -p blueprint-cli -- path/to/form.pdf --module ubs

# Multilingual merge (pass multiple language variants)
cargo run --release -p blueprint-cli -- form_DE.pdf form_EN.pdf --structured --html

# Dump raw XFA XML and exit
cargo run --release -p blueprint-cli -- path/to/form.pdf --dump-xfa
```

## App

The app is built with [Dioxus](https://dioxuslabs.com/) and targets the desktop. This is the recommended way of running the migration engine.

### Development

```sh
cd app
dx serve --platform desktop
```

### Production Build

```sh
cd app
dx build --release --platform desktop
```

## Library Documentation

```sh
cargo doc -p blueprint --open
```

## Judge

The judge evaluates translation quality of multi-language PDF forms in `core/input/`. It processes all form codes in parallel using all available CPU cores and writes scores to `judge/results.csv`.

```sh
# Run the judge on all form codes (parallel)
cargo run --release -p judge

# Run the judge on a single form code
cargo run --release -p judge -- --form-code ABCD_019

# Compare results against a baseline
cd judge
cp results.csv results-baseline.csv
# ... make changes ...
cargo run --release -p judge
python3 compare.py
```

## Teacher

The teacher runs the pipeline on a PDF and then performs an LLM smart-edit pass, printing the suggested changes. It requires an OpenAI API key.

```sh
# Run the teacher on a PDF (uses the OPENAI_API_KEY environment variable)
export OPENAI_API_KEY=sk-...
cargo run --release -p teacher -- path/to/form.pdf

# Use a specific model and profile
cargo run --release -p teacher -- path/to/form.pdf --model gpt-4o --profile ubs
```
