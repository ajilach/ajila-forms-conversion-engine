---
name: cascading-dropdowns
description: >
  Implement PDF cascading (chained) dropdown logic — where selecting one
  dropdown value determines the option list or fixed value of one or more
  downstream dropdowns — as an AEM Adaptive Form. Use whenever the user asks
  to convert or fix a dependent/linked/cascading dropdown, a "dropdown that
  changes another dropdown", or describes XFA behaviour like "when Bereich
  changes, Kundensegment options change" or "Code should auto-fill based on
  Kundensegment". This is a specialization of iterate-form for exactly this
  pattern — prefer this skill over iterate-form whenever the change request
  is about a chain of 2+ dependent dropdowns rather than a single field. The
  skill receives the user's description of the desired cascade, the source
  PDF path(s), and the path to the existing AEM package (.zip).
---

Implement chained-dropdown behaviour from an XFA PDF as a working AEM Adaptive Form, using the PDF's own JavaScript as the source of truth. This is the sibling of `iterate-form` specialized for one recurring, failure-prone pattern: cascading dropdowns don't survive a literal translation into AEM's `valueCommit`/`.enum` runtime, so this skill encodes a proven alternative — static option variants gated by visibility rules — derived from the AAIU_019_DE conversion.

Read this end to end before touching any tool; the phases build on each other and skipping one is where past attempts broke.

## Entry

Treat `$ARGUMENTS` as the cascade description. Ask for what is missing:

- **Cascade description** — natural language: which trigger dropdown, which downstream dropdown(s) it drives, any specifics the user already knows (labels, expected codes).
- **PDF path(s)** — absolute path(s) to the original source PDF(s).
- **Package path** — absolute path to the existing AEM package (`.zip`) to iterate on.
- **Profile** — same profile used to build the package (e.g. `ubs`). If unsure, ask the user.

Call `mcp__blueprint__start_conversion` with the PDF path(s) and profile, including the existing package path so the engine loads its current state. Verify with `get_aem_xml_outline` — if it's empty or clearly doesn't match the existing package, stop and tell the user rather than splicing into an empty tree.

## Workflow

### Phase 1 — Find the cascade in the XFA

The runtime behaviour lives in JavaScript inside the XFA layer, not in the visual layout, so read the script before assuming anything about the UI.

1. Start from the trigger field the user named. Use `search_xfa` with the label first (e.g. `"Bereich"`), then with the technical field name once you see it (e.g. `.Bereich.Bereich.rawValue`) — never dump the whole XFA.
2. Look for a change-event handler on that field. The giveaway is a call inside an event, e.g. `soLocalLabelDefinition.setKundensegment(xfa.event.newText);` — this tells you which function body defines the cascade.
3. Locate that function with `search_xfa` using a regex on the signature (e.g. `function setKundensegment(`). If the body is large, `get_xfa` will offer to dump it to a file — delegate reading that file in offset/limit chunks to a subagent with an explicit "quote verbatim, don't summarise" instruction, and have it return only the quoted conditional branches.
4. Inside the extracted JS, recognise the cascade primitives:
   - `<child>.clearItems(); <child>.addItem("...")` — picklist manipulation; count and record every `addItem` per branch.
   - `<child>.rawValue = "..."` — the script forces a specific value.
   - `<child>.access = "open" | "protected"` — `"protected"` after a `rawValue` assignment means the child is a fixed value; `"open"` without a `rawValue` means the user picks from the just-added items.
   - `<child>.presence = "visible" | "hidden"` — a whole sub-block appears or disappears.
   - Setup lines at the top of the function (e.g. `code.access = "open"; code.clearItems(); code.addItem("");`) — the default state before branches run.
5. Note cross-branch reuse: the same child value can appear under multiple parent branches. Record each as a separate trigger condition — they get OR'd together later in the AEM visibility script.

### Phase 2 — Build the cascade table

Tabulate the discovery into the intermediate JSON shape below. This is what the emitter in Phase 8 consumes, and it's also the artifact you show the user to confirm before generating any XML.

```json
{
  "trigger_field": "DD_Bereich_...",
  "child_field_template": "DD_Kundensegment_{group}",
  "grandchild_field_template": "DD_Code_{leaf}",
  "cascade": [
    {
      "parent_values": ["Markt", "UHNW/GIAM", "Access"],
      "child_group": "Markt",
      "child_options": [
        {"label": "Core Affluent (Cora)", "leaf_group": "cora"},
        {"label": "Mitarbeiter", "leaf_group": "mitarbeiter"}
      ]
    }
  ],
  "leaves": {
    "cora": {"options": ["541   CORA INLAND", "543   CORA INLAND FAMILIE", "546   CORA AUSLAND", "548   CORA AUSLAND FAMILIE"], "locked": false},
    "mitarbeiter": {"options": ["184   Mitarbeiter-Mietavale"], "locked": true}
  }
}
```

**Never invent labels, values, or codes.** Every entry must be quoted verbatim from the XFA and traceable back to a specific `addItem`/`rawValue` line — if you can't point to the source line, don't add the entry. If the cascade has only 2 levels, drop the grandchild layer; if it has 4+, extend the same shape recursively.

### Phase 3 — Choose the AEM implementation pattern

AEM Adaptive Forms supports two rule shapes for `fd:scripts fd:visible="[...]"`, but only one is reliable here:

- **Pattern A — SCRIPTMODEL, visibility-only.** A single-element array: `{"script":{"field":"…","event":"Visibility","model":{"nodeName":"SHOW_EXPRESSION"},"content":"…JS…"},"nodeName":"SCRIPTMODEL","version":1,"enabled":true}`. Confirmed working (e.g. in `AACF_019_SP`) for panels/fields gated on another field's value.
- **Pattern B — full rule-tree**, needed for genuine `Value Commit`/`change` event rules: `{nodeName:"ROOT", items:[{nodeName:"STATEMENT", …}], script:"…", eventName:"Value Commit"}`.

**Use Pattern A, visibility-only, for the whole cascade.** Pattern A with `event: "Value Commit"` validates but does not fire at runtime, and dynamically rewriting a dropdown's `.enum` from a change handler is equally unreliable. Don't attempt either — go straight to the static-variant design in Phase 4.

### Phase 4 — Design the static multi-variant form

Since the option list can't change at runtime, model the cascade as N independent static dropdowns, one per parent-value combination, each shown/hidden by a visibility script:

- **Level 2 (child):** one dropdown per group of parent values that share the same option list. Visibility gates on `parent.value == "X" || parent.value == "Y" || …`.
- **Level 3 (grandchild), if present:** one dropdown per unique grandchild option set / (Level-1, Level-2) combination. Visibility gates on **both** the Level-1 condition **and** the Level-2 condition — include the Level-1 check even though Level-2 is only visible when Level-1 matches, otherwise a stale Level-2 value left over from a previous Level-1 selection can wrongly keep Level-3 visible.
- **Fixed-value branches** (single option) still get their own 1-option dropdown — same pattern, different arity.
- **Multi-pick branches** get an N-option dropdown with all N options, visible only in that state.

Every visible field ends up intrinsically restricted to the correct option set with no runtime script needing to fire. It's verbose (AAIU produced 7 Kundensegment variants and 14 Code variants from a 3-level cascade) but each variant is trivially reviewable.

Set `visible="{Boolean}false"` as a plain attribute on every variant so it starts hidden; the visibility script shows it when its condition holds.

### Phase 5 — Find the precedent

Before writing any XML, find a canonical example of the exact rule shape in the profile's reference forms — hand-copying a shape that's already known to work in this profile is what makes Phase 7's encoding tractable.

1. `grep_references` for the literal attribute you plan to use (`fd:visible`, `eventName`, …) to see who uses it.
2. `get_reference_package` for the best hit's ref_id, then `read_reference_file` on its `_jcr_content/guideContainer/.../.content.xml`.
3. Delegate the reading to a subagent with an explicit "quote verbatim, don't summarise" instruction — these files are huge and the escaping in them is exactly what you need to preserve.

For visibility scripts, `AACF_019_SP` is the known-good reference. Copy its JSON shape exactly and only vary `field`, `content`, and the outer wrapper. Its visibility scripts call both `window.forms.ubs.showAFShowDor(this)` and `window.forms.ubs.hideAFHideDor(this)` — keep calling both so the runtime UI and the Document-of-Record PDF stay in sync. See [references/visibility-encoding.md](references/visibility-encoding.md) for the full shape and encoding detail.

### Phase 6 — Encode the attribute correctly

Most failed attempts fail here. The `fd:visible` JSON traverses three encodings before AEM parses it: XML attribute unescape, then FileVault multi-value unescape, then JSON parse. When emitting the attribute you apply the inverse, in this exact order:

1. Build the JS content, `json.dumps(...)` it (produces `\"` and `\n`).
2. FileVault-double every backslash: `replace("\\", "\\\\")`.
3. FileVault-escape every comma (JSON commas and any comma inside an option label): `replace(",", "\\,")`.
4. XML-escape `&`, `<`, `>`, `"`.

Full worked example and the validator's tell-tale error message are in [references/visibility-encoding.md](references/visibility-encoding.md) — read it before hand-deriving the escaping, it's easy to get the order wrong.

### Phase 7 — Generate with the Python emitter, don't hand-write

The same shape repeats N times with only the trigger condition changing — hand-writing 14+ dropdowns character-for-character is exactly where escaping errors creep in. Use [scripts/emit_cascade.py](scripts/emit_cascade.py) as the starting point: it implements the Phase 6 escaping pipeline and a dropdown XML fragment builder. Adapt its `build_visibility_js` template to match the exact precedent shape found in Phase 5 before generating the full set, then feed it the Phase 2 cascade table to emit every variant deterministically.

### Phase 8 — Splice into the AEM tree

Use only the structure-aware granular editors — never `set_aem` or `set_structured`, which re-emit large subtrees and would undo other work already in the package:

- `get_aem_xml_outline` — map current node paths (re-read fresh, see Phase 9).
- `get_aem_xml_node` — inspect the placeholder field's current `sling:resourceType`, `css`, `dorFieldStyling`, `guideNodeClass`, `textIsRich` conventions before replacing it; new variants must match the profile's expected shape.
- `remove_aem_xml_node` — delete the placeholder empty dropdown from the initial conversion.
- `insert_aem_xml_node` — add each variant; use `position: "last"` unless ordering is required (see Phase 9).
- `replace_aem_xml_node` — for in-place fixes to a single variant, sparingly.

### Phase 9 — Validate

After every batch of edits: `build_aem_package` then `validate_aem_package`. Don't ship an invalid package.

- If validation reports "script payload does not parse as JSON after reversing FileVault escaping" with a column number, you're missing a `\\` before a `&quot;` or before a `\n` — go back to Phase 6, not the JSON structure.
- **Stop at 3 validate failures.** If the validator keeps rejecting after three fix attempts, stop and inspect: the escaping is almost always wrong at the source (a stray unescaped quote inside a code label), not the tool call. Report the unresolved issue rather than looping indefinitely.

If an AEM connection is configured and the user asked to deploy: `upload_to_aem`, then `fetch_aem_form_html` to confirm the cascade behaves correctly live.

### Phase 10 — Ergonomic notes

- **Node names and field names regenerate every conversion run** (UUIDs, `DD_Bereich_8b487b50` vs `DD_Bereich_bead1882`). Always re-read `get_aem_xml_outline` after `start_conversion` before splicing — never reuse names from a previous run, and build your visibility scripts' field references from the outline you just read.
- **Language preservation.** Every language in `get_source_info` must survive to the final package. New variant dropdowns inherit their label from `jcr:title` (plain string, master language); if the source is multilingual, extend the pattern to write per-language labels into `assets/dictionary/<lang>.xml` for each variant.
- **Ordering.** If a variant needs to sit right after a specific sibling rather than at the end, try `position: {"after": "..."}` on `insert_aem_xml_node`; some harnesses only accept string `"first"/"last"` from this call shape, in which case either reorder with `set_aem_xml_attribute` on siblings or accept `"last"` and flag the manual fix to the user.

### Phase 11 — Iterate

Ask: **"¿Algún otro cambio en la cascada, o en otro dropdown?"**

If yes and it references fields already read this session, skip back to Phase 2 directly — no need to re-run Phase 1 discovery. If it's a genuinely new field, restart at Phase 1.

When done, call `mcp__blueprint__finish` with a one-sentence summary of the cascade implemented (trigger field, number of variants, levels).

## Hard rules

- Never invent labels, option values, or codes — every one must be quoted verbatim from the XFA and traceable to a specific `addItem`/`rawValue` line.
- Never implement a cascade with `fd:valueCommit`, a `Value Commit`/`change` event rule, or runtime `.enum` mutation — it validates but does not fire. Visibility-only static variants are the only proven pattern in this profile.
- Never re-emit the whole AEM tree — granular XML editors only (`get_aem_xml_node`, `insert_aem_xml_node`, `remove_aem_xml_node`, `replace_aem_xml_node`, `set_aem_xml_attribute`).
- Never hard-code a node or field name from a previous run — re-read `get_aem_xml_outline` after every `start_conversion`.
- Never drop or invent a language when writing per-language labels.
- Stop after 3 consecutive `validate_aem_package` failures and report the unresolved issue instead of looping.
- Never call `finish` while `validate_aem_package` reports structural errors, unless the 3-failure stop was triggered — in which case state the unresolved issue in the summary.
