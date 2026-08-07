# Blueprint

Decodes PDFs and extracts structured data for automated forms conversion.

## Supported Formats

**Input:**
- PDF (AcroForm)
- PDF (XFA)

**Output:**
- Structured JSON representation
- Standalone HTML
- XSD (XML Schema Definition)
- AEM Adaptive Forms package
- Redacto PostgreSQL dump

## Project Structure

| Crate | Description |
|---|---|
| `core` | Core library — PDF parsing, XFA processing, analysis pipeline, and all output renderers. |
| `cli` | Command-line interface for processing PDFs. |
| `app` | Dioxus desktop application: drag-and-drop upload driving the autonomous conversion agent. |
| `agent` | Headless conversion-agent engine — the tool catalog/executor, edit-history store, and AEM client. No UI or LLM dependency, shared by the app and the MCP server. |
| `mcp` | Model Context Protocol (stdio) server that exposes the conversion tools so an external LLM client (Claude Desktop, Claude Code, Cursor) can drive a conversion. |
| `judge` | Evaluates translation quality of multi-language PDF forms and writes scores to CSV. |

## Prerequisites

- [Rust](https://rustup.rs/) (edition 2024)
- [Dioxus CLI](https://dioxuslabs.com/learn/0.7/getting_started) — only needed for the desktop app

Dioxus can easily be installed using cargo-binstall:

```sh
cargo install cargo-binstall
cargo binstall dioxus-cli@0.7.9
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

It bundles an AI conversion agent that drives the engine's tools turn by turn to convert a form interactively. The agent uses the Anthropic API — set the API key and model (default `claude-opus-4-8`) in the app's settings. Every tree change is versioned into a local edit-history SQLite database, so conversions can be reviewed and resumed.

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

## MCP Server

The `mcp` crate is a [Model Context Protocol](https://modelcontextprotocol.io/) server that exposes the conversion tools over stdio, so an external LLM client (Claude Desktop, Claude Code, Cursor, …) can drive a conversion step by step. The client supplies the reasoning; the server supplies the tools, backed by the headless `agent` engine. It shares the same edit-history SQLite as the desktop app, so a conversion driven over MCP can later be reviewed in the app.

```sh
# Build the server binary
cargo build --release -p mcp
```

Register the built binary (`target/release/mcp`) in the client's MCP config with `command` pointing at it. The desktop app can also install the bundled server into Claude Desktop's config automatically. A call to `start_conversion` (with a `pdf_path` or `pdf_base64`, and an optional `profile`) loads a source PDF; every other tool then operates on that loaded conversion.

## Library Documentation

```sh
cargo doc -p blueprint --open
```

## Judge

The judge evaluates translation quality of multi-language PDF forms in `core/input/`. It processes all form codes in parallel using all available CPU cores and writes scores to `judge/results.csv` (override with `--input-dir`, `--profile` and `--output`).

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

## Regenerating build assets

Two scripts regenerate checked-in assets. Neither runs as part of the build; run
them by hand when the asset needs to change.

```sh
# The quantized sentence-embedding model in core/models/ (semantic matching).
pip install torch transformers safetensors
python3 scripts/download_model.py

# The desktop app icons in app/icons/, from app/assets/app-icon.svg.
pip install cairosvg pillow
python3 scripts/generate_icon.py
```
