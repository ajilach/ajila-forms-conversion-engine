# AEM Forms Output Conventions

> **Scope:** This document describes — for an AEM form designer — the conventions
> an exported Adaptive Form follows: how it is structured, how it paginates, how
> the toolbar looks, how step titles and the Document of Record behave, and so
> on. It is written in authoring terms, not technical ones.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Naming Conventions](#2-naming-conventions)
3. [Form Structure](#3-form-structure)
4. [Pagination / Wizard Steps](#4-pagination--wizard-steps)
5. [Step Titles](#5-step-titles)
6. [Toolbar](#6-toolbar)
7. [Preview & Summary Steps](#7-preview--summary-steps)
8. [Components](#8-components)
9. [Layout & Columns](#9-layout--columns)
10. [Document of Record (DoR)](#10-document-of-record-dor)
11. [Fragments](#11-fragments)
12. [Custom Elements](#12-custom-elements)
13. [Languages & Translations](#13-languages--translations)
14. [Document of Record Branding](#14-document-of-record-branding)
15. [Form Location](#15-form-location)

---

## 1. Overview

An exported form is a complete, import-ready Adaptive Form. It is shaped to match
a hand-built UBS reference form, so the result looks and behaves like a
manually-migrated form. The conventions below capture that shape.

---

## 2. Naming Conventions

Every component is given a name following the pattern:

```
PREFIX_<CamelCaseName>_<shortUuid>
```

- **PREFIX** identifies the component type (see table below).
- **CamelCaseName** comes from the component's label.
- **shortUuid** is a short identifier appended so the name is unique.

Names must be unique within the form because they are referenced from rules.
Example: `ST_ConRiferimentoAiServiziDi_84464274`.

| Component | Prefix |
|-----------|--------|
| Panel / step | `PN_` |
| Preface panel | `PRF_` |
| Appendix panel | `APX_` |
| Repeatable panel | `RP_` |
| Static text | `ST_` |
| Step title | `TTL_` |
| Footnote placeholder | `FNP_` |
| Text box | `TXT_` |
| Number box | `NB_` |
| Date | `DATE_` |
| Email | `EML_` |
| Telephone | `TEL_` |
| Checkbox | `CB_` |
| Radio button | `RB_` |
| Dropdown | `DD_` |
| Image | `IMG_` |
| Interactive table | `TBL_` |

The full prefix catalogue is in [AEM Naming Conventions.md](AEM%20Naming%20Conventions.md).

---

## 3. Form Structure

Every form is built into the same overall structure:

- A **header** at the top.
- The **form title**, taken from the form's main heading (the first H1). If there
  is no H1, the form code is used.
- The **content steps** of the wizard (see [Pagination](#4-pagination--wizard-steps)).
- An optional **Summary** step and an always-present **Preview** step.
- The **toolbar** at the bottom.
- A **footer**.

The form is set up to generate a Document of Record, uses the UBS theme, submits
through the UBS submit action, and redirects to the success-confirmation page
after submission.

---

## 4. Pagination / Wizard Steps

The form is a **wizard**: each top-level panel is a navigable step shown as a tab
in the wizard navigator.

Steps follow the document's headings:

- Each **H2 heading starts a new step**, and the heading text becomes the step
  title.
- **Content before the first H2** is placed at the top of the first step rather
  than getting its own step.
- A document with no H2 becomes a single step.
- Empty panels are removed.

Two extra steps are added at the end:

- An optional **Summary** step.
- A **Preview** step (always present).

---

## 5. Step Titles

For step titles, a panel is added with a static title component inside it. The
configuration is:

**Step Panel**

- No title
- Exclude title from Document of Record
- Exclude description from Document of Record

**Title Panel**

- Set title
- Exclude description from Document of Record

**Title Component**

- CSS Class: `stepTitle`
- Exclude from Document of Record

With this configuration the title from the Title Panel is shown in the DoR, and
the Title Component is shown in the Adaptive Form.

---

## 6. Toolbar

The toolbar sits at the bottom of the form and contains, in order:

| Button | Behaviour |
|--------|-----------|
| **Next** | Moves to the next step. When a Summary step is present, it fills in the summary first. |
| **Submit** | Submits the form. Shown **only on the last step**. |
| **Back** | Moves to the previous step. |
| **Preview** | Generates and shows the preview. Shown only on the last step. |
| **Save Progress** | Saves the current form data. |

All toolbar buttons are excluded from the Document of Record. Their labels are
translated (see [Languages & Translations](#13-languages--translations)).

---

## 7. Preview & Summary Steps

These two steps are always part of the form's structure:

- **Preview step** (always present)
  - Excluded from the Document of Record and from the Summary.
  - Shows an e-signature check message, a submission-info message, the preview
    itself (hidden until "Preview" is clicked), and error messages.

- **Summary step** (optional)
  - Excluded from the Document of Record.
  - Shows a read-back of the entered values, plus error messages and hidden
    metadata about the form (form code, entity, language, version, release date,
    and similar) used by downstream tooling.

---

## 8. Components

Each field type maps to a UBS form component. In general:

- The label becomes the component title.
- Mandatory fields stay mandatory.
- Hidden fields are also excluded from the Document of Record.

Type-specific behaviour:

| Component | Notes |
|-----------|-------|
| Text box | Supports an optional maximum-character limit. |
| Multiline text | Multi-line input. |
| Number box | Numeric formatting. |
| Date | Format `YYYY-MM-DD`, defaults to the current date. |
| Dropdown | Carries its option list. |
| Checkbox | Title hidden, options support rich text. |
| Radio button | Carries its options and alignment. |
| Static text | Free text / HTML content. |
| Step title | Uses the `stepTitle` CSS class. |
| Footnote | Uses the accordion footnote styling. |
| Fragment reference | References a fragment; excluded from the DoR. |
| Conditional panel | Title and description excluded from the DoR. |
| Repeatable | Supports a minimum/maximum number of instances with add/remove buttons. |

---

## 9. Layout & Columns

- Panels lay their children out in a single column, stacked vertically.
- Each field has a defined width within the 12-column grid.
- Panels can carry their own column settings for the Document of Record.

---

## 10. Document of Record (DoR)

The form generates a Document of Record. What appears in it is controlled
carefully:

- The root panel's title and description are excluded.
- Each step's own title/description is excluded; the dedicated Title Panel
  supplies the DoR title instead (see [Step Titles](#5-step-titles)).
- Step titles, footnotes, fragments, toolbar buttons, and the Preview/Summary
  steps are excluded.
- Hidden fields are excluded automatically.
- DoR settings hide panel descriptions, do not show selected options, use Arial,
  and separate options with `", "`.

---

## 11. Fragments

Reusable fragments are referenced rather than copied in. They live under the
fragment library, e.g.:

```
/content/dam/formsanddocuments/<...>_fragmentlib/<fragment>
```

- The fragment library used depends on the form's entity — German, Italian,
  Swiss, or a global fallback.
- Fragment references are excluded from the Document of Record; their content
  lives in the fragment itself.

---

## 12. Custom Elements

Certain recurring sections are replaced wholesale with hand-prepared blocks
rather than auto-generated content. Each is recognised by its section title and
brings in a predefined block (plus any blocks it depends on). Examples:

| Section title | Replacement |
|---------------|-------------|
| `Tipo` | Italian type selector |
| `Formular Adressat` / `Form addressee` | German/English addressee radio |
| `Kundendaten` | German account-holder block |
| `Signature(s)` / `Unterschrift(en)` | German signature blocks |
| `Dati del/i cliente/i …` | Italian account-holder block |
| `Firma/e` | Italian signature blocks |

These blocks reference each other (for example, signature blocks read the
addressee selection), so a matched block also pulls in the blocks it depends on.
Matching checks all language variants of a section title.

---

## 13. Languages & Translations

- The form carries a translation dictionary per language.
- Locale variants are added automatically (for example, German also produces a
  Swiss-German variant).
- Standard UI strings — the toolbar buttons and system messages — are translated
  in every language so labels like Next, Back, and Submit appear correctly.

---

## 14. Document of Record Branding

The Document of Record uses the UBS blank DoR template and includes:

- A header logo, form type, header text, and form title.
- An address block whose sender title is **"Banking Relationship"**.
- A banking-relationship flag and the application code.

---

## 15. Form Location

The form is filed under a path derived from the form code and entity:

- A **country folder** chosen from the form's entity (Germany, Italy,
  Switzerland, or a global fallback).
- A **prefix folder** based on the form code.
- A form folder named `AF_<form-code>`.

No schema is bundled, so the form's fields are unbound unless a schema is
explicitly enabled.

---

## See Also

- [AEM Naming Conventions.md](AEM%20Naming%20Conventions.md) — full prefix table.
- [AEM.md](AEM.md) — the underlying content format (technical reference).
</content>
