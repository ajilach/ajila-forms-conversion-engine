# .claude/

Claude Code project configuration and automation for the Blueprint conversion engine.

---

## skills/

Slash-command skills invoked via `/skillname` inside Claude Code.

| File | Command | What it does |
|------|---------|--------------|
| `convert-pdf.md` | `/convert` | Full PDF→AEM pipeline: parse XFA, render PNG, structure fields, merge languages, generate FileVault ZIP. Always run as a **fresh agent** (not a fork). |
| `install-aem.md` | `/install` | Upload a FileVault ZIP to AEM, run JCR inspection, iterate on fixes in a loop until the user is satisfied. |

---

## scripts/

Helper scripts used by the skills. Not invoked directly — called from within a skill run.

### `aem_inspect.py`

Reads the AEM form component tree from stdin (piped from curl) and prints a structured report.

**Reports:**
- All wizard panels with their input fields (type, label, component name) and section titles (titledraw)
- Fragment assignments (`FRAGMENT` lines) so they can be verified against the PDF

**Flags as issues:**
- `!! STRAY TEXT` — textdraw node with non-empty content (engine artefact, should be deleted)
- `!! MISSING LABEL` — input field with no `jcr:title`
- `!! DUPLICATE NAME` — two fields share the same `name` attribute across the form
- `!! ENGINE DUPLICATE` — standalone field whose label is already provided by a fragment in the same panel (engine bug)

**Usage:**
```bash
set -a && source .env && set +a
curl -s -u "$AEM_USER:$AEM_PASSWORD" \
  "$AEM_URL<form_jcr_path>/jcr:content/guideContainer/rootPanel/items.tidy.6.json" \
  | python3 .claude/scripts/aem_inspect.py
```

Requires `AEM_URL`, `AEM_USER`, `AEM_PASSWORD` in the environment (sourced from `.env`).

### `fragment_coverage.json`

Curated map of fragment name → list of German field labels the fragment already provides internally.

Used by `aem_inspect.py` to detect engine duplicates: if a standalone field in the same panel has a label that appears in this map, it's flagged as `!! ENGINE DUPLICATE`.

**Extend this file** when a new form reveals a fragment overlap that isn't yet covered. Keys are the fragment component name (last path segment of `fragRef`), values are lowercase German labels.

Current entries:

| Fragment | Covered labels |
|----------|---------------|
| `affrg_BankingRelationship1` | bankbeziehung, clearing-nr., konto-nr. |
| `affrg_IndividualBasic1` | nachname, vorname(n) |

---

---

## references/

Pattern documentation for the agent. Not invoked directly — read during skill execution when the pattern applies.

| File | Topic |
|------|-------|
| `engine-bugs.md` | Confirmed engine bugs (BUG-001…): symptom, local fix, expected engine behaviour. Read in Step 0 of every conversion. Append new bugs as they are discovered. |
| `repeatable-signature-sync.md` | XML templates and step-by-step guide for wiring repeatable customer sections to paired signature sections (`instanceManager` sync + `fd:valueCommit` name sync). Read this when a form has a repeatable person/customer block with a matching signature section. |

---

## ../references/

Working reference templates — full FileVault ZIPs of known-good forms.

| File | What it demonstrates |
|------|---------------------|
| `AAGO_019_DE.zip` | Repeatable customer section with paired signature sync: `fd:click` on Add/Remove buttons, `fd:valueCommit` name sync on name fields, conditional signature sub-panels per Formular-Adressat type. |

---

## settings.json

Project-level tool permission allowlist for Claude Code. Controls which `Bash(...)` patterns are auto-approved without a prompt.

---

## ubs-field-reference.json

Reference catalogue of all UBS AEM field types and their expected component representation. Read by the `/convert` agent during review to validate that the engine produced the correct component for each field.
