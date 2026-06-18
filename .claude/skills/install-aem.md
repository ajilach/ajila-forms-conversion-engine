# /install — AEM Package Install & Fix Loop

Install a converted AEM FileVault package, verify it loads, iterate on fixes with the user.

**Usage:** `/install path/to/form_merged.zip [-- additional notes]`

Additional notes after the path are carried into the fix loop as extra context.

---

## Steps

### 0. Load credentials

Source the `.env` file at the repo root to populate `AEM_URL`, `AEM_USER`, `AEM_PASSWORD`:

```bash
set -a && source .env && set +a
```

### 1. Read package metadata

Extract the package name and form JCR path directly from the ZIP:

```bash
unzip -p <zip> META-INF/vault/properties.xml
unzip -p <zip> META-INF/vault/filter.xml
```

- **Package name** — the `<entry key="name">` value in `properties.xml` (e.g. `AAMQ`)
- **Form JCR path** — the first `filter root=` value under `/content/forms/` in `filter.xml`
  (e.g. `/content/forms/af/afforms_germany_all/af_aa/AF_AAMQ`)

### 2. Upload and install

```bash
curl -u "$AEM_USER:$AEM_PASSWORD" \
  -F file=@"<zip>" \
  -F name="<name>" \
  -F force=true \
  -F install=true \
  "$AEM_URL/crx/packmgr/service.jsp"
```

Parse the response XML. If the `<status code=` is not `200` or the response contains an error, **stop and report the error** — do not proceed.

### 3. Verify form loads

```bash
curl -s -o /dev/null -w "%{http_code}" \
  -u "$AEM_USER:$AEM_PASSWORD" \
  "$AEM_URL<form_jcr_path>.html"
```

Note: the bare path redirects (302); append `.html` to get a direct 200.

Note the HTTP status.

### 4. JCR API inspection

Fetch the full form component tree from AEM and run the inspector script:

```bash
set -a && source .env && set +a
curl -s -u "$AEM_USER:$AEM_PASSWORD" \
  "$AEM_URL<form_jcr_path>/jcr:content/guideContainer/rootPanel/items.tidy.6.json" \
  | python3 .claude/scripts/aem_inspect.py
```

Where `<form_jcr_path>` is the JCR path from Step 1 (e.g. `/content/forms/af/afforms_germany_all/af_aa/AF_AAMQ`).

The script reports:
- All wizard panels and their input fields (type, label, name) and section titles (titledraw)
- Fragment assignments (`FRAGMENT` lines) — verify each matches what the PDF shows
- Flagged issues:
  - `!! STRAY TEXT` — textdraw node with a non-empty value. Cross-check against the PDF render before deleting: if the text matches visible paragraph content or a section label in the PDF, it is legitimate static content — leave it. Only delete textdraws whose content has no counterpart in the PDF (internal codes, orphaned artefacts).
  - `!! MISSING LABEL` — input field with no `jcr:title`
  - `!! DUPLICATE NAME` — two fields share the same `name` attribute

Always paste the **full script output** in the report to the user (panels, fields, titles, fragments, and issues section).

**Note:** For rare visual rendering questions (layout, spacing) you can still open the preview URL in a browser and use screencapture as a fallback. The DAM preview URL format is:
```
$AEM_URL/content/dam/formsanddocuments/<entity>/<prefix>/<form>/jcr:content?wcmmode=disabled&afAcceptLang=<lang>
```

### 5. Report to user

Tell the user:
- Package installed: `<name>` (group: `fd/export`)
- Form JCR path: `<form_jcr_path>` — HTTP `<status>`
- **Inspection findings** — paste the script output (panels + fields + any flagged issues)
- Any items that could not be fixed during conversion (carry them forward from the /convert report if available)

Then ask: **"Anything else you noticed? Should I go ahead and fix the issues above?"**

---

## Fix loop

Repeat the following whenever the user reports issues:

### 5a. Apply fixes

Use the same fix approach as `/convert`:

- **JSON-fixable** (field type, label, options, required): Edit `<name>_merged.json`
- **XML-only** (readOnly, AEM component attributes, fragment corrections, stray static text nodes): unzip → edit XML files → rezip

```bash
# XML patch cycle
unzip -o <zip> -d _pkg_tmp
# Edit XML files in _pkg_tmp/jcr_root/ with the Edit tool
cd _pkg_tmp && zip -r ../<zip> . && cd .. && rm -rf _pkg_tmp
```

### 5b. Rebuild from JSON (if JSON was edited)

```bash
rm -f <zip>
cargo run --release -p blueprint-cli -- --from-structured <name>_merged.json --aem --profile ubs
```

Skip this if you used the XML patch cycle (ZIP is already up to date).

### 5c. Delete old package from AEM

```bash
curl -u "$AEM_USER:$AEM_PASSWORD" -X POST \
  "$AEM_URL/crx/packmgr/service/.json/etc/packages/fd/export/<name>.zip?cmd=delete"
```

### 5d. Upload and install new package

Same as Step 2.

### 5e. Re-inspect and report

Re-run the JCR inspection after re-install to confirm fixes:

```bash
set -a && source .env && set +a
curl -s -u "$AEM_USER:$AEM_PASSWORD" \
  "$AEM_URL<form_jcr_path>/jcr:content/guideContainer/rootPanel/items.tidy.6.json" \
  | python3 .claude/scripts/aem_inspect.py
```

Report:
- What was fixed (before/after comparison of flagged issues)
- Any remaining flags

Ask: **"Anything else to fix?"**

---

## Notes

- AEM credentials come from `.env` at the repo root (`AEM_URL`, `AEM_USER`, `AEM_PASSWORD`)
- Package group is always `fd/export` for this profile; package name matches the form code
- The fix loop continues until the user is satisfied or remaining issues require engine-level changes
- Items that cannot be fixed at JSON or XML level should be reported clearly as requiring either hands-on AEM Studio work or an engine fix
- If a label or translation is missing inside a fragment component, leave it — fragment content is managed separately and cannot be patched via the package

## XML fix patterns

**Making a field read-only** — add `readOnly="{Boolean}true"` to the component node. Note: visually the field may look the same as editable fields; the constraint is enforced at the model level.

**Removing a stray static text node** — delete the entire `<textdraw_...>` element including its children. These are `sling:resourceType=".../textdraw"` nodes with `_value="<p>...</p>"`.

**AEM JCR boolean syntax** — always `{Boolean}true` / `{Boolean}false`, never plain `true`.

**Removing any field — always check layout** — fields sit in a 12-column responsive grid (`cq:responsive/default width`). When removing a field, check whether its row-neighbours had their width split with it (e.g. both `width="6"` for two-per-row). If so, expand the remaining neighbour to fill the vacated space (e.g. `width="12"`). Fix the layout in the same XML patch as the removal.
