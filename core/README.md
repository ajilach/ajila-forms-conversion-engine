# Blueprint

Decodes PDFs and extracts structured data for automated forms conversion. 

## Supported Formats

Input formats:

* PDF
* PDF (XFA)

Output formats:

* Structured JSON representation
* HTML
* AEM Package

## Usage

This crate can be used both as a CLI tool or a library.

### CLI Tool

```
# Basic analysis (processes the PDF, no file output)
cargo run -- input/AAAI_019_DE.pdf

# Export structured JSON
cargo run -- input/AAAI_019_DE.pdf --structured

# Export standalone HTML
cargo run -- input/AAAI_019_DE.pdf --html

# Export AEM Adaptive Forms package (XFA PDFs only)
cargo run -- input/AAAI_019_DE.pdf --aem

# Render images (modes: plain, labelled, annotated; repeatable)
cargo run -- input/AAAI_019_DE.pdf --render plain --render labelled

# Custom render scale (default 1.5)
cargo run -- input/AAAI_019_DE.pdf --render plain --scale 2.0

# Multilingual merge (pass multiple language variants)
cargo run -- input/AAAI_019_DE.pdf input/AAAI_019_EN.pdf --structured --html

# Dump raw XFA XML and exit
cargo run -- input/AAAI_019_DE.pdf --dump-xfa

# Suppress verbose output
cargo run -- input/AAAI_019_DE.pdf --structured -q
```

### Library

You can ceck out the library documentation by running `cargo doc --open`.

## Architecture

### Pipeline

```text
PDF bytes
  │
  ├─[XFA]──────► extract XFA XML ─► XfaNode tree ─► XfaForm (with scripting)
  ├─[AcroForm]─► parse fields + content streams ──────────┐
  │                                                        ▼
  ▼                                                   Vec<Flattened>
Exhaustive exploration (toggle radios/checkboxes/dropdowns)
  │
  ▼
Vec<FormState>
  │
  ├─► Document (analysis pipeline) ─► Vec<StructuredNode>
  │       │                                    │
  │       └─► render_*() ─► RgbaImage          ├─► HTML
  │                                            ├─► JSON
  │                                            └─► AEM Package
  │
  └─► RecursiveMerger ─► single tree with ConditionalNodes
```

### Modules

| Module | Purpose |
|---|---|
| `pdf_parser` | Parses AcroForm PDFs via `lopdf`. Extracts positioned text runs, form fields, and glyph-to-Unicode mappings. |
| `xfa` | Parses XFA XML into an `XfaNode` tree, wraps it in `XfaForm` with SOM path resolution and a scripting engine for calculate/validate events. Includes font management and text metrics. |
| `exhaustive` | Explores all reachable form states by toggling every radio button, checkbox, and dropdown. Deduplicates via structural keys to avoid redundant exploration. |
| `flattened` | Absolute-position layout model that bridges parsing and analysis. Each `FlattenedNode` carries bounds, content kind (text, field, image, line, …), font info, and semantic hints. Also handles rendering to `RgbaImage`. |
| `document` | Analysis layer. Builds a hierarchy of `Group`s from flat leaves using ~20 ordered analysis modules (heading detection, field grouping, radio/checkbox detection, table detection, label attachment, etc.). Modules run via the `AnalysisModule` trait. |
| `structured` | Converts the analyzed `Document` into a serialisable `StructuredNode` tree wrapped in a `DocumentEnvelope`. Includes the `RecursiveMerger` for merging multiple form states into conditional trees, and a `TranslationMerger` for multilingual LCS-based alignment. |
| `html` | Renders `Vec<StructuredNode>` into a standalone HTML page with embedded CSS/JS for dynamic repeatables, conditionals, and multilingual support. |
| `aem` | Converts structured nodes into an AEM Adaptive Forms JCR content package (XML + FileVault ZIP). |
| `context` | Pipeline-wide metadata: detected language, XFA variables, enriched by analysis modules. |

### Key Types

| Type | Role |
|---|---|
| `Blueprint` | Main façade. Auto-detects XFA vs AcroForm. Entry point for all processing. |
| `FormStates` / `FormState` | Result of exhaustive exploration. Each state holds a `Flattened` snapshot, the `Vec<Selection>` that produced it, and a shared `GlobalContext`. |
| `Flattened` / `FlattenedNode` | Absolute-positioned page layout with typed content nodes. |
| `Document` / `Group` / `GroupKind` | Analysis model. Groups are progressively built from leaves by analysis modules. `GroupKind` encodes semantics (text block, field, heading, radio button, table, repeatable section, etc.). |
| `StructuredNode` | Semantic output enum (Heading, Paragraph, Field, Table, Repeatable, Conditional, …). |
| `DocumentEnvelope` | Final pipeline output: `Context` + `Vec<StructuredNode>`. |
| `FieldNode` / `FieldId` | A form field with a deterministic UUID (v5 from SOM path), label, type, value, and placeholder. |
| `TranslatableString` | `Plain(String)` or `Translated(HashMap<lang, String>)` for multilingual content. |