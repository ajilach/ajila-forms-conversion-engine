# blueprint (core)

The conversion engine: PDF parsing, XFA processing, the analysis pipeline, and
every output renderer. A library crate — it has no binary.

For CLI usage see the [root README](../README.md) (`cargo run -p blueprint-cli`);
this file documents the architecture.

```sh
cargo doc -p blueprint --open
```

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
  │                                            ├─► XSD
  │                                            ├─► Redacto SQL dump
  │                                            └─► AEM package
  │
  └─► RecursiveMerger ─► single tree with ConditionalNodes
```

An output target (`Aem` or `Redacto`, see `target.rs`) selects which branch a
run takes and which section of the profile configures it.

### Modules

| Module | Purpose |
|---|---|
| `pdf_parser` | Parses AcroForm PDFs via `lopdf`. Extracts positioned text runs, form fields, and glyph-to-Unicode mappings. |
| `xfa` | Parses XFA XML into an `XfaNode` tree, wraps it in `XfaForm` with SOM path resolution and a scripting engine for calculate/validate events. Includes font management and text metrics. |
| `exhaustive` | Explores all reachable form states by toggling every radio button, checkbox, and dropdown. Deduplicates via structural keys to avoid redundant exploration. |
| `flattened` | Absolute-position layout model that bridges parsing and analysis. Each `FlattenedNode` carries bounds, content kind (text, field, image, line, …), font info, and semantic hints. Also handles rendering to `RgbaImage`. |
| `document` | Analysis layer. Builds a hierarchy of `Group`s from flat leaves using ~20 ordered analysis modules (heading detection, field grouping, radio/checkbox detection, table detection, label attachment, …). Modules run via the `AnalysisModule` trait. |
| `structured` | Converts the analyzed `Document` into a serialisable `StructuredNode` tree wrapped in a `DocumentEnvelope`. Includes the `RecursiveMerger` for merging multiple form states into conditional trees, and a `TranslationMerger` for multilingual LCS-based alignment. |
| `semantic` | Sentence-embedding matcher (`candle` + a quantized MiniLM in `models/`) used to align translations that differ structurally. Behind the default `semantic-matching` feature. |
| `html` | Renders `Vec<StructuredNode>` into a standalone HTML page with embedded CSS/JS for dynamic repeatables, conditionals, and multilingual support. |
| `xsd` | Renders the structured tree as an XML Schema Definition. |
| `aem` | Converts structured nodes into an AEM Adaptive Forms JCR content package (XML + FileVault ZIP), and parses one back. Includes package validation. |
| `redacto` | Renders the structured tree as a Redacto PostgreSQL dump, with its own content model and validation. |
| `review` | Post-conversion fidelity review: compares the source parse against a converted tree and reports missing content with a coverage score. |
| `profiles` | Per-customer configuration loaded from `profiles/<name>/<target>/`: AEM config, naming rules, templates, fonts. |
| `template` | The Tera templating layer the AEM and Redacto writers render through. |
| `reference_db` | Schema for the reference-form store the agent searches (written by the `agent` crate). |
| `graphviz` | Debug renderer for the analysis tree. |
| `context` | Pipeline-wide metadata: detected language, XFA variables, recovered master-page header, enriched by analysis modules. |
| `pipeline` | Convenience entry point wiring parse → explore → analyse → structure for a set of PDFs. |

### Key types

| Type | Role |
|---|---|
| `Blueprint` | Main façade. Auto-detects XFA vs AcroForm. Entry point for all processing. |
| `FormStates` / `FormState` | Result of exhaustive exploration. Each state holds a `Flattened` snapshot, the `Vec<Selection>` that produced it, and a shared `GlobalContext`. |
| `Flattened` / `FlattenedNode` | Absolute-positioned page layout with typed content nodes. |
| `Document` / `Group` / `GroupKind` | Analysis model. Groups are progressively built from leaves by analysis modules. `GroupKind` encodes semantics (text block, field, heading, radio button, table, repeatable section, …). |
| `StructuredNode` | Semantic output enum (Heading, Paragraph, Field, Table, Repeatable, Conditional, …). |
| `DocumentEnvelope` | Final pipeline output: `Context` + `Vec<StructuredNode>`. |
| `AemNode` / `AemNodeTranslated` | The AEM intermediate model. `AemNodeTranslated` carries every language in place and is what the conversion agent authors; lowering it yields an `AemNode` plus a translation dictionary. |
| `OutputTarget` | Which artefact a run produces (`Aem` or `Redacto`), and which profile section configures it. |
| `FieldNode` / `FieldId` | A form field with a deterministic UUID (v5 from SOM path), label, type, value, and placeholder. |
| `TranslatableString` | `Plain(String)` or `Translated(HashMap<lang, String>)` for multilingual content. |

## Tests

Tests live in `src/tests/` and assert against the intermediate structures
(`StructuredNode`, `FlattenedNode`, `Document`, …) rather than rendered output
files. Fixtures come from `input/` (git-lfs).

```sh
cargo test --release -p blueprint
```
