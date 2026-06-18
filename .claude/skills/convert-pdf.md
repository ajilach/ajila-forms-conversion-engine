# /convert — PDF to AEM Forms Conversion

Convert one or more XFA PDFs to a reviewed and corrected AEM FileVault package.

**Usage:** `/convert path/to/form_DE.pdf [path/to/form_EN.pdf ...] [-- any additional notes]`

Additional notes after the paths are carried into the review and fix steps as extra context. Example:
```
/convert form_DE.pdf form_EN.pdf -- date fields were wrong last time, check those carefully
/convert form_DE.pdf -- labels in section 3 must be in German
```

---

## Steps

### 0. Read reference files

Before doing anything else, read these two files:
- `.claude/ubs-field-reference.json` — ground truth for expected AEM field output
- `.claude/references/engine-bugs.md` — known engine bugs with symptoms and local fixes

Reading the engine-bugs file now lets you recognise known issues immediately during review rather than discovering them late.

### 1. Render PDF for visual inspection

```bash
cargo run --release -p blueprint-cli -- <pdfs> --render plain --scale 1.5 --profile ubs
```

Read the generated `*.plain.png` files to understand the form layout and content visually. Note all panels, fields, labels, required markers, and field types you can see.

If no PNG files are produced, **stop and report an error** — renders should always be produced for a valid XFA PDF.

### 2. Run full conversion

```bash
cargo run --release -p blueprint-cli -- <pdfs> --structured --aem --profile ubs
```

This produces:
- `<name>_merged.json` — structured DocumentEnvelope
- `<name>_merged.zip` — AEM FileVault package

### 3. Review fragment references in the generated ZIP

Extract all fragment references from the ZIP:
```bash
unzip -o <name>_merged.zip -d _frag_tmp && grep -r 'fragRef=' _frag_tmp/ | grep -o 'fragRef="[^"]*"' && rm -rf _frag_tmp
```

For each fragment found, note the XML path it appears in (which section) and cross-check against the PDF renders:
- **Leave alone** — known always-correct fragments: `affrg_BankingRelationship1`, `affrg_Address*`, `affrg_AccountHolder*`, `affrg_ClientBasic*`
- **Check for extra fields** — `affrg_IndividualBasic*`: the fragment reference itself is correct, but the panel holding it may be missing sibling fields — see the extra-field check note in the table below
- **Fix and report** — if a fragment clearly doesn't match what the PDF shows (e.g. a signature fragment on an Ort/Datum section, or a wrong fragment on any other section): determine what the correct output should be from the PDF render, fix it in the AEM XML via Step 6 (replace the fragment node with the correct fragment ref, or replace with individual field components if no matching fragment exists), and add it to the report under "Engine faults — fixed locally, needs engine-level change". This way the form works and the engine can be improved later.

**Known fragment contents** (for quick cross-check without reading the PDF):

| Fragment | Provides |
|----------|---------|
| `affrg_BankingRelationship1` | Clearing-Nr., Konto-Nr., Bankbeziehung (hidden) |
| `affrg_IndividualBasic1` | Nachname, Vorname(n) **only** — see note below |
| `affrg_Address*` | Street address block (Strasse, PLZ, Stadt, Land, …) |
| `affrg_AccountHolder*` | Full account holder identity block |
| `affrg_SignatureGeneric1` | Signature capture widget — only valid on signature sections |

**`affrg_IndividualBasic1` extra-field check:** This fragment provides only Nachname + Vorname(n). If the PDF shows additional fields in the same customer block, those fields are **not** provided by the fragment and must be added manually.

**Critical — fragRef panels are opaque:** AEM ignores any child nodes placed inside a panel that has a `fragRef` attribute. Extra fields must always be **siblings** of the fragRef panel (placed directly after its closing tag in the parent `<items>`), never children inside it. The engine sometimes places extra field panels incorrectly inside the fragRef panel's items — always verify in the XML and move them out if needed (see BUG-001 in `engine-bugs.md`).

### 3b. Detect repeatable→signature sync

From the PDF renders, check if the form has **both**:
- A repeatable section: a person/customer block with visible Add / Remove buttons
- A separate signature section that should have one signature slot per customer

If both are present, the conversion engine does **not** generate the sync scripts automatically. Flag the following as XML-only additions for Step 6:
- Add button `fd:click` — paired `addInstance()` on the customer repeatable and the signature repeatable
- Remove button `fd:click` — paired `removeInstance()` at the same index, plus re-show the Add button
- Vorname + Nachname `fd:valueCommit` — write the combined name into the matching signature instance by index

Read `.claude/references/repeatable-signature-sync.md` for the exact XML attribute templates and instructions on resolving the component name placeholders. `references/AAGO_019_DE.zip` is a concrete working reference — unzip and search for `instanceManager` or `valueCommit` if needed.

If neither condition is present, skip this step.

### 4. Review the structured JSON

Read `<name>_merged.json`.

Do a **bidirectional** comparison between the rendered PDF images and the JSON:

**Diagnose first, then fix.** Before deciding how to fix anything, always check both the JSON and the ZIP XML (see the decision matrix below). The engine can produce correct XML even when the JSON looks wrong — never assume JSON is the right source to edit without verifying. Once you have confirmed what needs fixing: prefer JSON over XML where the schema can express it (JSON edits are cheaper and regeneration picks them up), and only use XML patching for attributes the JSON schema genuinely cannot express.

**PDF → JSON** (nothing in the PDF should be missing from the JSON):
- Every section/panel visible in the PDF must have a corresponding panel in the JSON, with the correct title/label as it appears in the PDF
- Every data-entry field visible in the PDF must appear in the JSON
- The order of nodes in the JSON must match the visual top-to-bottom order in the PDF — wrong ordering is a JSON fix
- Pre-filled values visible in the rendered PDF (e.g. a field that already shows a fixed value) must be captured — these need a default value and are usually read-only; note them for the XML patch step (fd:init script + readOnly)

**JSON → PDF** (nothing in the JSON should be absent from the PDF):
- Every panel in the JSON must correspond to a visible section in the PDF. Cross-check each panel title against the rendered image.
- A section with a title + fragments is valid even if it has no standalone fields — the fragment provides the content.
- A section with a title but **no fragments and no fields** is a ghost — remove it entirely.
- If the PDF shows a section with content but the JSON/XML only has the title, the engine failed to generate the fragments — add the appropriate fragments manually with the correct bindRef path mirroring the equivalent sibling section.
- Every standalone field in the JSON must be visible in the PDF. Extra fields not in the PDF are engine artefacts — remove them.

**Before categorising any fix: check both the JSON and the ZIP XML.**

The JSON and the ZIP XML are generated independently. Never assume the XML is correct just because it looks complete — the engine generates both and can be wrong in either. Never assume the JSON is wrong just because it looks different from the PDF — always verify the XML first:

```bash
unzip -o <name>_merged.zip -d _check_tmp
grep -A 30 '<fieldname' _check_tmp/jcr_root/content/forms/af/.../.content.xml
rm -rf _check_tmp
```

Only after checking both sources, categorise each issue:

- **JSON wrong, XML correct** → **do not touch the JSON**. The ZIP (XML) is what gets uploaded to AEM; the JSON is only a source for regeneration. Note in the report that XML is already correct and no action is needed. If it is genuinely unclear whether the XML is correct (e.g. it looks implausible), **ask the user** before proceeding — do not silently accept or silently fix.
- **JSON wrong, XML also wrong** → fix JSON, then regenerate ZIP (Step 7)
- **JSON correct, XML wrong** → XML patch only (Step 6); skip JSON edit and regeneration
- **XML-only attribute** (readOnly, fd:init, AEM-specific properties not in the FieldNode schema, etc.) → XML patch only — these are the only legitimate reason to skip a JSON fix

**On `jcr:lastModifiedBy`:** Neither `"admin"` nor `"blueprint"` on a node means it is correct — the engine (`blueprint`) generates both the JSON and the XML and can be wrong in both. Review every node against the PDF regardless of the modifier value.

Specific issues to look for (fix in JSON wherever possible):
- **Missing fields or panels** — visible in PDF but absent from JSON → check XML first, then fix JSON if also absent
- **Wrong or missing labels** — any text visible in the PDF (field label, panel title, static text) that doesn't match the JSON → JSON-fixable
- **Wrong node order** — nodes in the JSON don't match the visual top-to-bottom order in the PDF → JSON-fixable
- **Wrong input type** — e.g. date rendered as `text` → check XML first, fix JSON if also wrong
- **Missing required flags** — PDF shows `*` but `"required": false` → JSON-fixable
- **Incorrect or incomplete options** — check `stateCount` in the JSON root: if it is lower than the number of visible options in the PDF, the engine did not explore all states. Check the XML — it may also be wrong or may have been manually corrected outside the engine. Compare both against the PDF and fix whichever is wrong (JSON if both wrong; XML patch only if JSON already correct).
- **Ghost section** — JSON panel has no counterpart in PDF AND no repeatable container → remove entirely. A panel showing only a titledraw in the inspector is not automatically a ghost — it may contain a repeatable whose inner fields the inspector does not descend into. Cross-check against the PDF before removing.
- **Repeatable min/max occurrences** — if the JSON has `"minOccurrences"` or `"maxOccurrences"` on a repeatable panel, verify the XML `minOccur`/`maxOccur` attributes match (see BUG-003 in `engine-bugs.md` for the full fix procedure including button scripts).
- **Pre-filled read-only value** — field shows a fixed value in PDF → XML-only (fd:init + readOnly), cannot be expressed in JSON
- **Read-only field** — field should not be editable → XML-only, cannot be expressed in JSON
- **AEM component property** — anything not in the `FieldNode` schema → XML-only, cannot be expressed in JSON

**What to ignore:** Pure UI/navigation elements — submit buttons, "Add" / "Remove" repeat buttons, green action buttons, decorative separators. Only review data-entry fields and their labels.

### 5. Apply JSON fixes

Edit `<name>_merged.json` directly using the Edit tool to correct all JSON-fixable issues.

### 6. XML patch cycle

If there are any XML-only fixes (including fragment corrections from Step 3), do the full patch cycle — unzip, fix everything in one pass, rezip:
```bash
unzip -o <name>_merged.zip -d _pkg_tmp
# Edit XML files in _pkg_tmp/jcr_root/ with the Edit tool
cd _pkg_tmp && zip -r ../<name>_merged.zip . && cd .. && rm -rf _pkg_tmp
```

### 7. Regenerate AEM package from fixed JSON

Delete the existing ZIP first so the file is guaranteed to be fresh:
```bash
rm -f <name>_merged.zip
```

```bash
cargo run --release -p blueprint-cli -- --from-structured <name>_merged.json --aem --profile ubs
```

This overwrites `<name>_merged.zip` with the corrected package.

Skip this step if you used the XML patch cycle in Step 6 (the ZIP is already up to date).

### 8. Report and hand off to /install

Provide a structured report with three sections:

**JSON vs XML discrepancies:**
For every case where the JSON and the generated XML differed, state: what the difference was, which source was trusted, and why (e.g. "XML had correct fragment options that JSON was missing — XML kept as-is, JSON not touched").

**Fixed in JSON (regenerated ZIP):**
List every issue fixed via JSON edit + regeneration, with: what was wrong, what was changed.

**Fixed in XML only (JSON cannot express this):**
List every change that required a direct XML patch because the JSON schema has no equivalent. This section is the signal for future JSON schema improvements — keep it honest and complete.

**Not fixed — needs manual work:**
List every issue that could not be resolved automatically, with: field name, what is wrong, what the expected correct state is.

**Engine faults:**
List any issues that were fixed locally but indicate a bug in the conversion engine that needs to be fixed upstream (e.g. wrong fragment assigned, stateCount too low, stray artefact nodes, extra fields placed inside fragRef panels).

For each engine fault in this section: check whether it already appears in `.claude/references/engine-bugs.md`. If it matches a known bug, note the BUG-ID. If it is a new bug not yet in the file, **append it** to `engine-bugs.md` using the same format (BUG-NNN, first observed, symptom, local fix, expected engine behaviour). Assign the next sequential BUG-NNN number.

Also include the output ZIP path.

Then immediately continue with the `/install` skill — pass the generated ZIP path and carry any unresolved issues forward as additional notes:

```
/install <name>_merged.zip -- <any unresolved issues from this conversion>
```

---

## Notes

- For multilingual forms the CLI merges the PDFs automatically; just pass all language variants as arguments
- The field reference at `.claude/ubs-field-reference.json` is the ground truth for expected output — update it when new patterns are established
- Profile templates are at `profiles/ubs/aem/` — if the same issue recurs across multiple forms, propose a template fix there instead of patching each JSON manually

## How to update this skill file

When adding new instructions based on issues encountered, follow these rules:

- **State principles, not examples.** Describe the general rule that applies to any form, not the specific field names or section titles from the form that triggered the insight.
- **No form-specific content.** Never mention specific field names, panel names, or values from a particular form. The skill must work for every future form.
- **Categorise correctly.** Every new instruction belongs in exactly one place: a step (what to do), the checklist (what to look for), the decision matrix (how to decide), or the report (what to surface). Don't add free-floating paragraphs.
- **Preserve the diagnose-first principle.** Any new fix pattern must be classified as either JSON-fixable or XML-only, and must respect the decision matrix: always check XML before touching JSON. If it is JSON-fixable, it goes in the checklist as JSON-fixable. If it is XML-only, the reason must be stated ("cannot be expressed in JSON").
