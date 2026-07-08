---
name: iterate-form
description: >
  Iterate on an existing AEM Adaptive Form package — apply targeted changes
  described in natural language using the original PDF(s) as the source of
  truth. Use whenever the user wants to modify dropdowns (including cascading
  options), conditional visibility, labels, enum values, rules, or any other
  part of a package that has already been built. The skill receives the change
  description, the original PDF path(s), and the path to the existing AEM
  package (.zip).
---

Apply targeted, validated changes to an existing AEM Adaptive Form package, using the PDF as the source of truth. Designed for human-in-the-loop iteration: small, reviewable changes that build on the current package state.

## Entry

Treat `$ARGUMENTS` as the change description. Ask for what is missing:

- **Change description** — natural language: which field/dropdown, what should change, expected behavior.
- **PDF path(s)** — absolute path(s) to the original source PDF(s).
- **Package path** — absolute path to the existing AEM package (`.zip`).
- **Profile** — same profile used to build the package (e.g. `ubs`). If unsure, ask the user.

Call `mcp__blueprint__start_conversion` with the PDF path(s) and profile, including the existing package path so the engine loads its current state.

Verify the state loaded by calling `get_aem_xml_outline`. If the outline is empty or clearly does not match the existing package, stop and tell the user — do not proceed against an empty tree.

## Workflow

### Phase 1 — Locate (read-only, targeted)

Find the field(s) the change description refers to, in both the PDF and the AEM tree.

- `search_xfa` with the labels/names from the description — never the full `get_xfa` unless strictly necessary.
- `get_aem_xml_outline` to see the current AEM shape.
- `get_aem_xml_node` on the specific node(s) you plan to touch — read existing attributes and child rules **before** editing, so you don't overwrite logic that is already correct.
- If the description is visual ("the dropdown next to X"), call `get_annotated_state_image` on the relevant state.

For cascading-dropdown work specifically: read the XFA scripts on the source dropdowns (`search_xfa` with the field name; the original logic typically lives in `calculate` or `change` events) so you can faithfully translate it into AEM rules.

### Phase 2 — Plan

Tell the user, in one short block, **before any edit**:

- Which AEM nodes you will modify (path + node name).
- What you will change (which attributes, which new rule nodes, which enum options).
- Why — which PDF logic this implements.

Wait for confirmation when the change is non-trivial (new rule branches, new conditional fields, reorganising enums). For trivial fixes (single attribute, typo) apply directly.

### Phase 3 — Apply (granular editors only)

Use ONLY:

- `set_aem_xml_attribute` — change attributes on existing nodes (enum, enumNames, visibility expressions, rule predicates).
- `insert_aem_xml_node` — add new rule/condition nodes or new conditional fields.
- `replace_aem_xml_node` — swap a whole subtree. Use sparingly; prefer attribute edits.
- `remove_aem_xml_node` / `remove_aem_xml_attribute` — remove obsolete logic.

For cascading dropdowns and conditional visibility, the conditional logic typically lives in `fd:rules` (or equivalent) child nodes under each field. Inspect first with `get_aem_xml_node`, then modify in place — never overwrite a whole rules block when you only need to add or change one branch.

**Never** call any `set_aem_translated*` or `set_structured` editor here — those re-emit large subtrees and would undo earlier granular work.

### Phase 4 — Validate

- `build_aem_package`
- `validate_aem_package`

**Validate escape:** if `validate_aem_package` returns the same errors 3 times in a row despite fixes, stop. Rebuild once more, report the unresolved issues, and do not loop indefinitely.

If an AEM connection is configured and the user asked to deploy: `upload_to_aem`, then `fetch_aem_form_html` to confirm the change is live in the deployed form.

### Phase 5 — Iterate

Ask: **"¿Algún otro cambio?"**

If yes, stay in the same session. Skip PDF inspection unless the new change references fields you have not read yet — reuse what is already in context. Go straight to Plan → Apply → Validate.

When done, call `mcp__blueprint__finish` with a one-sentence summary of what was changed.

## Hard rules

- Never re-emit the whole AEM tree — granular XML editors only.
- Never invent labels, option values, or rule expressions — quote them from the XFA; verify with `search_xfa`.
- Never silently overwrite existing rules — inspect first with `get_aem_xml_node`, then modify in place.
- Never call `finish` while `validate_aem_package` reports structural errors, unless the validate escape was triggered (in which case state the unresolved issues in the summary).
- Match the source's languages exactly — never drop or add one when editing per-language attributes.
- Never proceed against an empty AEM tree. If `get_aem_xml_outline` returns nothing after `start_conversion`, stop and tell the user.
