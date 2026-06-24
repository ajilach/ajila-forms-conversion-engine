# AF Design Guidelines — UBS Forms (Condensed)

> Dense reference for modifying AEM Adaptive Form (AF) output: components, DoR/PDF generation, rule-editor calls, naming conventions, styling and APIs. Condensed from the full "AF Design Guidelines" spec; page headers/footers, screenshots and repetitive boilerplate removed. Example form codes (e.g. ABKA, ACBU, BAMA) are illustrative.

---

## 1) Introduction, 2) Form Templates, 3) Adaptive Form

### 1. Introduction — Adaptive Form Types
- Every Adaptive Form is created from a **Form template**.
- **Form**: single adaptive form for one purpose (most common).
- **FormSet**: multiple Forms in an accordion style; references subforms into one adaptive form.
- **Confirmation page**: both Form and FormSet land here after submit; configurable per form; multiple confirmation pages can be created and referenced.

### 2. Form Templates

**2.1 Basic template** — source for every Single Adaptive Form. Creates a 2-step structure:
- `Formtitle` (same across every step); `Progress bar` (visible every step); `Step title (Step 1)` (per-step); `Toolbar` with 4 buttons:
  - `Next`: visible while a further step exists; validates current step; replaced by Submit on last step.
  - `Back`: visible when a predecessor step exists.
  - `Preview`: on last step; generates carousel preview.
  - `Submit`: on last step; validates + submits; redirects to confirmation page.

**2.2 FormSet template** — source for every **Form0** and **Business Formset**. Structure: overview of all subforms + panels referencing subforms + preview section. User clicks `Edit` per subform; `Finish` completes a subform (tick in overview); when all complete, `Next` → preview of whole formset; `Submit` as in single forms.

**2.2.1 Configuration** (reference an AF in a formset):
1. Create AF, select **"UBS FormSet"** template.
2. Pick a form name (`NAME`), e.g. `changeOfAddress`.
3. In "Subform1 overview" panel: rename panel → `NAMEOverview` + add title; rename `subform1Title` → `NAMETitle`; rename static text "Subform 1" → user-visible name.
4. Close "FormList" panel; open "Subform Reference 1": rename → `NAME` + title; reference subform in field **"Form or Fragment"**.
5. In rule editor of overview Edit button, on click: `window.forms.ubs.formset.enterSubform(NAME, overviewPanel);`
6. Repeat per subform; copy example panels for more.

**2.2.2 Options — DoR generation** (set in **RootPanel** config of formset):
- **Default DoR rendering**: all subforms in one DoR, each rendered into the formset-config template (custom XDP per subform NOT used). One PDF.
- **Form0**: each subform creates its own DoR (per its own config); DoRs merged into one PDF.
- **Business Formset**: each subform creates own DoR stored in CLP; all PDFs added to a `.zip`; user downloads zip.

**2.2.3 Conditional forms** — dynamically include/exclude subforms (pass the referencing panel):
- `window.forms.ubs.formset.includeSubform(panel);`
- `window.forms.ubs.formset.excludeSubform(panel);` (excluded subform need not be filled, absent from DoR).

**2.2.4 Subform completion indicator** — remove via: `$('.'.concat(subform.name).concat('Overview')).removeClass('subformCompleted');`

**2.2.5 External documents on Formset level** — only apply to formsets with the **"Merged PDF"** option on rootPanel; numbered per the **"Consecutive Numbering"** checkbox.

**2.3 Confirmation page template** — `Formtitle` (prefilled "Many thanks!…", changeable); `Success icon`; `Download button` (lets user download generated document).

**2.4 Using a template** — template choice presented during AF creation.

### 3. Adaptive Form

**3.1 Browser Tab Title** — if Formcode + mandator are set in the metadata component, tab title = master CDOK + formcode. Mandator from URL param `mandator` (e.g. `…/jcr:content?wcmmode=disabled&afAcceptLang=fr-ch&mandator=001`). Missing/wrong → falls back to current form name.

**3.2 Multi column form** — legacy "Number of Columns" + "Colspan" props removed; replaced by AEM **"Layout" mode**. Layout-mode columns affect only the AF, NOT the DoR (DoR multi-column configured separately). Old forms unaffected.
- **Access**: action bar → **"Layout"**. Steps side panel hidden in Layout mode.
- **Use**: click component → two blue dots; drag to resize width within a **12-column** grid. Use **"Float to new line"** to push a component to the next line.

**3.3 Submission**:
1. Open adaptive form container config → **"Submission"** tab.
2. **"Redirect URL/Path"**: select confirmation page. Base path: `/content/forms/af/ajila-forms-ubs`.
3. **"Submit Action"**: default **UBS Submit** (triggers PDF generation).

**3.4 Publishing** — select AF on author instance → **"Publish"** → wait for success popup.

**3.5 Multistep separation** — UBS standard splits form into steps. Conform to existing PDF section groupings; combine sections with ≤3 components; keep steps small (no scrolling within a step); build recurring sections (e.g. address) identically (consider fragments).

**3.6 Adaptive Form Types** (PDF naming → §7.7):
- **Single Adaptive Form** — from **"UBS"** template. One main CDOK; one main doc. Consecutive attachments merged into main doc; individual attachments → separate PDFs (Annexes).
- **Form0** — Formset tech with overview page; includes Single AFs; same characteristics. All "Main" docs of subforms merged into one output (same CDOK). Use when single AF has technical limits or for better UX. *Example*: ABKA — 4 subforms share CDOK 63126 → merged.
- **Business Formset** — Formset tech with overview page; handles multiple CDOKs; one "Main" doc per subform (CDOK) + all individual attachments. The "real" Formset (shown as such in Kiosk). *Example*: ACBU — Change of Name (61501), Change of Domicile (61502) → multiple Main docs.
- **DoR config for Form0** — set on Formset **Root Panel**; checkbox selects continuous pagination across subforms vs. separate pagination per subform.

---

## 4) Components

### 4.1 Basic configuration (all UBS components)
**Basic tab**: `Name` (unique technical id; used as XML tag when no schema referenced); `Title`; `Hide title`; `Placeholder Text`; `Required field` (validated each step change / before submit); `Required Field Message`; `Script Validation Message`; `Bind Reference` (bind to schema node, mainly custom XDPs); `Default Value`; `Hide Object` (not displayed); `Disable Object` (not editable); `CSS class`.
**Help tab**: `Short description` (underneath component); `Long description` (question-mark icon reveals on click).
**Patterns tab**: define display/validation patterns (AEM AF expressions).

### Component reference
- **Static title** — non-interactive title; emphasis/alignment/lists/links. For step-title-alternative styling apply CSS class `stepTitle`.
- **Static text** — non-interactive text; same emphasis options.
  - *Listings*: for roman/alpha lists: bullet-list symbol → HTML view → on `<ol>` add `style="list-style-type: *STYLE*;"` (STYLE ∈ `lower-roman`, `upper-roman`, `lower-alpha`, `upper-alpha`).
- **Information text** / **Error text** — like static text with info-box / error-box appearance. (NOTE §28.3: replaced by **MessageBox** going forward.)
- **Text Box** — single-line. Config: `Maximum/Minimum/Exact Number of Characters`.
- **Text Box Multiline** — shows 3 lines, scrollbar beyond; same char configs.
- **Numeric Box** — `Data Type` (`Empty`≈Decimal / `Decimal` 2 dp / `Integer` / `Float`); `Lead digits`; `Frac digits`; `Minimum value` + `Exclude minimum value`; `Maximum value` + `Exclude maximum value`.
- **Email** — derives from Text Box; `Enable autofill`; preconfigured email validation pattern + error text.
- **Telephone** — derives from Text Box; `Enable autofill`; preconfigured display + validation patterns + error text.
- **Datepicker** — calendar + manual input; locale-translated (EN/DE/IT/FR; else EN fallback; DE/FR/IT week starts Monday, EN Sunday). `Set current date as default value`; `Year range (from)`/`(to)`; `Minimum value` (`YYYY-MM-DD`) + `Exclude minimum value`; `Maximum value` + `Exclude maximum value`.
- **Drop-down List** — `Items` pattern `{key}={value}` (e.g. `USD=United States Dollar`; option group `afOptGroupName={groupName}`); `Sorting` (`Default`, `Key ASC/DESC`, `Value ASC/DESC` — value sorts use translated values); `Allows multiple selection`; `Number of selections`; `Allow filtering`; `Items Load Path`.
- **Radio Button** — `Items` pattern `{value}={text}`; `Item Alignment` (horizontal/vertical); single selection.
- **Check Box** — `Items` pattern `{value}={text}`; `Item Alignment`. *Checkbox indention DoR*: Content Panel with `Number of Columns in Document of Record`=6; inner panel colspan 1, place checkbox after a colspan-5 panel → indented.
- **Terms and conditions** — title + text box + checkbox; `Consent text` (next to agree checkbox); `Content For Terms and Conditions` (before consent text).
- **Image** — `Image` (asset/upload); `Alternate Text`.
- **Line Separator** — `Thickness` (px).
- **Content Panel** — groups/aligns components in a column grid. Components stick together in DoR **only if the panel has a panel title**; titled panels must not exceed one DoR page height or generation fails — split content.
- **Chart** — `Chart type`; `Repeating Row or Panel Name`; `X Axis`/`Y Axis` (`Title`, `Field`, `Function`); `Tooltip` (use `${x}`, `${y}`).
- **Accordion** — panel with layout `Accordion`.
  - *Footnote Placeholder*: links multiple footnotes on a step to one placeholder. (1) add footnote to static text via star icon (auto-numbered by placement); (2) add `footnote placeholder` component at step bottom → renders as hyperlink jumping to placeholder.
  - *Footnote*: style an Accordion-layout panel with CSS class `ubsAccordionFootnote` (info-box background).
  - *DoR config*: choose title vs description display — **title shown with a line above**, **description without a line**. Flags: `Exclude title from Document of Record`, `Exclude description from Document of Record`; to show description, global DoR `Hide description of panels` must be **unchecked**.
- **Footer** — not edited per-form; loaded on load from dictionary `/apps/ajila-forms-customers/ajila-forms-ubs/i18n/dictionary`, key `ajila-forms-ubs-footer-text` (per language). Changing dictionary affects all forms.
- **Generic Rule Editor — Mandatory**: `window.forms.ubs.components.setMandatory(this, true);` (param1 = component context; param2 = boolean; respects incomplete-form-mode state).
- **Forced line break** — Form title: `&#x2029;` under Master page config in DoR config (DoR); Shift+Enter in AF.

---

## 5) Configuration Components
Not shown in form or DoR; used for config.

### 5.1 Document of Record Configuration
Part of "UBS" template, on step 2 — keep on last step. Deletable if no DoR config needed.
- **Custom XDP Configuration**: checkbox "Enable external custom XDP"; configure CDOK/formcode reference to retrieve XDP.
- **Preliminary Document Configuration**: multiple preliminary docs via formcode + CDOK (UBS Forms API). `Individual` = NOT in DoR PDF; added to ZIP with own UUID + own CLP storage. `Consecutive` = included in DoR numbering.
- **Attachment Configuration**: multiple attachments merged into DoR (e.g. T&Cs), added AFTER the DoR. `Individual`/`Consecutive` as above.

**5.1.1 Attachment exclusion** (dynamic):
```js
window.forms.ubs.excludeAttachments(["ABCD", "EFGH"]);  // or "ABCD"
window.forms.ubs.includeAttachments(["ABCD", "EFGH"]);  // only removes from exclusion list
```
- Inspect excluded: `guideBridge.getData({success: function(data){console.log(data.data)}})`

### 5.2 Banking Relationship and form ID
Set footer-left/center + banking relationship via text boxes; write form-ID logic in each form's rule editor; exclude these boxes from DoR (shown in header/footer). Required exact names: `txtBankingRelationship`, `txtFormId`, `txtMiddleFormId`.

### 5.3 Address Multiline Text
Multiline textbox named `txtAddressHeader` sets the address value in the address template.

---

## 6) Component Combinations

### 6.1 Single option with accordion detail (only ONE checkbox selectable)
**Basic structure**: New AF (UBS template), delete Step 2. Add Child Panel to Root ("Options Panel") with CSS `ubsAccordionPanel` (gray bg) + `ubsAccordionCollapsed` (collapsed on init).
**Checkbox option + accordion detail**: Child Panel per option → Check Box (name matters for rules, e.g. `checkboxVeryLow`; "Hide title"; CSS `validationErrorNotVisible`) → Child Panel "Accordion" (Layout="Accordion") → "Details" (Colspan=2, Columns=2) → "LeftSection"/"RightSection" (Colspan=1, Columns=1) → per-row Child Panel (Colspan=1, Columns=3, CSS `ubsAccordionPanelRow` adds underline) → two Static Texts: description (Colspan=2, CSS `ubsPlaceholderFontColor` gray) + value (Colspan=1).
**Error text**: Error Text as LAST component in Options Panel; name matters (e.g. `riskToleranceErrorBox`); "Hide Object".
**Rules**:
- Each checkbox, Event **Value Commit** (enforce single selection) — array = ALL OTHER checkbox names:
```js
var otherCheckboxes = [checkboxVeryLow, checkboxLow, checkboxModerate, checkboxMedium, checkboxAboveAverage];
window.forms.ubs.handleSingleOption(this.value, otherCheckboxes);
```
- Each checkbox, Event **Validate** — array = ALL checkbox names incl. current; param2 = exact Error Text name (identical rule everywhere, copy-paste):
```js
var checkboxList = [checkboxVeryLow, checkboxLow, checkboxModerate, checkboxMedium, checkboxAboveAverage, checkboxHigh];
window.forms.ubs.validateSingleOption(checkboxList, riskToleranceErrorBox);
```

### 6.2 Dynamic repeating panels (user add/remove)
Limitation: cannot number instances on DoR (use §6.3 / §7.10.2.1 for numbering).
**Basic structure**: Repeat Container Panel → Content Panel "Container" → "Repeat Settings": set Minimum AND Maximum (both required; infinite = Maximum `-1`). Inside Container: Repeat Container Header Panel ("Header") + Content Panel ("Content"). Add **Tertiary Button** ("+ Add Element") to the Repeat Container Panel.
**Header**: add Repeat Container Header Title.
**Content**: add Repeat Container Button Panel ("Buttons") → Remove Button (Rule Editor → create rule "remove instance"); then add content components.
**Add Button**: "+ Add Element" → Rule Editor → create rule "add instance".
**Counter on repeatable title**: copy SOM expression from Header Title (3 dots → "View SOM Expression"). On Add Button add two rules (Events **Click** + **Initialize**); on Remove Button one rule (Event **Click**):
```js
var title = guideBridge.resolveNode('<SOM_EXPRESSION>').nonLocalizedTitle;
for (var i = 0; i < partnerContainer.instanceManager.instances.length; i++) {
  var num = i + 1;
  partnerContainer.instanceManager.instances[i].header.headerTitle.title = title + " " + num;
}
```
(Replace `partnerContainer`/`header`/`headerTitle` with actual binding names. Variant: skip number on first instance with `if (i === 0)`.)
**Scroll to top on new instance**: `window.forms.ubs.components.scrollTop(PN_Recepient_RP);`

### 6.3 Static repeating panels (enables numbering)
Trade-off: max entries preconfigured, shown/hidden dynamically.
**Structure**: Content Panel (Name `contractingPartners`, CSS `ubsRepeatPanel`) → Content Panel per element (Name `contractingPartner1`…) → "header" panel (Static title + Remove button on one line; verify mobile) → all components (unique names prefixed by element number, e.g. `partner1Company`). Copy-paste first element to desired count; adjust numbers. Add Tertiary Button ("Add partner"). Optional Separator in header of all elements except first.
**Rules**:
- Main panel, Event **Initialize** — hide+exclude all except first: `contractingPartner2.visible=false; contractingPartner2.dorExclusion=true;` …
- Add Button, Event **Click** — reveal next element + hide previous element's remove button (if/else-if chain). Second Click rule — hide Add button when last element shown: `if (contractingPartners.contractingPartner4.visible===true){this.visible=false;}else{this.visible=true;}`
- Each Remove Button, Event **Click** — hide element, `resetData()`, re-show previous remove button.

### 6.4 Performance Optimizations
≥10 repeating-panel instances may cause perf issues. Fix: extract repeating content into a **fragment** (Group → "Group Objects in Panel" → "Save as Fragment").

---

## 7) PDF Generation — Document of Record (DoR), Custom XDP, Naming, Styling

### 7 General
Submit ("UBS Submit") generates one or more PDFs; all PDF/A (compliance level 1B/2B/3B set in OSGI). Output types: single PDF, or ZIP (multiple docs).

### 7.1 Document of Record (DoR)
Generic PDF rendered from the AF definition; all such forms share one template. Use when PDF need not be pixel-perfect.
- **Enable DoR**: AF Properties → tab "Form Model" → radio "Generate Document of Record".
- **Select DoR template**: open AF in "Edit" → "Document of Record" icon → Template option "Custom" → select Custom Template (e.g. `UBS_Blank_DoR.xdp`; search via top-left input if not listed).
- **CRITICAL**: Deselect "For Check Box and Radio Button components, show only the selected value(s)." at bottom of DoR config — else checkboxes/radio buttons render wrong or not at all.

### 7.2 DoR templates (differ mainly in header section)
All have: `Logo` (fixed UBS logo) + `FormType` (configurable, e.g. "K"). Most have `senderAddressTitle` (default "Banking relationship") + `txtBankingRelationship` (runtime BR number). `txt*` fields = runtime-populated; non-`txt` = configurable titles.
- **UBS_Blank_DoR.xdp** — base/blank; `showBankingRelationship` checkbox shows/hides `txtBankingRelationship`.
- **UBS_Blank_Account_Suffix_DoR.xdp** — CustodyAccountNo, Company, P.O. Box + `txtPoBox`, AccountSuffix + `txtAccountSuffix` (hidden if empty).
- **UBS_Blank_Address_DoR.xdp** — `senderAddressCompanyName` (fixed "UBS Switzerland AG") + `txtAddress` (preconfig or runtime via `txtAddressHeader`; multiline; page break `&#x2029;`).
- **UBS_Blank_Address_Fax_DoR.xdp** — full sender block: `txtPostalCodeCity`, `txtReceptionPhoneNo`, `txtBusinessArea`, `txtDepartment`, `txtFirstLastName`, `txtInternalCode`, `txtStreetNo`, `txtPhoneNo`, `txtFaxNo`, `txtEmail`.
- **UBS_Blank_BankingRelationship_DoR.xdp** — sender address block: `senderAddressCompanyName`, `senderAddressCityBranch`, `senderAddressPoBox`, `senderAddressCity`, `senderAddressPhonenumber`, `senderAddressSpacer`, `senderAddressWebsite`.
- **UBS_Blank_Company_Account_DoR.xdp** — `CompanyTitle`, AccountNo title + `txtAccountNo`.
- **UBS_Blank_BankingRelationship_Account_Custody_DoR.xdp** — BR + AccountNo/`txtAccountNo` + CustodyAccountNo/`txtCustodyAccountNo` + `AddressOne` (bold) + `AddressTwo`.
- **UBS_Blank_Master_Account_DoR.xdp** — MasterNo, `txtMasterNoOne/Two`, AccountNo, `txtBCNo`, `txtMasterMo`, `txtObject`, `txtCurrency`, BCNo, Object, Currency, multiline Address.
- **UBS_Blank_Custody_DoR.xdp** — BR + CustodyAccountNo title + `txtCustodyAccountNo`.
- **UBS_Blank_Letter_DoR.xdp** — like blank but **no "FORM TITLE"** in header; BR fields only.
- **UBS_Blank_Po_Box_DoR.xdp** — BR + `UbsSwitzerland` + `PoBox` ("P.O. Box") + `txtPoBox`.
- **UBS_Blank_Safe_Deposit_Box_DoR.xdp** — BR + ProductNo + `txtCustodyAccountNo`, SafeDepositBox, BcNo + `txtBcNo`, Agency + `txtAgency`, SafeDepositBoxNo + `txtSafeDepositBoxNo`.
- **UBS_Blank_Safe_Deposit_Box_No_DoR.xdp** — same, **without "Product no."**.
- **UBS_Blank_Safe_Deposit_Box_Size_DoR.xdp** — BR + CustodyAccountNo + `txtBcNo`, `txtAgency`, SafeDepositBoxNo + `txtSafeDepositBoxNo`, Size + `txtSize`.
- **UBS_Blank_Sender_Information_DoR.xdp** — BR + CustodyAccountNo + `txtCustodyAccountNo`, Address one–four, YourBranch.
- **UBS_Blank_VestedBenefits_DoR.xdp** — `AddressOne` (bold), `AddressTwo`, `AddressThree`, VestedBenefitsAccount + `txtVestedBenefitsAccount`.
- **UBS_Blank_DoR_Footnotes.xdp** — required for footnotes (§20).

**Footer** (all templates): `AppCode` (fixed "AF"), `footerFreeText` (field name `txtMiddleFormId`), `footerPage`. Formcode/version/release-date/language/mandator are evaluated dynamically at render (see §11.7).

**Configure components**: most components have a "Document of Record" section defining what shows in the PDF.
**Multi-column DoR**: NOT via layout mode. Edit mode → parent panel **"Number of Columns in Document of Record"**; then each child gets **"Colspan for Document of Record"** (child colspan only appears after parent's is set).
**DoR form title line break**: DoR config → "Form Title" → "Enter Custom" → insert `&#x2029;`.

### 7.3 Custom XDP (pixel-perfect)
- **Enable**: AF Properties → "Form Model" → "Select From" = **"Schema"** → upload/select schema + Root Element → accordion "Document of Record Template Configuration" → radio **"Associate form template as the Document of Record template"** → select template (default language).
- **Bind components**: Edit mode → component config → **"Bind reference"** → schema tree → select XSD node.
- **Multilingualism**: one XDP per language; business logic picks by locale. Filename **`TEMPLATENAME_LOCALE.xdp`** (e.g. `ChangeOfAddress_en_us.xdp`). TEMPLATENAME identical across languages, **no underscore allowed**. LOCALE always **lower case**, underscore separates language/variant. Priority: language+variant (en_us) → language only (en) → config template.

### 7.4 Custom XDP in a FormSet
Only needed when FormSet generates per-subform PDFs.
- **FormSet schema**: Properties → "Form Model" → "Select From"="Schema" → predefined **`formset.xsd`**, RootElement=**`formset`**.
- **Binding**: select panel referencing the Custom-XDP AF → set **"Bind Reference"** to the RootElement prefixed with `/` (type manually; not searchable; must match referenced AF's RootElement).

### 7.5 External custom XDP
For XDP not on AEM. Add schema like normal custom XDP; retrieve via "Document of Record Configurations" component. XML Datastream gets attribute **APPCode = `AF`**.

### 7.6 Preview
Basic template includes a preview step (no extra setup); shows "Preview" watermark; regenerable. **Preview step must be the last step.** To add manually: add "Preview" step → move `submitErrorMessage` into it → add Information box + **Carousel** named `carouselPreview` + error box `previewErrorMessage` → add "Preview Button" after "Submit" with rules:
```js
com.ajila.forms.control.carousel.initializeForPreview(carouselPreview, undefined, previewErrorMessage);
this.visible=(!this.panel.navigationContext.hasNextItem);
```
Preview default 150 dpi (configurable in OSGI `DorPreviewService`).

### 7.7 PDF File Naming Convention
Two output types: **PDF** (one doc), **ZIP** (multiple docs).
- **Single PDF**: `MasterCdok_MAIN_FinalCdok_Formcode` → e.g. `64960_MAIN_64961_ABCQ.pdf`.
- **ZIP name**: `Formcode_Type_Timestamp` → e.g. `ACCC_Single_202105281402` (timestamp ensures uniqueness; `Type` = "Single" or "Formset").
- **PDF (MAIN)** in ZIP: `MasterCdok_MAIN_FinalCdok_Formcode_Numbering` → `61522_MAIN_61522_ACCC_01.pdf`.
- **PDF (ANNEX)**: `MasterCdok-ANNEX_FinalCdok_Formcode_Numbering` → `61522-ANNEX_61521_ACCB_02.pdf`.
- One MAIN PDF; each attachment = ANNEX. Business Formset: docs of same AF share master CDOK; numbering per master CDOK starts at `01`.

### 7.8 Additional Zeppo Translation (APAC: Traditional/Simplified Chinese)
- **Translate AF**: *Generated DoR* — translate AF normally (dictionary → ITT → import XLIFF); DoR produces Chinese copy with matching layout. *Custom XDP* — external XDP must be accessible in TC/SC with translation file.
- **User selection**: radio button + rule:
```js
if (this.value === "none") { window.forms.ubs.translation.setAdditionalOutputCopy(""); }
else { window.forms.ubs.translation.setAdditionalOutputCopy(this.value); }
```
Option keys: **`tc`** = Traditional, **`sc`** = Simplified.
- **Output**: translated docs merged with original; CLP `form_metadata` array gets an extra entry per language (differing `language`).

### 7.9 Automatic Preview (E-Banking restyle)
Preview auto-runs instead of button click. Manual change per form: on "Next" step, Click event:
```js
if (!this.panel.navigationContext.hasNextItem) {
  com.ajila.forms.control.carousel.initializeForPreview(carouselPreview, undefined, previewErrorMessage);
}
```
Then hide the "Preview" Infobox in the last step.

### 7.10 DoR Styling Options
Per-component dropdown: Panel properties → Document of Record → **"Component Style"**. No form-level change needed.
- **Numeration of repeating panels** (GitLab 1301/1330/169): for show/hide-based instances. Use `window.forms.ubs.addInstance(nameOfPanel);` (UBS call, not Adobe addInstance) for add/remove; do NOT exclude panel title from DoR; select **"repeating panel numbering"** in DoR dropdown.
- **Keep Content Together** (1411): select panel → DoR dropdown → **"DOR Keep Intact"**.
- **Signature layout without static text** (1372): set signature component DoR styling option (no empty static text).
- **Adjust height for DOR template** (1375): reduces DoR content height by **3.2mm** (gap before footer); all templates except Letter.
- **Red seal on signature block** (1891): signature scribble → DoR dropdown → **"Red seal version"**. Requires 3-col signature over a 6-col panel.
- **Wet signature panel** (2091/1672/1857): components (X = section number): Signature Instance Panel; Signature Section (DoR dropdown = **"Wet Signature Section"**, up to "…Section Two"…); Signature Date `dpSignatureDateX`; Place `txtSignaturePlaceX`; Name `txtSignatureNameX`; hidden placeholder `txtSignatureSectionX`; hidden static texts `dpSignatureRedCaptionX`, `signatureRedCaptionX`.

---

## 8) Rule Editor — Coding Guidelines
- Custom JS on AF objects; references = Adobe rule-editor docs + AEM 6.5 JS API.
- **ClientLib `afcomplib_all`** holds generic reusable JS callable from the Rule Editor. Reuse over copy-paste; ask Form Developers for a generic function when rewriting rules.
  - Example: `window.forms.ubs.getFormMetadata()` → `{mandator, language}` from URL (`mandator` default `"001"`; `language` = first 2 chars of `afAcceptLang`, default `"en"`).
- **Practices**: avoid multiple rules with the same condition on one component (combine into one rule); use descriptive variable names (`externalCompanyName`, not `k`).

---

## 9) Cross-Topic Information

### 9.1 Hide components (4 levels, different DoR impact)
- **Component "Hide object"**: hidden in AF, still shown in DoR.
- **Component DoR section**: per-component DoR exclusion.
- **Rule editor hide**: hidden in AF, shown as empty value in DoR.
- **DoR-level "Exclude hidden fields from Document of Record"**: when enabled → "Hide Object" components appear in DoR as empty value; rule-editor-hidden components do NOT appear in DoR.
- To hide in AF but show in DoR: "Hide Object" + don't exclude from DoR. If hiding via Rule Editor, do NOT enable "Exclude hidden fields from DoR" (else nothing appears → legal risk).

### 9.2 Change list styles (Static Text)
Edit HTML source, add to `<ol>` tag: Dash `class="dashed"`; Roman `style="list-style-type: upper-roman;"` (or lower-roman); Letters `lower-roman`/`upper-roman`. Do NOT use the `list-style` shorthand — breaks DoR rendering.

### 9.3 Show / Hide options (dropdowns/radios/checkboxes)
- Show (listed keys shown, rest hidden): `window.forms.ubs.components.showOptions(["ARG","CHE","DEU"], countryList);`
- Hide (listed keys hidden, rest shown): `window.forms.ubs.components.hideOptions(["ARG","CHE","DEU"], countryList);`

---

## 10) Translations
AF created in source language only; target languages via AEM Translator (source must be approved first; source changes propagate).
- **Add dictionary**: select AF → "Add Dictionary"; project title `AF_XXXX` (or `AFS_XXXX` for Formset) + target languages. When language==country (es-ES, de-DE, it-IT) use language only (`es`, `de`, `it`).
- **Export dictionary**: AEM translator `HOST:PORT/libs/cq/i18n/translator.html` → select dictionary → Export as xliff (ensure NO rows selected) → Save as UTF-8, name `FORMCODE_source_file.xliff`. Fragments need their own dictionary.
- **Import translation files**: ITT preprocessor `…/content/forms/af/translation/xliff-preprocessor.html`. Select form/fragment → upload target files → preprocess+upload. **Final author import is crucial** (publisher imports overwritten nightly; only author imports go to GIT).
  - *HTML Validation*: validator checks open vs closed HTML tag counts on upload; mismatch → error.
- **Scope**: translations affect both AF and DoR. Custom XDP: translation has NO effect — supply one XDP per language.
- **AF Manual Translations**: needs AEM Translator access (POD 1 / IC Core). Copy target-language text into source rows preserving tags exactly (`&amp;`, `<p>`, `&nbsp;`). Translate button text (Back/Next/Preview/Submit/Finish) + error messages. Workarounds for dictionary-excluded fields:
  - *Text Box default values*: add a hidden Text Box with Title = default value + a script.
  - *TextMiddleFooter*: same workaround, property name `"txtMiddleFormId"`, "Hide Object" + "Exclude from DoR", script on Initialize.
  - *Banking relationship*: add text in DoR config under `senderAddressTitle` so it enters the dictionary.
- **Chinese forms**: see §7.8; ref Confluence "AF Translations".
- **Invalid Translation Servlet** (Adobe EFORMS-20680: AEM now throws on invalid tags → breaks Preview/Submit). Identify affected forms + bad tags:
  - List forms with xliff issues: `GET /bin/com/ajila/forms/ubs/translations/htmlchecker` → array of form paths.
  - Per-form detail: `…/htmlchecker?formReference=<path>&language=de-ch` → object keyed by language → `{translation:{key,value}, validation:"<error>"}`.

---

## 11) Technical Documentation

### 11.1 Embedding Adaptive Forms
- **iFrame (recommended)**: `<iframe src="…/AF_64647/AF_64647_en.html" …>`. Easy; no reverse proxy. Needs OSGI **"Apache Sling Main Servlet"** header update — default `X-Frame-Origin=SAMEORIGIN` causes CORS error; each embedding host must be listed (`ALLOW-FROM` unsupported by Chrome/Firefox).
- **Loading resources**: Adobe-documented; CSS/JS loaded into host page; risks conflicts; requires reverse proxy. Responsive but heavy config.

### 11.2 Languages
UBS locales: `de-ch`, `en-us`, `en-gb`, `fr-ch`, `it-ch`, `ja-jp`, `nl`, `pt`, `sc` (Simplified Chinese), `tc` (Traditional Chinese). Affects translations + number patterns (`de → 1.000`, `de-ch → 1'000`). Cannot handle language==country (`de-de`, `es-es`) — AEM strips to `de`, `es`.
- **Language mapping** (`MandatorLanguageMappingConfiguration`): entry pattern `Mandator;IncomingLanguage;AemLanguage;DisplayLanguage`. `AemLanguage` = final language form opens in (e.g. `de-ch`); `DisplayLanguage` shown in DoR, stored as `txtDisplayLanguage`. Redirect service applies it. **Language code sent to CLP is always 2 chars.**

### 11.3 OSGI Configuration (`HOST:PORT/system/console/configMgr`)
- **JcrCleanupScheduler** — removes generated DoRs from temp folder. `scheduler.expression` (CRON, default `0 0 1 * * ?`); `keep.minutes` (default `60`).
- **DorPreviewService** — `preview.imageResolutionDpi` (default `150`).
- **DocumentNumeratorService** — numbers DoR pages: Font size, Margin right, Margin bottom, Page translation per-language.
- **UbsAttachmentAdapterDefaultImpl** — `Enabled`, `Service Location`, `Mandator` (Mandator default obsolete → use `DorAttachmentResolver`).
- **PdfConfiguration** — `Compliance level` (PDF/A level for every DoR/attachment).

### 11.4 System User
Install creates **`ubs-forms-tmp-writer`**: `/content/dam/formsanddocuments` (Read), `/content/forms/af` (Read), `/tmp/ubsdocs` (Read/Write/Modify/Delete — generated PDFs + cleanup).

### 11.5 CLP Integration
Authorize via **ISGA service**, then PUT to CLP (via **OP2 bundle**); `formData` follows XML structure **V3.1**.
- **FormDataPersistenceService** — `clpPersistenceEnabled: true/false`.
- **Fire-and-Forget**: submission logs to `logs/ubsdata-clp.log`; submission NOT blocked if CLP fails. On failure: DoR has no QR-code with UUID; UUID custom property empty. If CLP mandatory for an AF+mandator, configure in metadata component → submit fails if CLP unsuccessful. Runtime: `window.forms.ubs.setClpMandatory("AAAA", true);`
- **Health check**: mark via form Properties → **"UBS"** tab → sets CLP XML status `"healthcheck"`.

### 11.6 AF Caching
**`CacheManagementFilter`** appends a build timestamp query param to all CSS/JS links → browser reloads after deployment, caches until next.

### 11.7 AF Metadata Handling
**Metadata component** (part of basic & formset template; auto-added on Preview step; not shown in AF/DoR). Used for Kiosk dump, URL retrieval by business triple, DoR footer, CLP integration. Fields:
- **Formcode** (Kiosk key; auto-populates DoR config formcode, then readonly); **Master Language** (2 chars); **Languages** (`de-ch`+`de` collapse to `DE`); **Type** (`Adaptive Form` / `Form0` / `Business Formset`); **Entities** (each with a Language subset + multiple **CDOKs**; **Release Date** `dd.MM.yyyy`; first entry per mandator = **Master CDOK** fallback).
- **Final CDOK resolution priority**:
  1. **`finalCdokList`** (dynamic): `window.forms.ubs.setFinalCdok("AAAB", "62589");` (param1 = formcode, param2 = final CDOK).
  2. **`txtFormId`** (Form Data) — **deprecated**, don't manipulate directly.
  3. **Master CDOK** — first entry for the mandator. If none → preview/submit **fails**.
- **Extraction**: final CDOK + mandator (from URL) → metadata populated to DoR footer + CLP. No matching entry → preview/submit fail.
- CLP XML example: `{ pages, form_metadata:[{formcode, cdok, version, entity (=mandator), language}], pass_through_dta:{case-id, filled-by-user} }`.

### 11.8 Referenzdata (RDS)
See §12 RDS endpoints.

---

## 12) API Reference (UBS Forms afcomplib)
All services require **HTTP Basic Auth**. Some require **CSIV2** token (user ISSO token) and/or **ASN.1** token. OpenAPI 1.0.0, basePath `/app/LS4`. Some endpoints behave differently per AEM instance (author/publisher/processing).

### Core REST endpoints
- **RenderDor** — `POST /bin/api/bdr/renderdor` — create a PDF/DoR from supplied data. Headers `X-Mandator`, `X-Application-Id`. Body (form-data): `formcode`, `mandator`, `language` (2-char), `data` (Base64 XML). Responses `200`/`400`/`500`. JSON `RenderDorResponse`: `{submitTransactionUuid, exception, exceptionMessage, listSize, forms[{formUuid, formcode, finalCdok, releaseDate, version, file(base64 PDF)}]}`.
- **CreateDraft** — `POST /bin/api/bdr/createdraft` — persist a draft (LOB via OP3). Headers (mandatory): `X-Mandator`, `X-Application-Id`, `ASN1`. Body: `formcode`, `mandator`, `language`, `data`(Base64 XML). JSON `CreateDraftResponse`: `{draftTransactionId, exception, exceptionMessage, clpUuid}`.
- **OpenDraft** — `GET /app/LS4/bin/api/afforms/[Internal|External]?formcode=&afAcceptLang=&mandator=&clpUuid=` — open AF prefilled with draft data (no JSON body).
- **RetrieveDor** — `GET /bin/api/forms/retrievedor?clpUuid=&mandator=&applicationId=` — MOF access check, returns final PDFA (`application/pdf`). Header `CSIV2`. `200`/`401`/`409`. (OpenAPI path `/bin/api/afforms/retrievedor` adds optional `formcode`, `afAcceptLang`, `caseSwsc`, `caseUuid`, plus `400`/`404`.)
- **RetrieveDorData** — `GET /bin/api/bdr/retrievedordata?clpUuid=` — retrieve DoR (LOB via OP3). Headers (mandatory): `X-Mandator`, `X-Application-Id`, `ASN1`. JSON `RetrieveDorDataResponse`: `{retrieveTransactionId, exception, exceptionMessage, contentType("application/pdf"|"application/zip"|""), formData(base64)}`.
- **Flattened XSD Schema** — `GET /bin/api/xsd/getAdaptiveFormSchema?formcode=` — flattened XSD. `200` (with body success flag)/`401`/`500`. JSON `FlattenedXSDFormResponse`: `{formcode, flattenedXsd(base64), xsdFileReference, exception, exceptionMessage}`.
- **Redirect service** — `GET /bin/api/afforms?formcode=&mandator=&afAcceptLang=` — resolves business triple, HTTP-302 to the form. Variants `/external`, `/internal` (internal rewrites dev/cx/i4/r0/p0 host to official name). `302`/`404`. Validates triple completeness, unique formcode, language presence.
- **Consistency check** — `GET /bin/com/ajila/forms/ubs/consistencycheck` — JSON array of form paths with bad fragment/subform or DoR-template references.
- **Reference resolver** — `GET /bin/com/ajila/forms/ubs/referenceresolver?referencePaths=` — comma-separated paths (relative component paths, NOT full `/apps/...`); returns array of forms referencing them.
- **Form component metadata** — `GET /bin/com/ajila/forms/ubs/componentmetadata?form=<full path>` — metadata for guideTextBox/NumericBox/Telephone/DatePicker/DropDownList/CheckBox/RadioButton; array of `FormComponents` (`autoFieldKeyWord`, `guideNodeClass`, `jcrPrimaryType`, `jcrTitle`, `name`, `options[{key,value}]`, `slingResourceType`, `textIsRich`, `validatePictureClause`, `validatePictureClauseMessage`, `validationPatternType`).

### Other afcomplib OpenAPI endpoints (by tag)
**DoR**:
- `POST /bin/com/ajila/forms/ubs/dor` — DoR preview images. `multipart/form-data`: `data`, `locale`, `resource`. Produces `text/plain` = comma-separated PNG URLs.
- `POST /bin/com/ajila/forms/ubs/printpartial` — download incomplete PDFA (MOF-validated). `data`, `locale`, `resource`(optional). `200`(pdf)/`500`.

**FileUploadDownload** (`/bin/com/ajila/forms/ubs/...`):
- `GET /download?<file id>`; `POST /fileupload` (`file`, `runtimeLocale`, `formContainerPath` → `201` `{id, fileName, contentType}`); `GET /fileupload?identification&formContainerPath` (file path); `HEAD /fileupload` (`204`/`404`/`409`); `DELETE /fileupload` (`204`/`409`).

**Process**:
- `POST /bin/com/ajila/forms/ubs/saveasdraft` — save form data to CLP. `formData`, `formPath`. Returns CLP UUID. `200`/`403`(flag off)/`409`.

**RDS** (all `GET`, JSON unless noted; common params `formMandator`, `formLanguage`):
- `/rds/siap-subcategory` (SIAP subcategory `code=label`); `/rds/country?...&countryType` (`code=name`); `/rds/elsig?...&finalCdok&formPath&subFormPath&isSubformActive` (**plain text** e-signature eligibility; `finalCdok` = 5-digit CDOK prefixed with `0`; variants None/EACP/EAQS/Default); `/rds/nci-helperparams?formMandator` (country→`{isoCode, eeaMember}`); `/rds/nci-identifier` (ISO→ordered field labels).

**Retrieval** (all `GET`, JSON):
- `/authoringmetadata?formPath` (supported languages array); `/translation` (dictionary paths); `/translations/htmlchecker?formReference&language` (invalid HTML translations); `/bin/ubsforms/formfragment?mandator&formcode` (`{formCode, formPath, formXSDRef, formXSDExists, includedFragments[...]}`; `500` if no/multiple forms).

**SessionCheck**: `POST …/keepalive`, `GET …/ping` (empty `200` if session active).

### SoapUI / Postman test projects
Provide Basic Auth per server; for CSIV2-protected endpoints copy the `Cookie` from a normal open-AF request in the same environment.

---

## 13) Known Issues
- **Translator empty page**: app context `/app/LS4` breaks `.../libs/cq/i18n/translator.html`. Fix: add Application Context to CRX `/libs/cq/i18n/translator/html.jsp` (recurs on every AEM Forms install/upgrade).
- **Save/Cancel button not visible** (SP 6.5.5, fixed 6.5.6): open component config → change a property → click Preview → error appears → buttons return for the session.
- **Columns in content panel** (AEM 6.5): bug with number-of-columns + Colspan; for **form fragments** the config must be adjusted by Ajila team.
- **Dropdown shows key instead of value** (multi-select on DoR): known Adobe issue; not fixable.
- **Translation not reflected**: designer forgot "Add Dictionary" for a language; on-the-fly creation broken. In CRX `…/assets/dictionary/<lang>` if folder lacks `sling:basename` (primaryType `nt:folder`) → fails. Fix: delete corrupted folder → Save All → Author node → "Add Dictionary" → reimport XLIFF.

## 14) Things You Shouldn't Do
- Do NOT delete the Toolbar/its buttons (form unusable) or the Error Text component (Submit unusable).
- Do NOT change properties of default-initialized components (e.g. "Hide Object" on Next).
- `name` must be UNIQUE within an AF/form set (submit XML keyed by name; duplicates → fields dropped).
- **Download/Upload AFs**: download ZIP includes form + referenced fragments + DoR templates; re-uploading overwrites ALL of them (risk: stale local ZIP overwrites months of fragment work). Upload with caution.

## 15) Design Deviations
- Driven by CDD impact (DPE processing) for specific CDOKs. *Example* CDOK 64925: "Contracting partner" field should not contain the address.

## 16) Design Guidelines DoR — Standards
- **APAC (030, 046)**: APAC Information-text fragment at bottom of **Step 1 only**, AF-only (EFORMS-16589); APAC Footnote fragment at bottom of **Last step only**, DoR-only (EFORMS-16534); use APAC-specific fragments only; repeating panel max **6** instances; APAC-specific UP section fragment (EFORMS-14404).
- **CH (001, 101)**: repeating panel max **4** instances; CH-specific UP section fragment (EFORMS-6364).

## 17) Incomplete Form Mode (Print Partial)
- **Activate**: Form properties → UBS tab → "print partially".
- **Scripts**: `window.forms.ubs.printpartial.isPrintPartialActive()`; `window.forms.ubs.components.setMandatory(this, true);`; control element `window.forms.ubs.printpartial.setControlElement(this);` / `removeControlElement(this);`. Toggle listener:
```js
document.addEventListener('printPartialChange', function(printPartial) {
  if (printPartial.active) { window.forms.ubs.setFinalCdok('AAAA','12345'); }
  else { window.forms.ubs.setFinalCdok('AAAA','54321'); }
});
```

## 18) Menu (Hamburger)
- Form properties → "UBS" → field "Menu Options". None selected → menu hidden; one+ → shown.

## 19) Save as Draft
- **Activate**: enable "Save as draft" menu option (§18).
- **Behavior**: menu entry at every step. *Save*: "Draft Title" (max 50) stored in MyOnlineForms; formData → CLP returns UUID (no PDF yet) → UUID stores draft. *Update*: reopen → retrieve from CLP by UUID → prefill; re-save skips title dialog. *Submit*: generated PDF stored under draft UUID in CLP (status "Final").
- **Designer info**: `window.forms.ubs.saveasdraft.isActiveDraft();`
- **Make form ready**: Initialize-event scripts fire on draft reopen — handle ones that hide panels/objects; do NOT change layout/design.
  - *Print Blank script*: guard with `if(!window.forms.ubs.saveasdraft.isActiveDraft()){ ... }`.
  - *Static repeatable panels* (data-loss risk): remove Initialize scripts that hide instances; hide via panel properties instead. Add numeric box as first component in parent panel (hidden AF+DoR, name `NB_Counter_XXXX`, default 1); Add Click `NB_Counter_XXXX.value += 1;`, Remove Click `-= 1;`; show panels/buttons based on counter; handle Add/Remove visibility in **Initialize** too.
  - *Dynamic repeatable panels*: only handle Add/Remove visibility in Initialize.
  - *Disable Print incomplete*: replace `setMandatory(this,true)` / `setControlElement(this)` with `xxxxxxx_xxxxx.mandatory = true;`.

## 20) Footnotes
- DoR template must be `UBS_Blank_DoR_Footnotes.xdp`. In AF: name all footnote static-texts `AF_FOOTNOTE_OBJECT_TXT`; exclude the footnotes panel from DoR.

## 21) ELSIG Eligibility Check
- "Signature Level" component (info text box); 3 states: **None** (no e-signature), **EACC** (simple, confirm "Accept" in Digital Banking), **EAQS** (qualified, via Access App/Card). Must be placed on the **preview panel** (final CDOK only known at the end).

## 22) Translation of Entity Name
- UBS AG → UBS AG (DE/EN), UBS SA (FR/IT/ES); UBS Group AG → UBS Group AG (DE/EN), UBS Group SA (FR/IT/ES); **UBS Switzerland AG → UBS Switzerland AG in all languages** (never translated).

## 23) AF Security
- AEM Forms Publisher is unprotected OOTB → unauthenticated = "Anonymous". Restriction (part of `afcomplib_all`) blocks Anonymous from `/content/forms/af` and `/content/dam/formsanddocuments`. Two users: `ubs-internal-user` (context Internal), `ubs-external-user` (External).
- **Authentication**: every GET passes `OnlineFormsAuthenticationHandler` (MU9 integration). Open Empty Form: CSIV_2 present? → MU9 `validateToken` → `formAccessCheck`. Open Draft: → `validateToken` → `draftAccessCheck`. Servlet requests (`ServletFilter`, URL `*./bin/com/ajila/.*`): → `validateToken`. MU9 error → access denied.

## 24) Environmental Marker
- Marks an instance non-productive: colored header bar + "TEST" watermark on PDFs. OSGI `com.ubs.OP2.forms.internal.service.EnvironmentMarkerServiceImpl`: `Active For Test Env`, `Header Hex Colour`, `Text Hex Colour`, `Environment` (unused).
- Text translation in `afcomplib_all`: `apps\ajila-forms-customers\ajila-forms-ubs\i18n`, key `ajila-forms-ubs-environment-message` (overwritten on install).

## 25) SQL-2 Oak Index
- Check query: `…/libs/granite/operations/content/diagnosistools/queryPerformance.html` → "Explain"; "No indexes used" → add index.
- Create index: generate at `oakutils.appspot.com/generate/index`; in `/crx/de/index.jsp` under `oak:index` create subnode type `oak:QueryIndexDefinition`.
- Alternative: increase node limit via JMX `…/jmx/org.apache.jackrabbit.oak:name=settings,type=QueryEngineSettings`.

## 26) Accordion Layout — Best Practices (Legal/T&C)
- Populate BOTH the panel **'Title'** property and the expand description; missing Title → empty/blank accordion header (EFORMS-44746). Variants: heading+description / numbering only / no heading.

## 27) Layout Considerations — Do's & Don'ts
- **June 2024: AF no longer supports initial styling — all new forms must use the e-banking style.** Online forms ≠ PDFs; don't migrate 1:1.
- Buttons left-aligned only (never centered). Never use a checkbox as a radio button. Don't add blank text for spacing — use the panel **margin dropdown**. No tabular layouts. No input fields inside running text. Every input needs a label (placeholders aren't labels; always visible). Always group multiple checkboxes/radios; never place content between them. Group labels never bold, may use 16px. Tooltip on the group label (can't tooltip individual radios/checkboxes). Never recreate standard components; invalid messages must be meaningful. Never truncate labels (use info-icon tooltip). Accordion titles meaningful + short, avoid numbers-only.

## 28) AF Styling — E-Banking
- **Margin classes** (in component CSS field, `margin-bottom` px): `ubs-margin-0`, `ubs-margin-5`, `ubs-margin-10`, `ubs-margin-15`, `ubs-margin-20`, `ubs-margin-25`, `ubs-margin-30`.
- **Static Text indentation** (#1383): levels 1=5mm, 2=9mm, 3=13mm. Set via Static Text Settings → Document of Record → Component Style. **DoR only.**
- **MessageBox replaces Information/Error Text** (#1334, June 2024): stop using "Information Text", "Error Text", "Errortext carousel preview"; MessageBox adds Confirmation + Warning styles (besides Error/Info); includes the Preview step.
- **Migrating** (#1940): on Preview step change "Preview could not be created" from "Error Text" to **"MessageBox - carousel preview error"**.
- **Signature layout without empty static text** (#1372): delete empty Static Text; set Signature DoR setting to **"DoR right aligned"** (shared fragment → affects all forms).

## 29) Logging (Submit)
- `FormSubmitContext` (created at submit start): `submitTransactionId` (UUID linking all log entries), `browserlessDorRendering`, `clpPersistenceAllowed`, `userId`, `userType`, `draftUuid`, `xMandator`, `applicationId`.
- `DorPersistenceData` per form/subform; **FormMetadata**: `formUuid`, `formcode`, `mandator`, `language`, `masterCdok`, `finalCdok`, `documentType` (MAIN, FORMSET_DATA).
- Classes override `toString()` → JSON (omits CSIV2 token; Splunk-extractable). INFO = business traceability; DEBUG = technical detail. A PDF's `ClpUuid` links back to `submitTransactionId` to trace the whole submit.

## 30) RenderDor — FormMetadata Fragment
- Backend renderDor has no URL, so `getFormMetadata()` can't read it. Add the **FormMetadata** fragment (`/content/dam/formsanddocuments/afforms_global_fragmentlib/formmetadata`) as **Step 0**.

## 31) HTML Preview
- **Setup existing form**: rename preview panel → `summaryPanel` (Title "Summary of important form information"); add **Summary** component named `summaryComponent`; add the required call on the Next button; remove old preview items (carousel, preview messagebox).
- **Component level**: "Exclude from summary page" (hide from summary); "Show on summary page even if hidden" (show even when hidden, e.g. Banking Relationship). Panels + Static text/title also have **"Show jump to field button"** (edit button to jump back; panels need a Title set).
