# Consistent Problems

Systemic defects that affect **many forms in the same way** — a wrong fragment
used everywhere, an old-engine bug since fixed, a layout convention that changed.
Distinct from [resolved.md](resolved.md), which records one-off, per-feedback-item
resolutions.

Each entry is **data** the `/sweep` engine executes — symptom, how to find
affected forms, how to fix one. The engine is problem-agnostic: it dispatches on
the `Detect` / `Fix` verbs below and has no per-problem logic. A new entry is
**code-free only when it can reuse an existing `Detect`/`Fix` script**; a new
shape (a structural rewrite, an attribute target no regex matches) needs a new,
committed, reviewed `find_`/`set_` script first. See `.claude/skills/sweep/SKILL.md`
→ "Record a new problem".

**Establishing "correct":** a sweep needs a ground-truth reference. The conversion
engine is **not** authoritative by default (it has known faults) — rely on it only
when the user says it's right for this problem; otherwise ask for a reference.

## Entry format

```markdown
## PROBLEM-<slug> — <title>
**Symptom:** what's wrong, and where it shows up in the form .content.xml
**Detect:** grep: <regex>            # how to identify an affected form's deployed XML
            # or → script: <name> <args>   (when a regex can't express it)
**Fix:**    manual: <AI procedure>   # how to fix ONE form
            # or → script: <name> <args>   (deterministic, worth scripting)
**Verify:** must: <regex> ; must-not: <regex>   # optional auto-check (check_form.py) after the fix
**Max changed lines:** <N>                       # optional blast-radius guard (form_diff.py): flag forms whose fix touched more than N lines. Attribute-level fixes only — for additive/structural fixes set it to the expected per-instance insert size, or omit and rely on Verify / the fix's `ok`.
**Origin:** feedback-recurrence | engine-diff | manual
**Tracking:** #<n>                   # the consistent-problem GitHub issue
**Status:** open | piloted <YYYY-MM-DD> (N/TOTAL) | swept <YYYY-MM-DD>
```

**Verbs**
- `Detect: grep: <regex>` — the default; matched against each form's
  `.content.xml` by `find_affected.py`.
- `Fix: manual: <procedure>` — the default; the sweep agent edits each form's XML
  with judgment (extract → edit → repack), same discipline as the feedback Worker.
- `Fix: script: <name> <args>` — a deterministic fix. The script takes the form's
  ZIP, transforms its `.content.xml` in place, and prints a JSON report including a
  uniform **`ok`** boolean (form is now in the target state) that `run_sweep`'s
  verify consumes. Existing fixers to reuse: `swap_fragment` (fragRef value),
  `remove_attr` (drop an attribute), `set_attr_on` (ensure an attribute on
  selector-matched elements, quote-aware), `set_fragment_panel` (fragRef + panel
  attributes), `set_dor_template` (entity-aware DoR config). A new *shape* (e.g. a
  structural rewrite that synthesises nested nodes) needs a new such script.
- The whole staged run is executed by `run_sweep.py --slug PROBLEM-<x>
  [--pilot|--continue|--only]`, which reads this entry and does detect → fix →
  delta → diff → verify → deploy → issue row → one PR.

---

<!-- Problems are appended below this line. -->

## PROBLEM-banking-relationship-fragment — Banking-relationship panel: correct fragment + DoR exclusion
**Symptom:** the banking-relationship panel (the one bearing the BankingRelationship `fragRef`) needs two things the conversion engine emits: (1) the **one correct fragment** — `affrg_BankingRelationship1` (wrong forms use germany/italy/global variants or a dam-path reference; the correct one shows "UBS Europe SE" under Bankbeziehung), and (2) **`dorExclusion="true"`** so the whole panel is excluded from the Document of Record. The fix sets both on that panel; everything else on it (resourceType, name, dorExcludeTitle/Description) is left as-is.
**Detect:** script: `find_panel_noncanonical.py --match-frag BankingRelationship --exclude-frag CustodyAccount --require-frag /content/forms/af/afforms_ubs_fragmentlib/affrg_BankingRelationship1 --require-attr dorExclusion=true`  _(a form is affected if its banking panel lacks the canonical fragRef or `dorExclusion="true"`)_
**Fix:** script: `set_fragment_panel.py --match-frag BankingRelationship --exclude-frag CustodyAccount --set-frag /content/forms/af/afforms_ubs_fragmentlib/affrg_BankingRelationship1 --set-attr dorExclusion=true`
**Verify:** must: `fragRef="/content/forms/af/afforms_ubs_fragmentlib/affrg_BankingRelationship1"` ; must-not: `affrg_(germany|italiy|global)_BankingRelationship`  _(the per-panel `dorExclusion` is asserted by the fix's `all_canonical:true`, since a file-level grep can't tell which panel carries it)_
**Max changed lines:** 3   _(fragRef swap = 1 changed line, `dorExclusion` insert = +1 → up to 3 diff lines for a panel missing both; flag anything more, incl. the one multi-panel form)_
**Origin:** feedback-recurrence
**Tracking:** #10
**Status:** swept 2026-06-24 (212/212 fixed + deployed; 5 flagged diff>3, all verified benign — 4 had `dorExclusion="false"`→`true` plus the fragRef swap, AANB_019 has 3 banking panels). Open follow-up: whether the legacy custom UBS panel honors `dorExclusion` in the rendered DoR (the ~99 custom-panel forms may need the panel switched to the standard type for the exclusion to visually take effect; authored value is already correct).

**Scope note — summary exclusion deliberately NOT swept (2026-06-23):** the engine
*also* emits `summaryExclusion="true"` on this panel, but its summary-exclusion
output isn't correct yet, so we leave the deployed `summaryExclusion` exactly as-is
until the engine is fixed. This sweep enforces only the fragment + `dorExclusion`.

**Review note:** "UBS Europe SE" appears legitimately all over these forms
(signatures, legal text, document titles) — do **not** auto-strip it. After the
swap, the per-form render-verify must inspect the deployed form for (a) a
**duplicate** "UBS Europe SE" under the banking section, and (b) **leftover inline
clearing/bank fields** (forms that had a custom-control banking panel). Flag ⚠️
for manual touch-up if either appears. `*_BankingRelationship_CustodyAccount` is a
different fragment — leave it alone (the detector/fix already exclude it).

## PROBLEM-dor-exclude-hidden — Uncheck "Exclude hidden fields from Document of Record"
**Symptom:** the DoR config (`dorProperties`) has `excludeFromDoRIfHidden="true"`, so hidden fields/sections are dropped from the Document of Record. It should be unchecked — the conversion engine omits the attribute entirely, and the 181 unchecked forms simply lack it.
**Detect:** grep: `excludeFromDoRIfHidden="true"`
**Fix:** script: `remove_attr.py --attr excludeFromDoRIfHidden`
**Verify:** must-not: `excludeFromDoRIfHidden=`
**Max changed lines:** 1   _(removing the attribute deletes exactly one line; flag any form whose diff touches more)_
**Origin:** feedback-recurrence
**Tracking:** #8
**Status:** swept 2026-06-23 (105/105, all checks passed)

## PROBLEM-jump-to-field-button — Jump-to-field button on the step-title panel + canonical title-node config
**Symptom:** the "Show jump to field button" option must be set on the **step-title panel** (`PN_FooTitle`) — in its **Summary** section — **not** on the title-draw. The engine wrongly emits `jumpToFieldButtonVisible="true"` on the `css="stepTitle"` title-draw (`panel.xml:55`), where it has no effect (**engine bug** — should move to the `panel_title` node). The old #12 sweep targeted the draw and was closed. The Summary section only exists on the UBS custom panel, so **[PROBLEM-panel-type-ubs](#) / #20 is a prerequisite** (merged 2026-07-01). Beyond jump-to-field this normalizes the title node to the shape the owner validated in AEM. FormConfigurator title panels keep their intentional DoR/summary exclusions (we only *add* on the panel, never strip).

Per step-title panel + its `headingLevel="2"` draw — **two cases**:

**Normal steps:**
- **panel** `PN_FooTitle`: add `jumpToFieldButtonVisible="true"` (791) + `dorExcludeDescription="true"`; **strip "rest to false"** — remove `summaryExclusion`/`dorExcludeTitle`/`dorExclusion` if present (7 panels).
- **draw** `TTL_Foo`: **remove** `jumpToFieldButtonVisible` (286) + add `summaryExclusion="true"` (210) + `dorExclusion="true"` (57).

**Form configurator** (the "Formular Adressat" config step — **not** real content; **only ever the first page**, so matched only on `step_idx == 0` to avoid false hits on later steps — detected by `formconfig`/`configurator` in the step / title-panel / **title-draw** name, or `css="ubs-margin-10"` on the step; **124 across the corpus**):
- **NO** `jumpToFieldButtonVisible` (config step gets no jump button — it would otherwise also show in the summary jump-list).
- **title panel** `PN_FooTitle` (the panel around the configurator title): add `dorExclusion="true"` (73) + `summaryExclusion="true"` (111) + `dorExcludeDescription="true"` → excluded from both the summary and the DoR. Never strip. (On the **panel**, like jump-to-field — not the step panel, not the draw.)
- **draw**: same as normal (strip jtf, add summaryExclusion + dorExclusion).
- First-page gate matters: without it, a later `PN_FormSection` (AAOF/AAOM/AAPR step 1) or a signature step with `css="ubs-margin-10"` (BAZA/BAZC step 4) would be misread as a configurator.

137 title nodes already correct; 4 steps have no title panel (skip). **251 forms affected.**
**Detect:** script: `find_jump_to_field.py`  _(per step-title panel/draw; emits `fix[]` {form,step,panel,deltas} + counts; affected = forms with any delta)_
**Fix:** script: `set_jump_to_field.py`  _(byte-offset, quote-aware open-tag scan for rich-text `_value`; adds the panel attrs, strips jtf from the draw, adds draw attrs; prints `{inner, changed:[…], ok}`)_
**Verify:** the fix script's `ok` (well-formed) + a re-scan showing 0 deltas remain + a pilot **visual spot-check** (jump-to-field button now renders on the step; title shows once).
**Max changed lines:** omit — mixed add/strip across two nodes per step; gate on the fix's `ok` + re-scan.
**Origin:** engine-diff (engine misplaces it on the draw; correct target is the panel)
**Tracking:** #22 (supersedes closed #12)
**Status:** **swept 2026-07-02 (254/254 fixed + deployed, PR #26 merged)** — jump-to-field on the step-title panel; canonical title-node config; form-configurator (first-page) gets no button + title panel excluded from summary+DoR. Engine still misplaces jtf on the draw (upstream fix pending).

## PROBLEM-dor-custom-template — DoR config: custom template (metaTemplateRef) + "Use Adaptive Form Title"
Two DoR-config fixes the engine emits, applied together (all **other** master-page properties are left exactly as-is, per request 2026-06-24):
1. **Custom template** — `metaTemplateRef` on `dorProperties` should be the entity-General template, not the generic Blank. Engine branches on entity (config.toml): 019 → `…/02_forms/UBS_General_Germany_DOR.xdp`, 033 → `UBS_General_Italy_DOR.xdp` (else, e.g. CH 001 → `UBS_Blank_DoR.xdp`, so Blank is correct there). 93 forms have Blank.
2. **Form Title = "Use Adaptive Form Title"** — the Header `AF_FORM_TITLE` node should be `valueFrom="formTitle"` with **no** `value` and **no** `fd:translationIds` (the engine's exact output). 31 forms deviate (29 hold a hardcoded `value` with `valueFrom="     "`, 2 use `valueFrom="template"`). The 44 forms with **no** `AF_FORM_TITLE` node (minimal master-page) are skipped — we don't synthesize the node ("leave properties as they are").

**Combined affected:** 121 forms (90 template-only, 28 form-title-only, 3 both; 165 already correct).
**Detect:** script: `find_dor_config.py` _(to build for --continue: affected if `metaTemplateRef` basename is `UBS_Blank_DoR.xdp` on a 019/033 form, OR `AF_FORM_TITLE` is present and not already `valueFrom="formTitle"` with no value/translationIds)_
**Fix:** script: `set_dor_template.py --replace-basename UBS_Blank_DoR.xdp --use-adaptive-form-title`  _(entity from JCR path; metaTemplateRef only changes Blank→General; `--use-adaptive-form-title` sets AF_FORM_TITLE valueFrom=formTitle and drops value+translationIds; no-ops anything already correct)_
**Verify:** must: `metaTemplateRef="[^"]*UBS_General_(Germany|Italy)_DOR\.xdp"` ; must-not: `UBS_Blank_DoR\.xdp` — **plus** the AF_FORM_TITLE node must be `valueFrom="formTitle"` with no value/translationIds (checked directly, since a file-level grep can't isolate one node). The fix reports `changed` + `form_title_changed`.
**Max changed lines:** 6   _(metaTemplate replace = 2; AF_FORM_TITLE: drop value + drop translationIds + valueFrom replace ≈ up to 4)_
**Origin:** engine-diff
**Tracking:** #14
**Status:** swept 2026-06-24 (121/121 — combined fix; AAXC_019 + BAAD_019 had only the Form Title set, their out-of-scope Custody/Blank-BankingRelationship templates correctly left untouched). Verify-check note: the `must=General` template assertion false-flags form-title-only forms whose template is a non-General variant — tighten to "template==General OR was not Blank" if this sweep is re-run.

**Scope note — only `UBS_Blank_DoR.xdp` (custom template) + the Form Title property are swept.** All other master-page properties (`ShowBankingRelationship`, `senderAddressTitle`, `APPCode`, structural nodes, `AF_HEADER_TEXT`, etc.) are **left untouched** per the 2026-06-24 decision. Other non-General templates (`UBS_Blank_Letter_DoR.xdp` 44, `UBS_Custody_<entity>` 8, `UBS_Blank_BankingRelationship` 2) are also left as-is.

**Rollout note — overlaps other sweeps.** ~90% of these 93 forms are also in the banking (#10) and jump-to-field (#12) affected sets — **0 are free of both**. Separate sweep branches that edit the same `_merged.zip` conflict at merge (binary/LFS, no 3-way merge). **Sequence the rollouts**: fully merge one sweep, rebase the others on the new master, then `--continue` (all three fixes are idempotent, so re-running on the merged base is safe). Pilots are chosen to avoid forms already committed to another sweep branch, so no conflict arises before rollout.

## PROBLEM-step-title-panel — Each wizard step's title wrapped in a step-title panel
**Symptom:** the engine wraps every wizard **step**'s title in a dedicated *step-title panel* (`guidePanel` named `{step}Title`, holding a `headingLevel="2"` title-draw) as the step's **first child** (engine `profiles/ubs/aem/panel.xml`; **engine confirmed authoritative for this problem** by the owner — *but only for the wrap structure; the engine does NOT reliably detect the title text, so title text comes from the deployed form, never the engine*). A *step* = a panel directly under `guideContainer › rootPanel(guideRootPanel) › items`, excluding the specials (`summary`/`preview`/`formmetadata`/`signerInfo`/…). Per-step verdict (the agreed rule — title text taken **verbatim** from the form, this sweep only changes *structure*):
- **ok** (609) — already has a dedicated step-title panel, **or a child draw already renders the title** (a `titledraw`/`textdraw` whose `_value` text *exactly* equals the step title) → **skip** (synthesizing would duplicate the visible heading). **Only a drawn `_value` counts as "rendered" — a panel's `jcr:title` does NOT render as a visible heading** (validated on AAAI `PN_Agreement`, AACB `PN_Parte1`, `PN_StpSectionIiSectionTitle` — all panels whose `jcr:title` matched the step title but showed no heading; those steps need synth, not skip).
- **wrap:move** (77) — a `headingLevel="2"` draw exists but isn't wrapped → **move that draw intact** to the top of the step and wrap it; remove the step's own `jcr:title`. Translation-safe (validated: AAAM move kept its German title).
- **wrap:synth** (274) — no draw renders the title → synthesize a draw from the step's `jcr:title` at the top; remove the step's `jcr:title`. Covers steps whose title only lived in a panel `jcr:title` (AAAI, AACB sections, AAAL — all validated single after).
- **strip:double** (26) — already wrapped (a `headingLevel="2"` draw whose text *equals* the step's own `jcr:title`) **and** the step still carries that `jcr:title` → AEM renders the heading **twice on screen** (`dorExcludeTitle` only hides it in the DoR/PDF, not the form) → **strip the redundant step `jcr:title`** (no insert). Validated: AACB first two showed twice, single after.
- **wrap:promote** (45) — no h2 draw and no step `jcr:title`, **but the step LEADS with a `titledraw`** (its real title rendered at the wrong heading level: `h3`/`h4`/none) → promote that draw to a proper step title: set `headingLevel="2"` + `css="stepTitle"`. **If it's already inside a dedicated step-title panel** (17 of 45) → promote **in place** (no new panel — else double-wrap); **otherwise** (28 of 45) → move it intact to the top and wrap it in `{name}Title`. A leading *textdraw* (body text) or no draw → stays titleless. Validated: AACT (in-place, single panel), AAEU "Checkliste"/AAFA "Finanzdienstleister" (move).
- **flag:trans** (7) — `jcr:title` is translation-linked (path-keyed) and no draw renders it → auto-wrap + flag "update translations".
- **titleless** (31) — no title anywhere (no h2 draw, no `jcr:title`, and the leading drawn element is a textdraw/body or absent) → **skip**.

Rules that made the fix safe (all from AEM validation): **remove the step's own `jcr:title`** when wrapping (else AEM renders it a second time), **a panel `jcr:title` never renders a visible heading** (so only a matching drawn `_value` blocks a synth — everything else gets a proper title), and **strip the step `jcr:title` when a wrapped draw already shows the same text** (`strip:double`). Translations are text-keyed (`dictionary/<lang>.xml`, `sling:key="fd_<text>"`), so keeping title text verbatim preserves them.

Detection keys on a `headingLevel="2"` title-draw, **not** `css="stepTitle"` (that marker also sits on wrapper panels / `textdraw`s and is absent on ~415 — it both over- and under-matches). **182 forms / 422 auto-fixes (77 move + 274 synth + 26 strip + 45 promote) / 7 flags** of 286 packages.
**Detect:** script: `find_step_title_panel.py`  _(emits `wrap[]` {form,step,source:move|synth|strip|promote,title} and `flag[]` {…,reason}; that is the review surface — verify it before building/running the fix)_
**Fix:** script: `set_step_title_panel.py` _(byte-offset structural surgery: insert a `{name}Title` panel as the step's first child; `move` = relocate the existing draw intact (preserves translations), `synth` = build a draw from `jcr:title`, `strip` = remove the redundant step `jcr:title` (no insert), `promote` = a leading titledraw at the wrong heading level → set `headingLevel=2`+`css=stepTitle` (in place if already in a title panel, else move+wrap); all also remove the step's own `jcr:title` and set `dorExcludeTitle="true"` to match the engine. Auto-fixes `move`+`synth`+`strip`+`promote`+`flag:trans`; skips `ok` and `titleless`. Mirrors the detector's skip rules. Prints `{inner, wrapped:[{step,source}], ok}`.)_
**Verify:** the fix script's per-step `ok` (well-formed result + every targeted step now has a first-child step-title panel, or had its redundant title stripped) + a pilot **visual spot-check** (title shows once, correctly). The 2 `flag:trans` steps are auto-wrapped too but **listed in the issue table as "update translations"** — the owner tops up their translations after rollout.
**Max changed lines:** omit — additive structural insert; gate on removed==0 (move adds/relocates, never deletes content) + the fix's per-step `ok`.
**Origin:** manual (engine-confirmed for structure only)
**Tracking:** #18
**Status:** **swept 2026-06-30 (182/182 fixed + deployed, PR #19 merged).** Detector + fixer validated in AEM on AAAM (move, translation held), AAAN (synth), AACB (strip:double + 26 section synths), AAAI/AAAL (synth, no double), AACT/AAEU/AAFA (promote). Three rule corrections during the pilot: added `strip:double` (AACB's first two doubled); tightened `renders_title_elsewhere` to **draw `_value` only** (a panel `jcr:title` doesn't render → titles living only in a panel `jcr:title` are now synthed); added `wrap:promote` (titleless step leading with a titledraw at the wrong heading level → promote to h2, in place if already in a title panel else move+wrap). The 7 `flag:trans` steps (4 forms) were resolved **not-actionable** — all are `jcr:language="en"` with orphaned dictionaries (no live second locale), so the English synth titles are correct. 31 steps remain genuinely titleless. One edge: `contentPanel_forInternalBankUseOnly` (1 form) is a direct root child that looks like content, not a step.

**Dependency:** this is the **prerequisite** the jump-to-field sweep (#12) is deferred behind — jump-to-field's button goes on the step-title draw, which must first be correctly wrapped. **Land #18 before re-piloting #12.** Also overlaps banking (#10) / dor-template (#14) on the same `_merged.zip`s → sequence rollouts (merge one, rebase + `--continue` the rest; the fix re-derives each zip from base, so idempotent).

## PROBLEM-panel-type-ubs — Panels should use the UBS custom panel component, not the default AEM panel
**Symptom:** wizard panels use the **default AEM panel** (`sling:resourceType="fd/af/components/panel"`) instead of the **UBS custom panel** (`ajila-forms-customers/ajila-forms-ubs/components/controls/panel`). The default panel lacks the UBS authoring sections (notably the **Summary** tab that carries `jumpToFieldButtonVisible`), so those options can't be set/rendered. The engine emits the UBS custom panel everywhere; deployed forms and the step-title sweep's wrappers use the default one. Corpus: **2,798** non-fragment default panels across **258 forms** (5,840 already UBS). **Fragment panels (`fragRef`) are excluded** — they legitimately use the default panel. Every form keeps its always-UBS summary/preview panels, so none is 100% default.
**Detect:** script: `find_panel_type.py`  _(counts/lists `guideNodeClass="guidePanel"` nodes with `sling:resourceType="fd/af/components/panel"` and no `fragRef`; `affected` = forms with ≥1)_
**Fix:** script: `set_panel_type.py` _(byte-offset splice: change only the `sling:resourceType` value `fd/af/components/panel` → `ajila-forms-customers/ajila-forms-ubs/components/controls/panel` on qualifying guidePanels; fragments (`fragRef`) and already-UBS panels untouched; prints `{inner, swapped, skipped_fragment, ok}`)_
**Verify:** the fix script's `ok` (well-formed) + a re-scan showing 0 non-fragment default guidePanels remain + a pilot **visual spot-check** (panels render identically on the UBS component). Fragments must stay on the default panel.
**Max changed lines:** omit — one attribute value changed per swapped panel (no adds/removes); gate on the fix's `ok` + re-scan.
**Origin:** engine-diff (engine emits the UBS custom panel; deployed forms + our step-title wrappers used the default)
**Tracking:** #20
**Status:** **swept 2026-07-01 (258/258 fixed + deployed, PR #21 merged)** — 2,798 non-fragment default-AEM panels swapped to the UBS custom panel; 666 fragment panels left untouched (verified byte-identical on AACR: 11 default + 4 UBS fragments). Prerequisite for #12 (jump-to-field) — now unblocked.

## PROBLEM-fragment-panel-aem — Fragment panels should use the default AEM panel, not the UBS custom panel
**Symptom:** fragment-reference panels (`guideNodeClass="guidePanel"` with a `fragRef`) use the UBS custom panel (`ajila-forms-customers/ajila-forms-ubs/components/controls/panel`) instead of the **default AEM panel** (`fd/af/components/panel`) the engine emits for fragments. **Inverse of [PROBLEM-panel-type-ubs](#) / #20** (which swaps *non-fragment* panels default→UBS and leaves fragments alone). Corpus: **629** UBS-type fragment panels across **202 forms** (666 already default). Includes the hidden `formmetadata` fragment.
**Detect:** script: `find_fragment_aem.py`  _(counts fragRef guidePanels by resourceType; affected = forms with ≥1 UBS-type fragment panel)_
**Fix:** script: `set_fragment_aem.py` _(byte-offset splice: change only `sling:resourceType` UBS→`fd/af/components/panel` on fragRef guidePanels; non-fragment panels + already-default fragments untouched; prints `{inner, swapped, ok}`)_
**Verify:** the fix's `ok` (well-formed) + a re-scan showing 0 UBS-type fragment panels remain + a pilot visual spot-check (fragments render identically on the default panel).
**Max changed lines:** omit — one attribute value per swapped fragment panel; gate on `ok` + re-scan.
**Origin:** engine-diff (engine emits fragment panels as the default AEM panel)
**Tracking:** #24
**Status:** **swept 2026-07-02 (202/202 fixed + deployed, PR #25 merged)** — 629 UBS fragment panels → default AEM; already-default fragments + non-fragment panels untouched. Engine-confirmed (fresh AAAE conversion emits fragment panels as `fd/af/components/panel`). `formmetadata` fragment included (engine emits a metadata *control*, not a fragment, so no engine ref — treated as a normal fragRef panel).

## PROBLEM-nav-button-order — Wizard toolbar navigation buttons in the wrong order
**Symptom:** the toolbar's core nav buttons must be in the engine order `nextitemnav` (Next) → `submit` (Submit) → `previtemnav` (Back), with any extra buttons (preview / `fwbSaveProgress` / custom preview / tertiary) after. 66 forms deviate — mostly Back-first (`previtemnav, nextitemnav, submit`), 4 Submit-first. Engine-confirmed via fresh conversions (AAAN, AAAE): order is Next, Submit, Back, Preview.
**Detect:** script: `find_nav_order.py`  _(compares the relative order of the three core buttons in the toolbar; affected if all three present but not Next→Submit→Back)_
**Fix:** script: `set_nav_order.py` _(byte-offset element relocation: moves the three core buttons to the front in engine order, keeps every other toolbar child in its existing relative order; prints `{order_before, order_after, ok}`)_
**Verify:** the fix's `ok` (well-formed) + re-scan showing 0 wrong + a pilot visual check (button row renders in the right order).
**Max changed lines:** omit — structural reorder; gate on `ok` + re-scan.
**Origin:** engine-diff (engine emits Next, Submit, Back, Preview)
**Tracking:** #27
**Status:** **swept 2026-07-02 (66/66 fixed + deployed, PR #28 merged)** — toolbar reordered to engine order Next→Submit→Back, trailing buttons kept.

## PROBLEM-banking-relationship-wrap — Wrap the first-page banking panel in a PN_BR exclusion panel
**Symptom:** the **first-page** banking-relationship fragment panel (`guideNodeClass="guidePanel"` with a `fragRef` ending in `affrg_BankingRelationship1`) must be wrapped in a dedicated UBS custom panel named `PN_BR` carrying `summaryExclusion="true"` + `dorExclusion="true"`, so the whole banking subtree is excluded from the summary and the DoR. This is an established hand-authored convention — **21 forms already ship the `PN_BR` wrapper** (UBS custom panel, both exclusions, banking fragment as sole child). The fragment panel already carries `dorExclusion="true"` from [PROBLEM-banking-relationship-fragment](#) / #10; this sweep adds the *summary* exclusion via the wrapper and gives the DoR exclusion its correct structural home (a wrapper isolates the whole subtree, which flags on the fragment panel alone may not do in the rendered summary). **First page only** — banking panels on later wizard steps are left alone (9 forms are later-page-only; no form has banking on both page 1 and a later page).
**Detect:** script: `find_banking_wrap.py`  _(finds first-page banking fragRef panels; affected = forms with ≥1 unwrapped one; already-`PN_BR` (name, or a UBS panel with both exclusions + sole child) and later-page banking are skipped — idempotent for rebase + --continue)_
**Fix:** script: `set_banking_wrap.py`  _(byte-offset wrap surgery mirroring #18: moves the fragment panel **intact** into a new `PN_BR` UBS custom panel with summaryExclusion+dorExclusion; multiple panels spliced back-to-front; prints `{inner, wrapped, skipped_wrapped, skipped_later, ok}`)_
**Verify:** the fix's `ok` (well-formed) + a re-scan showing 0 unwrapped first-page banking panels remain + a pilot **visual spot-check** (banking section no longer appears in the summary; the form itself renders unchanged).
**Max changed lines:** omit — additive structural wrap; gate on the fix's `ok` + re-scan.
**Origin:** manual (candidate engine-fix — the engine emits the exclusion flags *on* the fragment panel in `profiles/ubs/aem/fragment.xml`, which may not exclude the rendered subtree from the summary; the `PN_BR` wrapper is the reliable structure the owner hand-authored on 21 forms).
**Tracking:** #29
**Status:** **swept 2026-07-02 (232/232 fixed + deployed, PR #30 merged)** — first-page banking fragment wrapped in a `PN_BR` UBS panel with summaryExclusion+dorExclusion; 22 already had `PN_BR`, 9 later-page-only left untouched. `validate_delta` was updated to ignore benign ancestor re-paths (a wrap/move re-paths a pre-existing violation deeper — one-in/one-out, net-zero), so the rollout ran 0-flagged.

## PROBLEM-nav-save-progress-button — Remove the "Save Progress" button from the wizard toolbar
**Symptom:** the wizard toolbar carries an extra **"Save Progress"** button — a `guidebutton` (`sling:resourceType="fd/af/components/guidebutton"`, `guideNodeClass="guideButton"`) named `fwbSaveProgress`, a direct child of the toolbar's `items` (each with `fd:scripts` + `fd:rules` children). It should be removed from every form. **119 of 286 packages** carry it. Shape is **uniform** across the corpus (no lineage split): exactly one `fwbSaveProgress` guidebutton per affected form, always a direct child of `toolbar > items`.
**Detect:** script: `find_save_progress.py`  _(affected = forms whose toolbar `items` has a child named `fwbSaveProgress`; matches on the name in the toolbar, not a raw substring, so a stray reference elsewhere wouldn't false-positive — idempotent: a fixed form reports 0)_
**Fix:** script: `remove_save_progress.py`  _(byte-offset element excision of the `fwbSaveProgress` guidebutton, incl. its preceding indent so no dangling blank line; prints `{inner, removed, ok}`)_
**Verify:** must-not: `name="fwbSaveProgress"`  _(+ the fix's `ok` (well-formed) + a re-scan showing 0 affected + a pilot **visual check**: the toolbar no longer shows a Save Progress button and the button row renders unchanged otherwise)_
**Max changed lines:** omit — structural deletion; gate on the fix's `ok` + re-scan + must-not.
**Origin:** manual (owner directive — remove the Save Progress button everywhere)
**Tracking:** #50
**Status:** rolled out 2026-07-07 (118/119 fixed + deployed, PR #51) — pilot (6) reviewed OK, then --continue for the remaining 112, all 0-flagged. delta:skip throughout (engine MCP not built; pure well-formed-subtree deletion — no new violations possible). **AABJ_019 deferred** (1 remaining): it is also edited by open feedback PR #49, and two branches editing the same LFS `_merged.zip` conflict at merge — sweep it via `--continue` once #49 lands (Detect re-scans, so it's picked up automatically). **Not yet `swept`** — mark swept + run `check_regressions.py` (expect 0 affected) only after AABJ_019 is done.
