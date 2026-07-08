# Visibility script shape and attribute encoding

Read this when you're in Phase 5 (precedent hunt) or Phase 6 (encoding) of the
cascading-dropdowns skill and need the exact shapes rather than the summary.

## Pattern A JSON shape (visibility-only, the one to use)

```json
[
  {
    "script": {
      "field": "guideNodePath/to/DD_Kundensegment_markt",
      "event": "Visibility",
      "model": {"nodeName": "SHOW_EXPRESSION"},
      "content": "<JS shown below>"
    },
    "nodeName": "SCRIPTMODEL",
    "version": 1,
    "enabled": true
  }
]
```

This whole array is what ends up, fully escaped, inside `fd:visible="..."` on
the `<fd:scripts>` node.

## Visibility JS content — confirm against the precedent first

The known-good reference is `AACF_019_SP`. Its visibility scripts call both
helpers so the runtime UI and the Document-of-Record PDF stay in sync:

```js
if (/* condition over the trigger field(s) */) {
  window.forms.ubs.showAFShowDor(this);
} else {
  window.forms.ubs.hideAFHideDor(this);
}
```

The exact way fields are referenced inside that condition (dot path, `$field`
helper, `guideBridge` lookup, etc.) varies by profile — **read the actual
`AACF_019_SP` (or equivalent) `.content.xml` via `get_reference_package` +
`read_reference_file` before generating anything**, and copy its field-
reference idiom verbatim. Don't guess the API from this doc; it only tells you
which helpers must be called and in which branch.

For a two-condition cascade gate (Level-1 AND Level-2), AND the two conditions
in the same `if`:

```js
if (
  (parent1 == "Markt" || parent1 == "UHNW/GIAM" || parent1 == "Access") &&
  parent2 == "Core Affluent (Cora)"
) {
  window.forms.ubs.showAFShowDor(this);
} else {
  window.forms.ubs.hideAFHideDor(this);
}
```

Always include the Level-1 condition even when Level-2 can only hold that
value while Level-1 matches — a stale Level-2 value from a prior Level-1
selection must not keep this field visible.

## The three encoding layers

AEM peels these off in order when it parses the package, so you must apply
them in reverse when you emit the attribute:

| # | Layer | What it does | Unescape (AEM does this) |
|---|-------|---------------|---------------------------|
| 1 | XML attribute | `&quot;` → `"`, `&amp;` → `&`, `&lt;`/`&gt;` → `<`/`>` | outermost |
| 2 | FileVault multi-value | `\,` → `,`, `\\` → `\`, any other `\x` → `x` | middle |
| 3 | JSON parse | standard JSON string parsing | innermost — the actual JS content |

To emit the attribute, apply the inverse in this exact order:

```
1. content        = the JS string (Pattern A "content" field)
2. json_string     = json.dumps(full_pattern_a_array)   # produces \" and \n
3. fv_escaped      = json_string.replace("\\", "\\\\")   # double every backslash FIRST
                                 .replace(",", "\\,")    # then escape every comma
4. xml_escaped     = fv_escaped.replace("&", "&amp;")
                                .replace("<", "&lt;")
                                .replace(">", "&gt;")
                                .replace('"', "&quot;")
```

Order matters: escaping commas before doubling backslashes would double the
backslash you just inserted for the comma. `emit_cascade.py` in `scripts/`
implements this in `escape_visibility_attribute()` — use it rather than
re-deriving it by hand.

### Worked example

JS content fragment: `if (a == "Markt") { ... }`

1. After `json.dumps`, the embedded quote becomes `\"`: `...a == \"Markt\"...`
2. FileVault backslash-doubling: `\"` → `\\"`... concretely the two chars `\`
   `"` become three chars `\` `\` `"` i.e. `\\\"` in the raw string sense —
   the point is every literal backslash from step 1 gets a second one in
   front of it.
3. FileVault comma-escaping: any `,` in the JSON (structural commas between
   object keys, or commas inside option labels) becomes `\,`.
4. XML-escaping: the `"` characters (now preceded by `\\` from step 2) become
   `&quot;`, so the final attribute source contains `\\&quot;` where the
   original JS had a single `"`.

### Validator round-trip

`validate_aem_package` reports:

> script payload does not parse as JSON after reversing FileVault escaping

with a column number, when the escaping is off. Seeing this means you're
missing a `\\` before a `&quot;` or before a `\n` — go fix the emitter's
escaping order, don't try to patch the JSON structure itself.
