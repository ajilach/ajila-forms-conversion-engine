# Blueprint — AEM Forms Conversion Engine

Rust workspace that converts XFA PDFs into AEM Adaptive Forms FileVault packages.

## Crates

| Crate | Binary | Purpose |
|-------|--------|---------|
| `core` | — | All PDF parsing, form-state exploration, structured output, AEM generation |
| `cli` | `blueprint` | CLI wrapper around core |
| `app` | `blueprint-app` | Dioxus web/desktop UI |
| `teacher` | — | LLM smart-edit pass (OpenAI) |
| `judge` | — | Translation quality evaluator |

## Conversion pipeline

```
PDF(s) ──► run_pipeline() ──► DocumentEnvelope (JSON) ──► to_aem_package() ──► FileVault ZIP
```

1. **Parsing** — XFA XML extracted from PDF
2. **Exhaustive searching** — all reachable form states (radio/checkbox/dropdown combos)
3. **Flattening** — render each state to PNG
4. **Structuring** — convert states to `StructuredNode` trees
5. **Merging** — merge all states + languages into one `DocumentEnvelope`

## CLI

```bash
# Build
cargo build --release -p blueprint-cli

# Convert PDF(s) → structured JSON + AEM package
cargo run --release -p blueprint-cli -- form_DE.pdf form_EN.pdf --structured --aem --profile ubs

# Render plain images for visual inspection
cargo run --release -p blueprint-cli -- form.pdf --render plain --scale 1.5

# Regenerate AEM package from edited JSON (skips PDF pipeline)
cargo run --release -p blueprint-cli -- --from-structured form_merged.json --aem --profile ubs
```

## Output file naming

Given input `AAAI_019_DE.pdf`:
- `AAAI_019_merged.json` — structured DocumentEnvelope
- `AAAI_019_merged.zip` — AEM FileVault package
- `AAAI_019_0.plain.png` — rendered form page(s)

For multilingual (`_DE` + `_EN`):
- `AAAI_019_multilingual.json`
- `AAAI_019_multilingual.zip`

## AEM credentials (env vars)

```
AEM_URL=http://localhost:4502
AEM_USER=admin
AEM_PASSWORD=admin
```

## Profile

UBS profile: `profiles/ubs/aem/`
- `config.toml` — form path templates, language mappings, theme/DOR refs
- `*.xml` — Tera component templates (root, panel, textbox, checkbox, etc.)
- `custom/` — custom component overrides (account_holder, signatures, etc.)
- `translations/` — per-language label overrides

## Running /convert

Always launch `/convert` as a **fresh agent** (not a fork). Forks inherit the full conversation context and cost ~130k tokens before doing any work. A fresh agent reads everything it needs from CLAUDE.md and the skill file on disk.

Minimal prompt template:
```
Repo: /Users/photz/repos/ubs/ajila-forms-conversion-engine
Read .claude/skills/convert-pdf.md and execute the /convert skill exactly as documented.
PDFs: <paths>
Notes: <any user notes>
```

## Fragment system

The engine automatically detects panels whose child `bindRef` elements match a known XSD type and replaces them with AEM fragment references (`fragRef=` attribute). ~195 fragment types exist under `profiles/ubs/aem/fragments/`.

**Always leave these alone** — the engine handles them correctly:
- `affrg_BankingRelationship1` — banking relationship panel
- `affrg_Address*` — address blocks
- `affrg_AccountHolder*` — account holder panels
- `affrg_IndividualBasic*`, `affrg_ClientBasic*`

**Wrong fragment assignments:** When a fragment clearly doesn't match what the PDF shows (e.g. signature fragment on an Ort/Datum section, or any fragment on a section where it doesn't belong) — fix it in the AEM XML and report it under "Engine faults — fixed locally, needs engine-level change". Fix locally so the form works; report so the engine can be corrected later.

**How to inspect fragments in a generated ZIP:**
```bash
unzip -p <name>_merged.zip 'jcr_root/**/.content.xml' | grep -o 'fragRef="[^"]*"'
```

## Field reference

`.claude/ubs-field-reference.json` — lists all field types and their expected AEM representation.
Claude reads this during conversion review to validate output correctness.

## Tests

```bash
cargo test --release
```

## AEM component naming conventions

All AEM component names follow the pattern `PREFIX_<CamelCaseName>_<shortUuid>`.

| Component | Prefix |
|-----------|--------|
| Radio Button | `RB_` |
| Date | `DATE_` |
| Text Box | `TXT_` |
| Check Box | `CB_` |
| Dropdown | `DD_` |
| Panel | `PN_` |
| Repeat Container Header Title | `RCHT_` |
| Repeat Container Panel | `RCP_` |
| Repeat Container Button Panel | `RCBP_` |
| Repeat Container Header Panel | `RCHP_` |
| Number Box | `NB_` |
| Buttons | `BT_` |
| Static Text | `ST_` |
| Telephone | `TEL_` |
| Textbox Multiline | `TXTM_` |
| Email | `EML_` |
| Image | `IMG_` |
| Chart | `CRT_` |
| Separator | `SPT_` |
| Information Text | `ITXT_` |
| Error Text | `ETXT_` |
| Interactive Table | `TBL_` |
| Barcode | `BARCODE_` |
| QR Code | `QRCODE_` |

## Common intervention patterns

- **Missing field label** — `label` is null or empty in JSON; set it manually
- **Wrong input type** — e.g. text field used instead of date; change `inputType`
- **Missing `required` flag** — check the PDF visual for mandatory markers
- **Incorrect select options** — radio/dropdown options wrong or truncated
- **Signature fragment on non-signature section** — flag in report, do not fix (engine quirk)
