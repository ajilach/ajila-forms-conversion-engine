# Resolved Feedback Patterns

Append new entries as feedback is processed. Manager is the sole writer — Workers read a snapshot.

## Entry format

Use a **slug ID** (not a global number) so entries appended on different fix
branches don't collide at merge.

```markdown
## FEEDBACK-<short-slug> — <short title>
**Feedback pattern:** "..." (what the feedback typically says)
**Root cause:** ...
**Fix applied:** ...
**First seen:** <form>
**Seen again:** <forms>
```

## FEEDBACK-ubs-europe-se-rendered-by-the-banking-fragment-do-not-re-author — 'UBS Europe SE missing' is usually STALE — the fragment renders it automatically
**Feedback pattern:** "UBS Europe text is missing under the banking relationship section."
**Root cause:** This is almost always STALE feedback already fixed by sweep #10 (PROBLEM-banking-relationship-fragment). The **canonical** banking-relationship fragment (`affrg_BankingRelationship1`) renders the "UBS Europe SE" line **automatically** — there is no separate authored draw for it, and there must not be. Adding a standalone `ST_UbsEuropeSe_*` textdraw creates a **duplicate** (the exact defect the sweep-#10 review note warns about) and, if placed inside the `PN_BR` exclusion wrapper, also breaks the banking-relationship-wrap invariant (that wrapper must hold the fragment as its SOLE child).
**Fix applied:** Verify the form already carries the canonical `affrg_BankingRelationship1` fragRef (sweep #10 detector clean) → the item is stale, make **no** authored change; the fragment renders the text. Do NOT add a `ST_UbsEuropeSe_*` draw. (`ST_IWeAuthorizeUbsEuropeSeUbsTo_*` is a different, legitimate authorization sentence — leave it alone.)
**First seen:** AAGO_019 (initially mis-fixed by authoring a duplicate draw; corrected to no-op after owner confirmed the fragment renders it)

## FEEDBACK-remove-the-preview-step-without-losing-dor-metadata-config — Remove the preview step without losing DoR/metadata config
**Feedback pattern:** 'Remove the preview section' feedback — the previewpanel hosts the invisible doroptionsubs + metadata components.
**Root cause:** Preview step is optional (120/286 packages lack it) but carries mandatory config nodes.
**Fix applied:** Move doroptionsubs + metadata verbatim into summaryPanel/items after submitErrorMessage, then delete previewpanel and the toolbar 'preview' tertiarybutton (its rules reference the carousel; 146/168 no-preview forms also carry no toolbar preview button — refs AAAE/AAAN/AACR/AABA).
**First seen:** AAGO_019

**Seen again:** AABJ_019
## FEEDBACK-english-shows-in-the-german-version-despite-a-populated-dictionary — English shows in the German version despite a populated dictionary
**Feedback pattern:** 'EN text in DE version' feedback with a populated de.xml present.
**Root cause:** Individual dictionary entries are untranslated stubs (message == English key), or the key mismatches the authored draw text by a trailing &#xa; — AEM falls back to the English source per string.
**Fix applied:** Diff every visible _value/jcr:title against fd_-prefixed keys in assets/dictionary/de.xml (a separate ZIP entry — extract/repack it alongside the form XML); fix messages / add exact-match keys, translations verbatim from the form's <LANG> PDF XFA or the corpus-majority message. Strings identical in the PDF's German are correct untranslated.
**First seen:** AAGO_019

**Seen again:** AABJ_019
## FEEDBACK-two-legal-guardian-signature-blocks-for-minderjaehrige — Two legal-guardian signature blocks for Minderjaehrige
**Feedback pattern:** Only one signature block shows when 'Minderjaehrige' is selected (RB_FormularAdressat == "2").
**Root cause:** TWO things must be right, and `minOccur` is the *lesser* one. (1) The `affrg_LegalGuardianSignature1` fragment panel (name `PN_SignatureIndividual`) must have `minOccur=2` (maxOccur 2 or 4 — corpus varies). (2) **The decisive one:** the conditional show/hide CONTAINER for that signature section must drive its visibility through the legacy UBS hook — an `fd:scripts` `fd:visible` calling `window.forms.ubs.showAFShowDor(this)` / `hideAFHideDor(this)` — **not** a stock machine-generated `fd:rules` Hide/Show. In this legacy runtime the initial `minOccur` instances are *materialised by the `showAFShowDor` init hook*, not by stock AF's min-occur engine, so a form re-converted with a plain `fd:rules` visibility (empty `fd:scripts`) sets `minOccur=2` correctly yet still renders ONE block — the hook never fires. **General lesson: a repeatable panel that "won't repeat" in this corpus is a VISIBILITY-MECHANISM problem, not an occur-value problem.**
**Fix applied:** Set `minOccur=2` on the panel (necessary but NOT sufficient), AND on each conditional signature container replace the machine-generated `fd:rules` `fd:visible` with the reference-style `fd:scripts` `fd:visible` `showAFShowDor`/`hideAFHideDor` hook (preserve the guard, e.g. `RB_FormularAdressat.value=="2"`). Copy the hook VERBATIM from a byte-identical known-good form (AABO_019 / AAGD_019 / AAGZ_019) — do not author the script (only adapt the host-node `field` path to the form's own signature-parent node). Verify structurally (hook present, machine-gen rule gone on every signature container); the rendered 2-block behavior is a HUMAN check — a static GET can't exercise the radio.
**First seen:** AAGO_019 (initially mis-fixed with `minOccur=2` only, which did **not** render two blocks; corrected by restoring the `showAFShowDor` visibility hook copied from AABO_019)

## FEEDBACK-standard-address-fragment-is-regional — 'Use the standard address fragment' → the region-specific AddressBlock fragment
**Feedback pattern:** "Use the standard address fragment for the address block (Straße / Nr. / PLZ / Stadt / Land)."
**Root cause:** The form has hand-built inline address fields (or the engine's inline `TXT_Street`/`TXT_No`/`TXT_PostalCodeCity`/`TXT_Country`) instead of the corpus-standard address fragment. NB: the **engine emits inline fields and never assigns an address fragment**, and the 18-form blueprint reference library is a narrow subset — "no reference uses it" is NOT evidence a fragment is non-standard. Confirm against the FULL corpus of deployed ZIPs (grep every `forms/issued/*/_merged.zip`).
**Fix applied:** Replace the inline address panel with one `fd/af/components/panel` fragRef to the REGION-appropriate fragment: entity 019 (Germany) → `affrg_germany_AddressBlock_CountryDD` (11 forms); entity 033 (Italy) → `affrg_italy_AddressBlock_CountryDD` (6 forms); generic variants `affrg_AddressGeneric1` (31) / `affrg_Address1` (16) also exist corpus-wide. The fragment is OPAQUE — empty `<items>` in the JCR, fields resolve at render — so never edit inside it. It renders Country as a **dropdown** and includes an optional "Additional address details" (Adresszusatz) line the source PDF may lack; that extra line is STANDARD (all 11 germany-CountryDD forms carry it, none hide it) — leave it, do not author an unprecedented form-level hide.
**First seen:** AAGO_019 (item #5 — `affrg_germany_AddressBlock_CountryDD`, confirmed standard across 11 sibling 019 forms)
