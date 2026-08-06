//! The conversion agent's engine surface — the tool catalog and the executor
//! that drives the form-conversion engine.
//!
//! The agent extracts from the source PDF, builds and edits a **structured**
//! node tree, converts to an **AEM** node tree, edits that, packages it,
//! optionally uploads to AEM and verifies, and can consult reference forms /
//! documentation. Every tree change is snapshotted into an edit-history session
//! ([`crate::db`]) so a UI can review the full history.
//!
//! Tree mutations use a **whole-tree replace** model: the caller reads a tree
//! (`get_*`) and writes the whole tree back (`set_*`); each write is versioned.
//!
//! This type holds no LLM and no UI state: an external loop streams turns,
//! calls [`ConversionAgent::tools`] / [`ConversionAgent::execute`], and surfaces
//! the results. Network tools hit the engine's AEM client.

use std::collections::HashMap;

use blueprint::{
    AemConfig, AemConnection, AemI18nText, AemNode, AemNodeTranslated, AemOptionTranslated, Context,
    DocumentEnvelope, OutputTarget, RedactoDump, StructuredNode,
};

/// Error returned by the AEM-tree tools when nothing has been authored yet.
const NO_AEM_TREE: &str = "No AEM tree yet; author it with set_aem_translated.";

/// Error returned by the structured-tree tools when nothing has been authored
/// yet.
const NO_STRUCTURED_TREE: &str =
    "No structured tree yet; seed one with seed_structured_from_state.";

/// Returned when AEM-only machinery is reached in a run aimed at another target.
/// Should be unreachable through the app (roles are never offered out-of-scope
/// tools) but not through MCP, which serves the flat catalog.
const AEM_ONLY_STATE: &str = "This run targets Redacto; no AEM state exists.";

/// The tools that only mean anything for [`OutputTarget::Redacto`].
///
/// The structured-tree editors are deliberately *not* here: they operate on
/// [`ConversionAgent::structured`], which both targets have (an AEM run seeds it
/// when resuming a session). Only building and reviewing the dump needs the
/// Redacto state.
const REDACTO_ONLY_TOOLS: &[&str] = &["build_redacto_dump", "review_redacto_output"];

/// The tools that only mean anything for [`OutputTarget::Aem`].
///
/// Checked once at the top of [`ConversionAgent::execute`] so a Redacto run
/// rejects them with a clear reason rather than failing deeper down with
/// something misleading like "No AEM tree yet".
const AEM_ONLY_TOOLS: &[&str] = &[
    "set_aem_translated",
    "get_aem_translated",
    "get_aem_translated_outline",
    "get_aem_translated_node",
    "set_aem_translated_field",
    "replace_aem_translated_node",
    "insert_aem_translated_node",
    "remove_aem_translated_node",
    "build_aem_package",
    "get_package_info",
    "read_package_file",
    "validate_aem_package",
    "review_output",
    "upload_to_aem",
    "fetch_aem_form_html",
    "fetch_aem_dor_pdf",
];

/// All language codes appearing in any text field of a working tree (used to
/// keep a pre-loaded template's languages alive through lowering, and to pick
/// the languages a restored tree is lowered with — see [`crate::session`]).
pub(crate) fn collect_translated_languages(
    tree: &AemNodeTranslated,
) -> std::collections::BTreeSet<String> {
    fn add(text: &AemI18nText, out: &mut std::collections::BTreeSet<String>) {
        out.extend(text.languages().map(String::from));
    }
    fn add_opts(opts: &[AemOptionTranslated], out: &mut std::collections::BTreeSet<String>) {
        for o in opts {
            add(&o.label, out);
        }
    }
    fn walk(node: &AemNodeTranslated, out: &mut std::collections::BTreeSet<String>) {
        match node {
            AemNodeTranslated::Root { title, children } => {
                add(title, out);
                children.iter().for_each(|c| walk(c, out));
            }
            AemNodeTranslated::Panel { title, children, .. } => {
                add(title, out);
                children.iter().for_each(|c| walk(c, out));
            }
            AemNodeTranslated::Repeatable { title, children, .. } => {
                add(title, out);
                children.iter().for_each(|c| walk(c, out));
            }
            AemNodeTranslated::TextField { label, .. }
            | AemNodeTranslated::NumberField { label, .. }
            | AemNodeTranslated::DatePicker { label, .. } => add(label, out),
            AemNodeTranslated::Dropdown { label, options, .. }
            | AemNodeTranslated::Checkbox { label, options, .. }
            | AemNodeTranslated::RadioButton { label, options, .. }
            | AemNodeTranslated::Custom { label, options, .. } => {
                add(label, out);
                add_opts(options, out);
            }
            AemNodeTranslated::TextDraw { content, .. }
            | AemNodeTranslated::TitleDraw { content, .. } => add(content, out),
            AemNodeTranslated::Fragment { .. }
            | AemNodeTranslated::Preface { .. }
            | AemNodeTranslated::Appendix { .. }
            | AemNodeTranslated::FootnotePlaceholder { .. } => {}
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(tree, &mut out);
    out
}

/// The package writer's translation dictionary: master text → { lang → text }.
type I18nDict = std::collections::HashMap<String, std::collections::HashMap<String, String>>;

/// Render scale for on-demand page images.
const RENDER_SCALE: f32 = 1.5;

/// The workflow guidance that teaches a driving model how to operate the
/// conversion tools. Shared by every consumer so the app's autonomous loop and
/// the standalone MCP server present one source of truth: the app injects it as
/// the agent's opening message, and the MCP server advertises it as its server
/// `instructions`. Consumer-specific bits (e.g. the MCP-only `start_conversion`
/// / `write_package` bootstrap) are appended by the consumer.
pub const SYSTEM_PROMPT: &str = "\
You are an autonomous conversion agent operating the form-conversion engine via tools, \
replacing manual interaction. Goal: produce an AEM Adaptive Form that is analogous to the \
uploaded PDF(s) — a faithful recreation that a person comparing the two side by side would \
recognize as the same form. \"Analogous\" means matching the source in: the sections and their \
order; every heading, label, paragraph and footnote text (in every language the source has); \
every fillable field, with the right control type, options, default and required state; the \
visual grouping and layout (panels, columns, tables, repeatable sections); and the conditional \
behaviour. The output should look and read like the original form rebuilt as an Adaptive Form, \
not an approximation — judge your work throughout by whether the rendered AEM form resembles the \
source, and keep fixing until it does.\n\n\
If a content-package ZIP was uploaded as a template, your working AEM tree is ALREADY pre-loaded \
from it — start with get_aem_translated_outline / get_aem_translated_node to study it, then MODIFY \
the existing tree (set_aem_translated_field / replace/insert/remove) to match the source instead of \
authoring a new tree from scratch; only call set_aem_translated to overwrite it wholesale if it is \
unusable.\n\n\
Typical workflow (call tools as needed; each step is a separate call):\n\
1. Inspect the input: get_source_info, get_profile_info (form_code, languages, JCR paths, \
binding flags), list_states, explore_states, get_xfa (the authoritative text/fields, in every \
language), search_xfa (find specific fields/labels), get_plain_state_image / \
get_annotated_state_image, and get_flattened_structure_for_state (the engine's CLEAN, \
single-language, single-variant tree for ONE state — your structural reference for fields and \
grouping). A form is multilingual whenever get_source_info lists more than one language — trust \
that over get_profile_info if they disagree. You MUST carry every one of those languages into the \
final form; don't invent translations, and never drop a language the source contains.\n\
2. Find precedents (before building): BEFORE authoring any node, consult the reference DOCUMENTATION \
to build a deep understanding of the house conventions behind the issues you'll face — call \
list_reference_docs, then read_reference_doc on the relevant guides (grep_reference_docs to jump to a \
topic): the \"AF Fragments and Common Fields\" catalogue (which standard fragment + entity library to \
use for banking relationship, address, signatures, account holder, and the rest), wizard pages and \
step-title headings, DoR and summary exclusions, and the multilingual/translation rules. The \
conventions summarised in this prompt are pointers INTO those docs, not substitutes — confirm each \
against the documentation and the reference packages before you rely on it, and do not begin building \
until you understand how the reference forms handle each issue. Then work section by section. For \
EACH section, do NOT search by \
form name or a single keyword — write a short natural-language DESCRIPTION of that section (its \
purpose, the kinds of fields it has and how they're grouped) and pass it to search_references, \
which matches it semantically against the reference forms. Use grep_references only for a verbatim \
string (a field name, label, or AEM resource type); also consult grep_reference_docs / \
list_reference_forms. Different sections often match different references; study how those \
known-good forms were built with get_reference_package / read_reference_file, and optionally run \
the engine on a reference's input via source={\"reference\":\"<ref_id>\"}. Match the references' \
structure and patterns rather than inventing your own — including noticing where they reference a \
reusable fragment (a `fragRef` to a `_fragmentlib` path) instead of building a section's fields inline.\n\
3. Author the AEM tree DIRECTLY as an AemNodeTranslated: one multilingual AEM node tree in which \
every user-visible text field (title/label/content and option labels) is a per-language map like \
{\"de\":\"…\",\"en\":\"…\"}. Call get_schema('aem_translated') for the exact shape. There is no \
automated merge — YOU combine the languages and configurator variants, because you can read every \
language and see the rendered pages. Steps:\n\
  a. Read each state with get_flattened_structure_for_state (every language × every configurator \
selection, e.g. EN/Private-Person, DE/Company) plus its page image. The XFA is the authority for \
verbatim text in each language; the images are the authority for layout and section order.\n\
  b. Build the whole tree in one set_aem_translated call: lay out the sections in source order; \
for every text field include EVERY source language (pair translations by meaning and layout \
position — never leave a language blank or collapse to one); give each fillable field the right \
component type, options (real labels AND values), required/visible state and column width; nest \
fields into Panels and use Repeatable for repeating sections; where content differs by configurator \
selection, include each variant once — keep shared content shared, and NEVER reuse a node `name` \
(that collides in AEM). \
NAMING: give every node a `name` beginning with the canonical PREFIX_ for its component TYPE, per the AEM \
Naming Conventions — PN_ panel, TXT_ text box, TXTM_ multiline, NB_ number box, DATE_ date, DD_ dropdown, \
CB_ checkbox, RB_ radio, TEL_ telephone, EML_ email, TTL_ heading/title, ST_ static text (ITXT_/ETXT_ \
info/error text), IMG_ image, TBL_ table, SPT_ separator, RCP_/RCHP_/RCBP_/RCHT_ repeat-container panels, \
BT_ button, SIGN_ signature (consult the naming-conventions reference doc for the full table). Only the \
leading PREFIX_ is enforced — the rest of the name is free — and the Reviewer flags any component whose \
leading prefix does not match its resourceType (review_output's naming_violations, bucketed wrong-prefix/raw). \
PAGES: the Root is laid out as a wizard, so ONLY its direct-child Panels \
become pages (wizard steps). Set `is_page: true` on each first-level section Panel — the top-level \
sections of the form, in source order — and `is_page: false` on every Panel nested below them \
(inner groupings, column wrappers, conditional panels, fields inside a section). So a new page \
starts exactly at each first-level section and nowhere deeper: never mark a nested panel as a page, \
and never leave a top-level section without `is_page: true`. \
HEADINGS: a Panel's `title` does NOT render as a visible heading in AEM — for every section/page \
that shows a heading in the source, author an explicit `TitleDraw` (heading level 2) as the panel's \
FIRST child carrying that text; do not rely on the Panel `title` to display it, and never render the \
same heading twice (exactly one TitleDraw per heading — don't also leave a second drawn copy of the \
same text). For recurring standard sections that the bank ships as reusable fragments \
— address, signature, account holder / contractual partner / beneficial owner / power of attorney, \
banking relationship, IBAN, individual or entity basics, internal-bank-use, and the like — do NOT \
hand-build the panel's inner fields; emit a single `Fragment` node that references the fragment by \
its JCR path (`frag_ref`), exactly as the reference forms do. Find the matching fragment and its \
path in the fragment-library documentation (read_reference_doc / grep_reference_docs for \"AF \
Fragments and Common Fields\") and confirm it against the reference packages (grep_references for \
`fragRef`); pick the `_fragmentlib` matching the form's entity (e.g. germany / italy / ch / ubs / \
global). Use the exact fragment the corpus standardised on for these recurring sections: BANKING \
RELATIONSHIP → the UBS fragment `affrg_BankingRelationship1` (in `afforms_ubs_fragmentlib`), NEVER a \
germany/italy/global variant or a dam-path reference; it renders the \"UBS Europe SE\" line itself, so \
NEVER also author a standalone \"UBS Europe SE\" text draw (that duplicates it), and set \
`dor_exclude: true` on that panel. ADDRESS block → the entity's AddressBlock fragment (germany \
`affrg_germany_AddressBlock_CountryDD`, italy `affrg_italy_AddressBlock_CountryDD`, else \
`affrg_AddressGeneric1` / `affrg_Address1`), NEVER hand-built Street / No. / PLZ / City / Country \
fields; it renders Country as a dropdown and may add an \"Additional address details\" (Adresszusatz) \
line, which is standard — keep it. A fragment's internal fields are supplied by AEM at runtime from that path, so never also \
recreate them as children — that duplicates the section. Keep the fragment's `bind_ref`; for a \
section repeated per party emit one Fragment instance per party inside the Repeatable; and never \
replace a conditional panel (one with show/hide behaviour) with a fragment. \
CASCADING / DEPENDENT DROPDOWNS (one dropdown's options or value depend on another field's selection \
— in the XFA a change-event script drives it via clearItems/addItem/rawValue): do NOT make a single \
dropdown mutate its options at runtime, and do NOT rely on a value-commit/change rule — that shape \
validates but does NOT fire in this profile. Instead model it as static variants: emit one dropdown \
per parent selection, each in its own Panel holding ONLY that selection's options, authored \
`visible: false`, and shown by a `ConditionRule` on the TRIGGER field (its `conditions` — one rule \
per parent value, targeting that variant panel's `name` with `show: true`). Gate a third-level \
variant on BOTH the level-1 and level-2 selections so a stale upstream value can't keep it visible. \
Read the XFA change-event function to enumerate the branches, and take every option label, value and \
code VERBATIM from its addItem/rawValue lines — never invent one.\n\
  c. Refine with the granular editors rather than re-emitting the whole tree: \
get_aem_translated_outline maps every node by path and flags `⚠ empty` (text-bearing node with no \
text) and `⚠ 1 lang` (only one language — likely a missing translation); get_aem_translated_node \
shows a node's exact shape; set_aem_translated_field changes one field (label/title/content/options/ \
visible/mandatory/colspan/bind_ref…); replace_aem_translated_node rebuilds a node or changes its \
type; insert_aem_translated_node / remove_aem_translated_node add or drop nodes (Panel/Repeatable/ \
Root hold children). Resolve EVERY flag and verify the whole field set, the grouping and all \
languages against the source — authoring and packaging as-is is a failure.\n\
4. Package & validate: build_aem_package lowers your tree to the AEM form plus a per-language \
translation dictionary, then ALWAYS run validate_aem_package — it checks the required package \
structure and validates the form and DAM content XML against the AEM contract. If it reports \
problems, fix them in the AEM tree (set_aem_translated_field / replace/insert/remove) and rebuild; \
never upload or export an invalid package. Inspect with get_package_info / read_package_file.\n\
5. Review end to end. (a) review_output compares the source against your tree and lists input \
text/elements missing from the output plus a coverage score. For EVERY miss, fix it (edit the tree \
and rebuild) or satisfy yourself it was an intentional drop; spot-check non-master languages with \
search_xfa, since review_output compares the master language only. Every fillable source field \
(text boxes, numeric boxes, dates, dropdowns, checkboxes, radio/choice groups, signatures, …) MUST \
have a counterpart in the output — investigate and resolve any field-count mismatch or missing \
field (never silently dropped), since a lost field means data the form can no longer capture. \
(b) Confirm the result is analogous to the source, not merely complete: compare the rendered form \
against the source page images (get_plain_state_image / get_annotated_state_image) and check that \
the section order, grouping, headings, field layout and overall appearance resemble the original; \
fix any drift with the editors and rebuild. (c) If an AEM connection is configured, upload_to_aem, \
then fetch_aem_form_html / fetch_aem_dor_pdf to verify the deployed result looks like the source. \
Do not finish with unexplained misses or while the form still looks materially different from the \
original.\n\
After ANY edit to the tree, the package is invalidated — rebuild with build_aem_package and re-run \
validate_aem_package before reviewing. Consult reference documentation when unsure: \
list_reference_docs, read_reference_doc, grep_reference_docs.\n\n\
Never invent text content: take all labels/options/help text verbatim from the XFA, and never \
write copy of your own. The final form must contain EVERY language present in the source \
(get_source_info lists them) and ONLY those: never drop a language the source contains, and never \
invent a translation for a language it does not. A non-master language whose text merely repeats the \
master-language text is an untranslated stub, not a translation — supply the genuine per-language \
wording (AEM otherwise silently falls back to the master language). When done, call finish. Keep tool inputs minimal \
and valid JSON.";

// ── Multi-agent role prompts ─────────────────────────────────────────────────
//
// The desktop pipeline (see `app`'s `run_conversion`) splits the run into an
// Analyst → Author → Reviewer sequence. Each role's system prompt is composed by
// the controller as SHARED_PREAMBLE + the role addendum (+ for Author/Reviewer,
// the Analyst's plan and accumulated review reports, pinned in the system field
// so they are never evicted). The Author reuses the full [`SYSTEM_PROMPT`] as its
// authoring body; MCP still serves [`SYSTEM_PROMPT`] to external clients.

/// Prepended to every pipeline-stage role prompt.
pub const SHARED_PREAMBLE: &str = "\
You are one stage of a pipeline that converts an uploaded PDF form into an AEM Adaptive Form \
analogous to the source. Invariants for every stage: (1) Never invent text — take all labels, \
options, help and titles verbatim from the XFA (get_xfa / search_xfa); the source is the only \
authority for content. (2) Carry EVERY language get_source_info lists and ONLY those; a non-master \
value that merely repeats the master-language text is an untranslated stub, not a translation. \
(3) The reference forms and profile templates are ground truth for structure — consult them and \
copy proven shapes (fragment references, visibility scripts) verbatim rather than inventing, and \
read them fresh. When your stage is done, stop and reply with a concise, structured summary of what \
you found or changed.";

/// Analyst role: read-only source analysis + precedent research → a conversion plan.
pub const ANALYST_ADDENDUM: &str = "\
ROLE: Analyst. You do NOT edit the tree. Produce ONE detailed CONVERSION PLAN that lets the Author \
build the form without re-reading the bulky source. Inspect exhaustively (get_source_info + \
get_profile_info — form codes ending 019 = Germany, 033 = Italy; list_states + explore_states; \
get_plain_state_image / get_annotated_state_image; get_xfa / search_xfa; \
get_flattened_structure_for_state per state) AND research precedents FIRST via the reference \
documentation (list_reference_docs, read_reference_doc, grep_reference_docs — the \"AF Fragments and \
Common Fields\" catalogue, wizard pages & step-title headings, DoR/summary exclusions, translation \
rules), then per section search_references / grep_references / get_reference_package / \
read_reference_file. The plan must give, per top-level SECTION in source order: whether it is a \
wizard page (a first-level section = one page); its heading and the verbatim labels / options / \
field text in EVERY language; each field's control type; any conditional or CASCADING behaviour \
(quote the XFA change-event function and its clearItems/addItem/rawValue branches); the recommended \
standard fragment with its exact JCR path + entity library (banking relationship → \
affrg_BankingRelationship1 in afforms_ubs_fragmentlib; address → affrg_germany_AddressBlock_CountryDD \
(019) / affrg_italy_AddressBlock_CountryDD (033) / else affrg_AddressGeneric1; signatures → \
affrg_SignatureGeneric1); and any verbatim script/hook shape to copy (showAFShowDor / hideAFHideDor, \
cascade visibility scripts) with its source ref_id + file path. List the languages and any DoR / \
summary exclusion notes. Your final message IS the plan — make it complete and self-contained; the \
Author works from it, not by re-reading the source.";

/// Author role: appended AFTER the full [`SYSTEM_PROMPT`] authoring body.
pub const AUTHOR_ADDENDUM: &str = "\
STAGE NOTE: A CONVERSION PLAN produced by an Analyst is appended below as your section / field / \
precedent map — trust it and use search_xfa only to fill specific gaps rather than re-dumping the \
whole XFA. A separate Reviewer judges fidelity after you, so do NOT call finish; once you have \
authored a complete tree and run build_aem_package + validate_aem_package, stop with a short summary. \
If REVIEW FEEDBACK appears below, address EVERY point from every round, then rebuild and re-validate.";

/// Reviewer role: read-only quality gate that ends by calling `submit_review`.
pub const REVIEWER_ADDENDUM: &str = "\
ROLE: Reviewer / validator. You do NOT edit the tree. build_aem_package, then ALWAYS \
validate_aem_package; run review_output (coverage vs the source, master language) and spot-check \
non-master languages with search_xfa; compare the rendered pages against the source images (and, if \
an AEM connection is configured, upload_to_aem then fetch_aem_form_html / fetch_aem_dor_pdf). Judge \
ANALOGY to the source AND conformance to the CONVERSION PLAN appended below, and confirm every point \
in any prior REVIEW FEEDBACK is now fixed. Checklist: naming prefixes (trust \
review_output's naming_violations — a deterministic per-node check on the rendered JCR XML: each \
author-named component's leading PREFIX_ must match its resourceType, bucketed wrong-prefix/raw; \
treat any listed violation as a defect); first-level \
sections are pages and nothing deeper is; exactly one rendered TitleDraw heading per section (a Panel \
title does not render); banking = affrg_BankingRelationship1 inside a PN_BR sole-child wrapper with \
dor_exclude; address uses the entity AddressBlock fragment; DoR exclusions set; no invented text; \
every language present and non-stub; cascading dropdowns implemented as static visibility-gated \
variants (never a runtime option mutation); every fillable source field present. \
ENGINE-INTRINSIC issues — some defects come from the conversion engine itself (fixed template output, \
resourceType assignments, lowering behaviour) and CANNOT be changed by the Author via the tree editors \
(e.g. email / telephone / multiline fields render as a plain `textbox` resourceType, so review_output \
buckets their EML_/TEL_/TXTM_ names as wrong-prefix). When an issue is genuinely engine-intrinsic and \
not authorable, mark it DONE: do NOT count it against approval and do NOT send it back to the Author — \
instead note it separately as a known engine limitation for the operator. Only return issues the Author \
can actually fix by editing the tree. End by calling submit_review with approved=true ONLY if every \
remaining issue is either resolved or engine-intrinsic (not authorable); otherwise approved=false and \
report = a detailed, actionable message listing every AUTHORABLE issue (with node paths where possible), \
noting any engine-intrinsic limitations separately. Do not fix anything yourself.";

// ── Redacto target prompts ───────────────────────────────────────────────────
//
// Deliberate duplicates of the AEM constants above rather than a shared
// fragment library: only about fifteen of SYSTEM_PROMPT's ~140 lines are
// target-neutral, so a composition layer would abstract almost nothing while
// perturbing the working AEM path. The copied blocks are marked with the lines
// they came from, and `redacto_prompts_do_not_leak_aem_vocabulary` in the app
// guards the split. Revisit when a third target lands.

/// Prepended to every Redacto pipeline-stage role prompt.
/// Mirrors [`SHARED_PREAMBLE`]; invariants (1) and (2) are copied verbatim.
pub const REDACTO_SHARED_PREAMBLE: &str = "\
You are one stage of a pipeline that converts an uploaded PDF into a Redacto text document \
analogous to the source. Invariants for every stage: (1) Never invent text — take all headings, \
paragraphs, list items and footnotes verbatim from the source (get_xfa / search_xfa); the source is \
the only authority for content. (2) Carry EVERY language get_source_info lists and ONLY those; a \
non-master value that merely repeats the master-language text is an untranslated stub, not a \
translation. (3) A Redacto document is text only — it has no fillable fields, no scripts and no \
conditional behaviour. When your stage is done, stop and reply with a concise, structured summary \
of what you found or changed.";

/// Redacto authoring body, the Author's counterpart to [`SYSTEM_PROMPT`].
pub const REDACTO_SYSTEM_PROMPT: &str = "\
You are an autonomous conversion agent operating the form-conversion engine via tools, \
replacing manual interaction. Goal: produce a Redacto text document that is analogous to the \
uploaded PDF(s) — a faithful recreation that a person comparing the two side by side would \
recognize as the same document. \"Analogous\" means matching the source in: the sections and their \
order; every heading (at its original level), paragraph, list, table and footnote, in every \
language the source has; the inline emphasis and superscript markers; and the multi-column \
sections. A Redacto document is TEXT ONLY: it has no fillable fields. If the source turns out to \
carry input fields, say so plainly in your summary rather than inventing a representation for \
them.\n\n\
Typical workflow (call tools as needed; each step is a separate call):\n\
1. Inspect the input: get_source_info, list_states, explore_states, get_xfa (the authoritative \
text, in every language), search_xfa (find specific passages), get_plain_state_image / \
get_annotated_state_image, and get_flattened_structure_for_state (the engine's CLEAN, \
single-language, single-variant tree for ONE state). A document is multilingual whenever \
get_source_info lists more than one language. You MUST carry every one of those languages into the \
final document; don't invent translations, and never drop a language the source contains.\n\
2. SEED, don't hand-author. Call seed_structured_from_state with the master-language state: the \
engine has already parsed that variant correctly — block structure, heading levels, list nesting, \
inline markup, footnote markers and multi-column sections all intact. Re-emitting that yourself \
with set_structured would lose exactly those details. Seed first, always.\n\
3. Layer in the other languages. There is no automated merge — YOU pair the languages, because you \
can read every one of them and see the rendered pages. For each state in another language, read it \
with get_flattened_structure_for_state and add its text to the corresponding node: \
get_structured_outline maps every node by path, get_structured_node shows a node's exact shape, and \
set_structured_field writes one field back. Every text is a per-language map like \
{\"de\":[…],\"en\":[…]} — call get_schema('structured') for the exact shape. Pair by meaning and \
layout position (use the page images), never by guesswork. Never leave a language blank, and never \
collapse a multilingual text onto a single entry.\n\
4. Fix what the outline flags. `⚠ text?` / `⚠ label?` mark missing or placeholder text; \
`⚠ unsupported` marks a node the Redacto output cannot represent (a field, image, conditional or \
repeatable) — those are dropped from the dump, so remove them deliberately or restructure them into \
text. Use replace_structured_node to change a node's type or level, insert_structured_node / \
remove_structured_node to add or drop nodes.\n\
5. Build & validate: build_redacto_dump generates the PostgreSQL dump and reports the languages, \
the per-table row counts, `problems` and `warnings`. Run it after every substantive change. A \
`problem` means the dump is not shippable — no text assets at all, or a language missing its \
variants — and MUST be resolved. A `warning` means content was dropped on the way into the dump; \
investigate every one.\n\
6. Review end to end: review_redacto_output compares the source against the text that actually \
reaches the generated dump and lists anything missing, with a coverage score. For EVERY miss, fix \
it and rebuild, or satisfy yourself it was an intentional drop; it compares the master language \
only, so spot-check the others with search_xfa. Then confirm the result is analogous, not merely \
complete: compare against the source page images (get_plain_state_image) and check that the section \
order, heading levels, lists and column layout resemble the original.\n\n\
Never invent text content: take all headings, body text and footnotes verbatim from the source, and \
never write copy of your own. The final document must contain EVERY language present in the source \
(get_source_info lists them) and ONLY those: never drop a language the source contains, and never \
invent a translation for a language it does not. A non-master language whose text merely repeats \
the master-language text is an untranslated stub, not a translation. Keep tool inputs minimal and \
valid JSON.";

/// Redacto Analyst role: read-only source analysis → a conversion plan.
pub const REDACTO_ANALYST_ADDENDUM: &str = "\
ROLE: Analyst. You do NOT edit the document. Produce ONE detailed CONVERSION PLAN that lets the \
Author build the Redacto document without re-reading the bulky source. Inspect exhaustively: \
get_source_info (the authority on which languages the source has), list_states + explore_states, \
get_plain_state_image / get_annotated_state_image, get_xfa / search_xfa, and \
get_flattened_structure_for_state for EVERY state. The plan must give, per top-level SECTION in \
source order: its role (heading / body text / list / table / footnote block / multi-column region); \
its heading level; and, crucially, HOW THE LANGUAGES LINE UP — state which state label carries each \
language, whether their block structures correspond one-to-one, and call out every place they do \
NOT. Those mismatches are the entire difficulty of this conversion: the automatic merger cannot \
resolve them, which is why the Author pairs the languages by hand. Also record: any footnote \
markers and the text they refer to; any multi-column section; and whether the source carries \
fillable fields (a Redacto document cannot represent them, so the Author must be told). Your final \
message IS the plan — make it complete and self-contained; the Author works from it, not by \
re-reading the source.";

/// Redacto Author role: appended AFTER [`REDACTO_SYSTEM_PROMPT`].
/// Mirrors [`AUTHOR_ADDENDUM`]; the "do NOT call finish" contract is what the
/// controller's review loop depends on and is copied verbatim in substance.
pub const REDACTO_AUTHOR_ADDENDUM: &str = "\
STAGE NOTE: A CONVERSION PLAN produced by an Analyst is appended below as your section / language \
map — trust it and use search_xfa only to fill specific gaps rather than re-dumping the whole \
source. A separate Reviewer judges fidelity after you, so do NOT call finish; once you have seeded \
the tree, layered in every language and run build_redacto_dump with no problems reported, stop with \
a short summary. If REVIEW FEEDBACK appears below, address EVERY point from every round, then \
rebuild and re-validate.";

/// Redacto Reviewer role: independent fidelity judgement.
pub const REDACTO_REVIEWER_ADDENDUM: &str = "\
ROLE: Reviewer. You do NOT edit the document — you judge the Author's result and report. Verify \
independently: run build_redacto_dump (every `problem` is disqualifying; every `warning` means \
content was dropped) and review_redacto_output (investigate every missing text and the coverage \
score). Read the document with get_structured_outline and resolve every `⚠` flag: `⚠ unsupported` \
means content will be dropped from the dump, and a text present in only one language when the \
source has several is an untranslated stub. Compare against the source page images \
(get_plain_state_image) to confirm the section order, heading levels, lists and multi-column \
layout are analogous — not merely that the text is present. Spot-check non-master languages with \
search_xfa, since review_redacto_output compares the master language only. End by calling \
submit_review with approved=true ONLY if the dump has no problems and every remaining issue is \
resolved; otherwise approved=false and report = a detailed, actionable message listing every issue \
with node paths where possible. Do not fix anything yourself.";

/// The result of executing one tool call, to be returned to the model as a
/// `tool_result` content block.
pub enum ToolReply {
    /// A textual result (JSON, plain text, …).
    Text(String),
    /// One or more images (base64), all sharing one media type — e.g. the pages
    /// of a rendered form. Emitted as multiple image blocks in one `tool_result`.
    Image {
        media_type: &'static str,
        images: Vec<String>,
    },
    /// The tool failed; the message is surfaced to the model as an error result.
    Error(String),
}

/// The outcome of the Reviewer role's `submit_review` call: whether the form is
/// approved, and (if not) a detailed report the controller pins into the Author's
/// next system prompt.
#[derive(Debug, Clone)]
pub struct ReviewResult {
    pub approved: bool,
    pub report: String,
}

/// Settings key under which the desktop app persists its serialized settings
/// blob in the shared `history.db` (see `app`'s `AppSettings`).
const APP_SETTINGS_KEY: &str = "app";

/// Build an AEM connection from the app settings stored in the shared
/// `history.db`, so a conversion driven headlessly (e.g. over MCP) can
/// upload/verify against the same instance the desktop app is configured for.
///
/// Reads the `aem_host` / `aem_username` / `aem_password` fields out of the
/// settings blob — mirroring `AppSettings::aem_connection` — and returns `None`
/// when no settings are stored or host/username are blank.
pub fn aem_connection_from_settings() -> Option<AemConnection> {
    let json = crate::db::get_setting(APP_SETTINGS_KEY)?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let host = v
        .get("aem_host")
        .and_then(|h| h.as_str())
        .unwrap_or_default()
        .trim();
    let username = v
        .get("aem_username")
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .trim();
    if host.is_empty() || username.is_empty() {
        return None;
    }
    let password = v
        .get("aem_password")
        .and_then(|p| p.as_str())
        .unwrap_or_default();
    Some(AemConnection {
        host: host.trim_end_matches('/').to_string(),
        username: username.to_string(),
        password: password.to_string(),
    })
}

/// Validate FileVault package bytes (session-agnostic).
///
/// Runs the same checks as the `validate_aem_package` tool: required FileVault
/// structure, form `.content.xml` (`cq:Page`) validation, and DAM
/// `.content.xml` (`dam:Asset`) validation. Returns `Ok(success message)` when
/// the package is valid, or `Err(problem report)` listing every violation.
pub fn validate_package_bytes(pkg: &[u8]) -> Result<String, String> {
    let files = crate::references::unzip_package(pkg)
        .map_err(|e| format!("Could not read package: {e}"))?;

    let mut problems: Vec<String> = Vec::new();

    // 1. Required FileVault package structure.
    const REQUIRED: &[&str] = &[
        "META-INF/MANIFEST.MF",
        "META-INF/vault/config.xml",
        "META-INF/vault/nodetypes.cnd",
        "META-INF/vault/filter.xml",
        "META-INF/vault/properties.xml",
        "META-INF/vault/definition/.content.xml",
        "jcr_root/.content.xml",
        "jcr_root/content/.content.xml",
        "jcr_root/content/forms/.content.xml",
        "jcr_root/content/forms/af/.content.xml",
        "jcr_root/content/dam/.content.xml",
        "jcr_root/content/dam/formsanddocuments/.content.xml",
    ];
    for path in REQUIRED {
        if !files.iter().any(|(p, _)| p == path) {
            problems.push(format!("missing required package entry: {path}"));
        }
    }

    // 2. Validate the form content XML (the cq:Page under forms/af).
    let form_xml = files.iter().find(|(p, c)| {
        p.starts_with("jcr_root/content/forms/af/")
            && p.ends_with("/.content.xml")
            && c.contains("\"cq:Page\"")
    });
    match form_xml {
        Some((path, xml)) => {
            if let Err(violations) = blueprint::validate_aem_form_xml(xml) {
                problems.push(format!(
                    "form {path} failed {} validation check(s):\n    - {}",
                    violations.len(),
                    violations.join("\n    - ")
                ));
            }
        }
        None => problems.push(
            "no form .content.xml (jcr:primaryType cq:Page) found under \
             jcr_root/content/forms/af/"
                .into(),
        ),
    }

    // 3. Validate the DAM content XML (the dam:Asset).
    let dam_xml = files.iter().find(|(p, c)| {
        p.starts_with("jcr_root/content/dam/formsanddocuments/")
            && p.ends_with("/.content.xml")
            && c.contains("\"dam:Asset\"")
    });
    match dam_xml {
        Some((path, xml)) => {
            if let Err(violations) = blueprint::validate_aem_dam_xml(xml) {
                problems.push(format!(
                    "DAM {path} failed {} validation check(s):\n    - {}",
                    violations.len(),
                    violations.join("\n    - ")
                ));
            }
        }
        None => problems.push(
            "no DAM .content.xml (jcr:primaryType dam:Asset) found under \
             jcr_root/content/dam/formsanddocuments/"
                .into(),
        ),
    }

    if problems.is_empty() {
        Ok(format!(
            "✓ Package valid: {} entries; required FileVault structure present; \
             form and DAM content XML pass AEM validation.",
            files.len()
        ))
    } else {
        Err(format!(
            "Package validation found {} problem(s):\n- {}",
            problems.len(),
            problems.join("\n- ")
        ))
    }
}

// ── Per-source extraction (sync; cached) ─────────────────────────────────────

struct StateRec {
    label: String,
    pdf_name: String,
    selections: usize,
    state: blueprint::FormState,
    context: Context,
}

/// The engine's view of one input source (the uploaded form, or a reference):
/// discovered states (for listing / rendering / per-state structure), the XFA,
/// and the merged structured tree.
struct Extractor {
    states: Vec<StateRec>,
    xfa: Vec<(String, String)>,
    /// The merged multilingual envelope — content *and* context. The context is
    /// the only carrier of [`Context::header`], which `Blueprint::context()`
    /// never sets (only `merged_structured()` does), so consumers that want the
    /// recovered master-page header must read it from here.
    merged: DocumentEnvelope,
    /// Why the cross-language merge failed, if it did (`merged.content` is empty
    /// in that case). Swallowing this is what produced silently empty Redacto
    /// dumps: a document whose language variants are too dissimilar to merge
    /// looked exactly like a document with no content.
    merge_error: Option<String>,
}

impl Extractor {
    /// `semantic` is the sentence-embedding matcher used to align nodes across
    /// languages when merging the per-PDF (per-language) trees into one
    /// bilingual tree. Without it `merge_translations` can only align nodes
    /// structurally, which duplicates whole sections (one per language, with
    /// colliding field names) whenever the language variants don't line up
    /// node-for-node — so always pass it for a multi-language source.
    fn build(
        pdfs: &[(String, Vec<u8>)],
        semantic: Option<&blueprint::semantic::SemanticMatcher>,
    ) -> Self {
        let multi = pdfs.len() > 1;
        let mut states = Vec::new();
        let mut xfa = Vec::new();
        let mut envelopes: Vec<DocumentEnvelope> = Vec::new();

        for (name, bytes) in pdfs {
            if let Ok(Some(x)) = blueprint::extract_xfa_from_pdf_bytes(bytes) {
                xfa.push((name.clone(), String::from_utf8_lossy(&x).into_owned()));
            }
            if let Ok(mut bp) = blueprint::Blueprint::from_pdf_bytes(bytes) {
                let context = bp.context();
                if let Ok(fs) = bp.states() {
                    for s in fs.iter() {
                        let label = if multi {
                            format!("{name}::{}", s.label)
                        } else {
                            s.label.clone()
                        };
                        let selections = s.selections.len();
                        states.push(StateRec {
                            label,
                            pdf_name: name.clone(),
                            selections,
                            state: s,
                            context: context.clone(),
                        });
                    }
                }
            }
            // Merged structured needs its own Blueprint (states()/merged both &mut).
            if let Ok(mut bp2) = blueprint::Blueprint::from_pdf_bytes(bytes)
                && let Ok(env) = bp2.merged_structured()
            {
                envelopes.push(env);
            }
        }

        // Each single-PDF `merged_structured()` succeeds even when the
        // cross-language merge does not, so keep the first envelope's context as
        // the base: it carries the recovered master-page header, which would
        // otherwise be lost exactly when the merge fails.
        let base_context = envelopes
            .first()
            .map(|e| e.context.clone())
            .unwrap_or_else(|| Context::with_language("en"));
        let empty = |context: Context| DocumentEnvelope {
            context,
            content: Vec::new(),
            state_count: 1,
        };

        let (merged, merge_error) = match envelopes.len() {
            0 => (empty(base_context), None),
            1 => (envelopes.into_iter().next().unwrap(), None),
            _ => match blueprint::merge_translations(envelopes, semantic) {
                Ok(env) => (env, None),
                Err(e) => (empty(base_context), Some(e.to_string())),
            },
        };

        Extractor {
            states,
            xfa,
            merged,
            merge_error,
        }
    }

    fn find(&self, label: &str) -> Option<&StateRec> {
        self.states.iter().find(|s| s.label == label)
    }

    /// The clean, single-language structured tree for one state (one language ×
    /// one configurator selection). Unlike the merged tree this carries no
    /// cross-language/cross-state merge artifacts (no duplicated sections,
    /// colliding field names or mispaired translations) — it's the engine's
    /// faithful read of exactly one rendered variant, suitable as a base to
    /// assemble the working tree from.
    fn state_structured(&self, label: &str) -> Result<Vec<StructuredNode>, String> {
        let rec = self
            .find(label)
            .ok_or_else(|| format!("No state with label '{label}'. Use list_states."))?;
        Ok(rec.state.structured(rec.context.clone()).content)
    }
}

// ── The agent ────────────────────────────────────────────────────────────────

/// Everything a run aimed at [`OutputTarget::Aem`] accumulates.
#[derive(Default)]
struct AemState {
    config: Option<AemConfig>,
    /// The working multilingual AEM tree the agent authors directly. Lowered to
    /// `(AemNode, translations)` at build/review time.
    tree: Option<AemNodeTranslated>,
    package: Option<Vec<u8>>,
    /// The derived `#aem` edit-history session id, once anything is snapshotted.
    session: Option<String>,
    /// Set once the package has been uploaded + installed on AEM.
    uploaded: bool,
    /// JCR path of the uploaded form on AEM (for the "done" screen).
    form_path: Option<String>,
}

/// Everything a run aimed at [`OutputTarget::Redacto`] accumulates.
///
/// The authored document itself lives in [`ConversionAgent::structured`], which
/// both targets share; this is only what building the dump produces.
#[derive(Default)]
struct RedactoState {
    /// The dump from the most recent `build_redacto_dump`, reused by `finalize`
    /// so the shipped SQL is the one the agent last saw validated.
    dump: Option<RedactoDump>,
}

/// The per-target half of the agent's state.
///
/// Splitting it makes an AEM tool structurally unreachable in a Redacto run
/// rather than merely un-offered: the app filters tools by role name, but MCP
/// serves the flat catalog, so the guarantee has to live here.
enum TargetState {
    Aem(Box<AemState>),
    Redacto(RedactoState),
}

impl TargetState {
    fn new(target: OutputTarget) -> Self {
        match target {
            OutputTarget::Aem => TargetState::Aem(Box::default()),
            OutputTarget::Redacto => TargetState::Redacto(RedactoState::default()),
        }
    }

    fn target(&self) -> OutputTarget {
        match self {
            TargetState::Aem(_) => OutputTarget::Aem,
            TargetState::Redacto(_) => OutputTarget::Redacto,
        }
    }

    fn aem(&self) -> Option<&AemState> {
        match self {
            TargetState::Aem(state) => Some(state),
            TargetState::Redacto(_) => None,
        }
    }

    fn aem_mut(&mut self) -> Option<&mut AemState> {
        match self {
            TargetState::Aem(state) => Some(state),
            TargetState::Redacto(_) => None,
        }
    }

    fn redacto_mut(&mut self) -> Option<&mut RedactoState> {
        match self {
            TargetState::Redacto(state) => Some(state),
            TargetState::Aem(_) => None,
        }
    }
}

pub struct ConversionAgent {
    profile: Option<String>,
    context: Context,
    conn: Option<AemConnection>,
    current_pdfs: Vec<(String, Vec<u8>)>,
    extractors: HashMap<String, Extractor>,

    /// The working structured tree. Under [`OutputTarget::Redacto`] this is what
    /// the agent authors and the dump is generated from; under
    /// [`OutputTarget::Aem`] it stays empty (the agent authors the AEM tree
    /// directly) and only feeds `config()`'s language detection when a resumed
    /// session seeded it.
    structured: Vec<StructuredNode>,

    /// State belonging to the output target this run aims at.
    target: TargetState,

    structured_session: String,

    /// Sentence-embedding model backing semantic `search_references`. Loaded
    /// lazily on first use (~200ms) and reused for the rest of the run.
    matcher: Option<blueprint::semantic::SemanticMatcher>,

    finished: bool,

    /// The Reviewer role's latest `submit_review` outcome, drained by the
    /// controller via [`take_review`](Self::take_review).
    review: Option<ReviewResult>,
}

impl ConversionAgent {
    /// `files` may mix source PDFs and a single AEM content-package ZIP. The
    /// PDFs are the conversion source; the ZIP (if any) is parsed into an
    /// `AemNodeTranslated` and pre-loaded as the working tree, acting as an
    /// editable template the agent modifies instead of authoring from scratch.
    ///
    /// `target` fixes what the run produces, and with it which half of the
    /// agent's state exists at all: an uploaded template is only meaningful for
    /// [`OutputTarget::Aem`] and is ignored otherwise.
    pub fn new(
        profile: Option<String>,
        files: Vec<(String, Vec<u8>)>,
        conn: Option<AemConnection>,
        structured_session: String,
        target: OutputTarget,
    ) -> Self {
        let pdfs: Vec<(String, Vec<u8>)> = files
            .iter()
            .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".pdf"))
            .cloned()
            .collect();

        // First AEM content-package ZIP, parsed once for both the template tree
        // and (for template-only runs) the document context/language.
        let template_bp = files
            .iter()
            .find(|(_, b)| blueprint::aem::detect_aem_zip(b))
            .and_then(|(_, b)| blueprint::Blueprint::from_aem_zip(b).ok());

        let context = pdfs
            .iter()
            .find_map(|(_, b)| {
                blueprint::Blueprint::from_pdf_bytes(b)
                    .ok()
                    .map(|bp| bp.context())
            })
            .or_else(|| template_bp.as_ref().map(|bp| bp.context()))
            .unwrap_or_else(|| Context::with_language("en"));

        let mut target_state = TargetState::new(target);
        if let Some(aem) = target_state.aem_mut() {
            aem.tree = template_bp.as_ref().and_then(|bp| bp.aem_translated());
        }

        let mut agent = Self {
            profile,
            context,
            conn,
            current_pdfs: pdfs,
            extractors: HashMap::new(),
            structured: Vec::new(),
            target: target_state,
            structured_session,
            matcher: None,
            finished: false,
            review: None,
        };
        // Record the pre-loaded template as the initial AEM edit so it shows in
        // the AEM edit history (no-op when no template was uploaded).
        if agent.aem_tree().is_some() {
            agent.aem_translated_edited("Template (from uploaded package)");
        }
        agent
    }

    // ── Target-state access ──────────────────────────────────────────────────

    /// The output target this run aims at.
    pub fn target(&self) -> OutputTarget {
        self.target.target()
    }

    /// The working AEM tree, if this is an AEM run that has one.
    fn aem_tree(&self) -> Option<&AemNodeTranslated> {
        self.target.aem().and_then(|s| s.tree.as_ref())
    }

    /// Mutable access to the working AEM tree, if this is an AEM run with one.
    fn aem_tree_mut(&mut self) -> Option<&mut AemNodeTranslated> {
        self.target.aem_mut().and_then(|s| s.tree.as_mut())
    }

    /// Lazily load (and cache) the sentence-embedding model used by semantic
    /// `search_references`.
    fn matcher(&mut self) -> Result<&blueprint::semantic::SemanticMatcher, String> {
        if self.matcher.is_none() {
            self.matcher =
                Some(blueprint::semantic::SemanticMatcher::new().map_err(|e| e.to_string())?);
        }
        Ok(self.matcher.as_ref().unwrap())
    }

    /// Seed the working structured tree (used when resuming a session to apply
    /// user feedback to a prior result).
    pub fn seed_structured(&mut self, nodes: Vec<StructuredNode>) {
        self.structured = nodes;
    }

    /// Seed the working AEM tree from a restored session, so feedback is applied
    /// to the tree the previous run actually authored instead of re-deriving one
    /// from the source.
    ///
    /// Deliberately does *not* snapshot: the tree came out of the history, and
    /// re-recording it would add a no-op entry to every resumed session.
    pub fn seed_aem_translated(&mut self, tree: AemNodeTranslated) {
        if let Some(aem) = self.target.aem_mut() {
            aem.tree = Some(tree);
            aem.package = None;
        }
    }

    // ── Public accessors (for the driving loop's result finalization) ─────────

    /// `true` once the agent has called the `finish` tool.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Drain the Reviewer role's latest `submit_review` outcome (the controller
    /// reads this after running the Reviewer stage).
    pub fn take_review(&mut self) -> Option<ReviewResult> {
        self.review.take()
    }

    /// The detected document context (language, …).
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// The current working structured tree.
    ///
    /// Empty on a fresh run: the agent authors the AEM tree directly and only
    /// seeds this when resuming a session. Use
    /// [`source_structured`](Self::source_structured) for the converted source
    /// document.
    pub fn structured(&self) -> &[StructuredNode] {
        &self.structured
    }

    /// The merged structured tree of the current source PDFs — the plain
    /// conversion of the document, identical to what the CLI produces.
    ///
    /// Builds and caches the extractor if the run has not needed it yet, so
    /// this is free once the agent has read the source (the usual case) and a
    /// full conversion otherwise. Returns an empty slice if extraction fails.
    pub fn source_structured(&mut self) -> &[StructuredNode] {
        match self.extractor(&serde_json::json!({})) {
            Ok(extractor) => &extractor.merged.content,
            Err(_) => &[],
        }
    }

    /// The merged source [`DocumentEnvelope`] — [`source_structured`](Self::source_structured)
    /// plus the context it was extracted with.
    ///
    /// Prefer this over pairing `source_structured()` with
    /// [`context`](Self::context) when building an output: only this context
    /// carries [`Context::header`], the master-page header the analysis
    /// recovers. `ConversionAgent::context` is taken from `Blueprint::context()`
    /// before any analysis has run and always has `header: None`.
    pub fn source_envelope(&mut self) -> DocumentEnvelope {
        match self.extractor(&serde_json::json!({})) {
            Ok(extractor) => extractor.merged.clone(),
            Err(_) => DocumentEnvelope {
                context: self.context.clone(),
                content: Vec::new(),
                state_count: 1,
            },
        }
    }

    /// Why the source's cross-language merge failed, if it did.
    ///
    /// A `Some` here means [`source_structured`](Self::source_structured) is
    /// empty for a reason worth reporting rather than because the document has
    /// no content.
    pub fn source_merge_error(&mut self) -> Option<String> {
        match self.extractor(&serde_json::json!({})) {
            Ok(extractor) => extractor.merge_error.clone(),
            Err(_) => None,
        }
    }

    /// The working AEM (translated) tree — what the agent actually authored.
    ///
    /// This is the run's real product: the structured tree stays empty, so any
    /// consumer that wants the authored document (the editors, the recorded
    /// snapshot) must go through here rather than [`structured`](Self::structured).
    pub fn aem_translated(&self) -> Option<&AemNodeTranslated> {
        self.aem_tree()
    }

    /// The most recently built AEM package (ZIP), if any.
    pub fn package(&self) -> Option<Vec<u8>> {
        self.target.aem().and_then(|s| s.package.clone())
    }

    /// The resolved form code, if the AEM config has been loaded.
    pub fn form_code(&self) -> Option<String> {
        self.target
            .aem()
            .and_then(|s| s.config.as_ref())
            .map(|c| c.form_code.clone())
    }

    /// The derived AEM edit-history session id, if any AEM snapshot was taken.
    pub fn aem_session(&self) -> Option<String> {
        self.target.aem().and_then(|s| s.session.clone())
    }

    /// Whether the package has been uploaded + installed on AEM.
    pub fn aem_uploaded(&self) -> bool {
        self.target.aem().is_some_and(|s| s.uploaded)
    }

    /// The JCR path of the uploaded form, once uploaded.
    pub fn aem_form_path(&self) -> Option<String> {
        self.target.aem().and_then(|s| s.form_path.clone())
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn source_key(input: &serde_json::Value) -> String {
        match input["source"]["reference"].as_str() {
            Some(id) => format!("reference:{id}"),
            None => "current".to_string(),
        }
    }

    /// Get (building+caching if needed) the extractor for the requested source.
    fn extractor(&mut self, input: &serde_json::Value) -> Result<&Extractor, String> {
        let key = Self::source_key(input);
        if !self.extractors.contains_key(&key) {
            let pdfs = match input["source"]["reference"].as_str() {
                Some(id) => {
                    let bytes = crate::references::get_reference_pdf_bytes(id, 0)?;
                    vec![(format!("{id}.pdf"), bytes)]
                }
                None => self.current_pdfs.clone(),
            };
            // A multi-language source must be merged with the semantic matcher
            // (see Extractor::build). Load it best-effort; if it can't load we
            // fall back to None and the structural merge. Single-PDF sources
            // need no cross-language merge, so don't pay the load cost.
            let ex = if pdfs.len() > 1 {
                let _ = self.matcher();
                Extractor::build(&pdfs, self.matcher.as_ref())
            } else {
                Extractor::build(&pdfs, None)
            };
            self.extractors.insert(key.clone(), ex);
        }
        Ok(self.extractors.get(&key).unwrap())
    }

    /// The resolved AEM configuration. AEM-only by construction: a Redacto run
    /// has no `AemState` to cache it on, and its language resolution (which
    /// prefers the AEM profile's list) is the wrong answer for a Redacto
    /// document — that uses `resolve_redacto_languages` instead.
    fn config(&mut self) -> Result<AemConfig, String> {
        let cached = self
            .target
            .aem()
            .ok_or(AEM_ONLY_STATE)?
            .config
            .clone();
        let cfg = match cached {
            Some(cfg) => cfg,
            None => {
                let p = self
                    .profile
                    .clone()
                    .ok_or("No profile selected — AEM conversion needs a profile.")?;
                let loaded = blueprint::load_aem_config(&p, &self.context)?;
                if let Some(aem) = self.target.aem_mut() {
                    aem.config = Some(loaded.clone());
                }
                loaded
            }
        };
        // Reflect the languages actually present in the document so
        // get_profile_info and the package builder never misreport a
        // multilingual form as en-only. `resolve_aem_languages` only overrides
        // when it detects ≥1 language, so monolingual flows keep the default.
        // Resolved per-call (not cached) because set_structured mutates
        // self.structured without touching the cached config. Prefer the working
        // tree once seeded; otherwise fall back to the merged source extraction
        // so the languages are reported even before the tree is seeded.
        let mut cfg = if !self.structured.is_empty() {
            blueprint::resolve_aem_languages(&self.structured, &cfg)
        } else if let Ok(ex) = self.extractor(&serde_json::Value::Null) {
            blueprint::resolve_aem_languages(&ex.merged.content, &cfg)
        } else {
            cfg
        };
        // Carry any languages present in the working tree (e.g. a pre-loaded
        // template) into the config so they survive lowering — important for
        // template-only runs where there is no PDF source to detect them from.
        if let Some(tree) = self.aem_tree() {
            for lang in collect_translated_languages(tree) {
                if !cfg.languages.contains(&lang) {
                    cfg.languages.push(lang);
                }
            }
        }
        Ok(cfg)
    }


    /// Snapshot the working AEM (translated) tree for versioning.
    fn snapshot_aem_translated(&mut self, label: &str) {
        let derived_session = format!("{}#aem", self.structured_session);
        let Some(aem) = self.target.aem_mut() else {
            return;
        };
        let Some(ref tree) = aem.tree else {
            return;
        };
        let Ok(json) = serde_json::to_string(tree) else {
            return;
        };
        let sid = aem.session.get_or_insert(derived_session).clone();
        crate::db::insert_edit(&sid, label, &json);
    }

    /// Common tail of every structured-tree edit: invalidate the built dump,
    /// then snapshot the tree into the edit history.
    ///
    /// Recording into `structured_session` (the primary session, not the derived
    /// `#aem` one) is what makes a Redacto run reopenable: `session::restore`
    /// already prefers a non-empty structured snapshot, so it needs no changes.
    fn structured_edited(&mut self, label: &str) {
        if let Some(redacto) = self.target.redacto_mut() {
            redacto.dump = None;
        }
        if let Ok(json) = serde_json::to_string(&self.structured) {
            crate::db::insert_edit(&self.structured_session, label, &json);
        }
    }

    /// Build the Redacto dump for the working structured tree, and cache it.
    ///
    /// The context comes from the merged source envelope rather than
    /// [`context`](Self::context) so the recovered master-page header reaches
    /// the profile's `page.header`.
    fn build_redacto(&mut self) -> Result<(RedactoDump, blueprint::RedactoConfig), String> {
        let profile = self
            .profile
            .clone()
            .ok_or("No profile selected — the Redacto dump needs a profile.")?;
        let ctx = self.source_envelope().context;
        let (dump, config) =
            blueprint::to_redacto_dump_for_profile(&profile, &ctx, &self.structured)?;
        if let Some(redacto) = self.target.redacto_mut() {
            redacto.dump = Some(dump.clone());
        }
        Ok((dump, config))
    }

    /// The dump from the most recent `build_redacto_dump`, if one succeeded.
    pub fn redacto_dump(&self) -> Option<&RedactoDump> {
        match &self.target {
            TargetState::Redacto(state) => state.dump.as_ref(),
            TargetState::Aem(_) => None,
        }
    }

    /// Common tail of every AEM-tree edit: invalidate the package, then snapshot.
    fn aem_translated_edited(&mut self, label: &str) {
        if let Some(aem) = self.target.aem_mut() {
            aem.package = None;
        }
        self.snapshot_aem_translated(label);
    }

    /// Lower the working multilingual tree to the single-language `AemNode` plus
    /// the master-text-keyed translation dictionary the package writer consumes.
    fn lower_aem_translated(&mut self) -> Result<(AemNode, I18nDict), String> {
        let cfg = self.config()?;
        let tree = self.aem_tree().ok_or(NO_AEM_TREE)?;
        Ok(tree.lower(&cfg.master_language, &cfg.languages))
    }

    // ── Tool definitions ───────────────────────────────────────────────────────

    pub fn tools(&self) -> Vec<serde_json::Value> {
        let source = serde_json::json!({
            "source": {
                "type": "object",
                "description": "Optional: which input to read. Omit for the uploaded form, or {\"reference\": \"<ref_id>\"} to run the engine on a reference's input.",
                "properties": { "reference": { "type": "string" } }
            }
        });
        let with_source = |props: serde_json::Value| {
            let mut m = props.as_object().cloned().unwrap_or_default();
            m.insert("source".to_string(), source["source"].clone());
            serde_json::Value::Object(m)
        };
        let t = |name: &str, desc: &str, props: serde_json::Value, required: serde_json::Value| {
            serde_json::json!({
                "name": name, "description": desc,
                "input_schema": { "type": "object", "properties": props, "required": required }
            })
        };
        let state_label = serde_json::json!({ "state_label": {"type": "string", "description": "A label from list_states."} });

        vec![
            // §1 extraction (source-parameterized)
            t(
                "get_source_info",
                "Info about the source PDFs (name, language, state count).",
                with_source(serde_json::json!({})),
                serde_json::json!([]),
            ),
            t(
                "explore_states",
                "Run exhaustive state discovery on the source; returns a count summary.",
                with_source(serde_json::json!({})),
                serde_json::json!([]),
            ),
            t(
                "list_states",
                "List discovered form states (label, pdf, selection count).",
                with_source(serde_json::json!({})),
                serde_json::json!([]),
            ),
            t(
                "get_xfa",
                "Return the source's authoritative XFA XML (all PDFs concatenated).",
                with_source(serde_json::json!({})),
                serde_json::json!([]),
            ),
            t(
                "search_xfa",
                "Regex/substring search within the source's XFA; returns matching snippets.",
                with_source(
                    serde_json::json!({"query": {"type":"string"}, "regex": {"type":"boolean"}}),
                ),
                serde_json::json!(["query"]),
            ),
            t(
                "get_plain_state_image",
                "Render a state's page image (plain).",
                with_source(state_label.clone()),
                serde_json::json!(["state_label"]),
            ),
            t(
                "get_annotated_state_image",
                "Render a state's page image with field-name overlays.",
                with_source(state_label.clone()),
                serde_json::json!(["state_label"]),
            ),
            t(
                "get_flattened_structure_for_state",
                "The engine's clean structured tree for ONE state (one language × one configurator selection). Carries no merge artifacts — no duplicated sections, colliding field names or mispaired translations. This is the building block you assemble the working tree from: inspect each state, compare against its page image and XFA, then seed from one and layer in the rest.",
                with_source(state_label.clone()),
                serde_json::json!(["state_label"]),
            ),
            // §2a structured tree (Redacto target) — seeded, then refined.
            t(
                "seed_structured_from_state",
                "Load the engine's clean structured tree for ONE state as the working tree, replacing whatever is there. START HERE: the engine already got the block structure, the inline markup, the list nesting, the footnote markers and the multi-column sections right for that state — you only have to add the OTHER languages to each node. Far cheaper and far more faithful than emitting the tree yourself with set_structured. Pick the state in the master language, then layer in the rest with set_structured_field.",
                state_label.clone(),
                serde_json::json!(["state_label"]),
            ),
            t(
                "set_structured",
                "Set the WHOLE working structured tree as a JSON array of StructuredNode (call get_schema('structured') for the exact shape). Rarely needed: prefer seed_structured_from_state followed by targeted edits, which cannot silently drop a node or a language.",
                serde_json::json!({"nodes": {"type":"array"}}),
                serde_json::json!(["nodes"]),
            ),
            t(
                "get_structured_outline",
                "Map the working structured tree: one line per node — `<path>  <type> <summary>  <flags>`. Flags: `⚠ text?` / `⚠ label?` (missing or placeholder text), `⚠ no-options` (empty choice list), `⚠ unsupported` (a node the Redacto output cannot represent: fields, images, conditionals, repeatables). Paths are `/`-separated walks from the top level, e.g. `0/children/2`, `5/rows/0/cells/1`.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "get_structured_node",
                "Return the node (its whole subtree) at `path` as JSON. Inspect it before editing to see the exact field shapes — in particular that every text is a per-language map like {\"de\":[…],\"en\":[…]}.",
                serde_json::json!({"path": {"type":"string"}}),
                serde_json::json!(["path"]),
            ),
            t(
                "set_structured_field",
                "Set one field of the node at `path`. `field` is a node key such as `content`, `level`, `label`, `items`, `columnFlow`; `value` is the raw JSON for it (match the shape from get_structured_node). This is how you add a language: read the node, then write back its `content` map with every language present. Validated by round-trip; a bad value is rejected and the tree left unchanged. Cannot change a node's `type` (use replace_structured_node).",
                serde_json::json!({"path": {"type":"string"}, "field": {"type":"string"}, "value": {}}),
                serde_json::json!(["path", "field", "value"]),
            ),
            t(
                "replace_structured_node",
                "Replace the whole node at `path` with `node`, a JSON object parseable as a StructuredNode (must include its `type`). Use to change a node's type or rebuild it.",
                serde_json::json!({"path": {"type":"string"}, "node": {"type":"object"}}),
                serde_json::json!(["path", "node"]),
            ),
            t(
                "insert_structured_node",
                "Insert `node` (a StructuredNode JSON object) into a child list. `parent_path` is empty/\"root\" for the top level, or the path of a Group. `position` is \"first\", \"last\", {\"before\":<i>} or {\"after\":<i>}.",
                serde_json::json!({"parent_path": {"type":"string"}, "node": {"type":"object"}, "position": {"type":["string","object"]}}),
                serde_json::json!(["parent_path", "node", "position"]),
            ),
            t(
                "remove_structured_node",
                "Remove the node at `path` from its list (top-level nodes and Group children only).",
                serde_json::json!({"path": {"type":"string"}}),
                serde_json::json!(["path"]),
            ),
            t(
                "build_redacto_dump",
                "Build the Redacto PostgreSQL dump from the working structured tree and report what it contains: languages, document id, per-table row counts, `problems` and `warnings`. Run it after every substantive change. A `problem` means the dump is not shippable (no text assets at all, a language missing its variants); a `warning` means content was dropped in translation to the Redacto model. Resolve every problem before you stop.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "review_redacto_output",
                "Fidelity review: compare the engine's parse of the source against the text that actually reaches the generated dump, and report input text with no match, plus a coverage score. Compares the master language only. Reviews the DUMP, not the working tree — that is the artefact that ships.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            // §2 multilingual AEM tree (AemNodeTranslated) — authored directly.
            t(
                "set_aem_translated",
                "Set the WHOLE working AEM tree as an AemNodeTranslated JSON object (call get_schema('aem_translated') for the exact shape). Use this for the initial authoring of the form; for small fixes afterwards use the targeted editors below. Text fields (title/label/content and option labels) are per-language maps like {\"de\":\"…\",\"en\":\"…\"}; include EVERY source language. Invalidates the package.",
                serde_json::json!({"root": {"type":"object"}}),
                serde_json::json!(["root"]),
            ),
            t(
                "get_aem_translated",
                "Return the current working AemNodeTranslated tree (JSON).",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "get_aem_translated_outline",
                "Map the working AEM tree: one line per node — `<path>  <Type>  [langs] \"excerpt\"  <flags>`. Flags: `⚠ empty` (text-bearing node with no text), `⚠ 1 lang` (only one language present — likely a missing translation). Use it to find the path to fix, then call the set/replace/insert/remove tools. Paths are `/`-separated child indices from the root (e.g. 2/0/3); `root`/empty addresses the root node.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "get_aem_translated_node",
                "Return just the node (its whole subtree) at `path` as JSON. Inspect it before editing to see the exact field shapes (e.g. how `label`/`options` are structured).",
                serde_json::json!({"path": {"type":"string"}}),
                serde_json::json!(["path"]),
            ),
            t(
                "set_aem_translated_field",
                "Set one field of the node at `path`. `field` is a node key such as `label`, `title`, `content`, `options`, `visible`, `mandatory`, `colspan`, `bind_ref`; `value` is the raw JSON for it (match the shape from get_aem_translated_node — text fields are per-language maps). Validated by round-trip; a bad value is rejected and the tree left unchanged. Cannot change a node's `type` (use replace_aem_translated_node). Invalidates the package.",
                serde_json::json!({"path": {"type":"string"}, "field": {"type":"string"}, "value": {}}),
                serde_json::json!(["path", "field", "value"]),
            ),
            t(
                "replace_aem_translated_node",
                "Replace the whole node at `path` with `node`, a JSON object parseable as an AemNodeTranslated (must include its `type`). Use to change a node's type or rebuild it. Invalidates the package.",
                serde_json::json!({"path": {"type":"string"}, "node": {"type":"object"}}),
                serde_json::json!(["path", "node"]),
            ),
            t(
                "insert_aem_translated_node",
                "Insert `node` (an AemNodeTranslated JSON object) into a child list. `parent_path` is empty/\"root\" for the root, or the path of a Panel or Repeatable (only those hold children). `position` is \"first\", \"last\", {\"before\":<i>} or {\"after\":<i>} (i = child index). Invalidates the package.",
                serde_json::json!({"parent_path": {"type":"string"}, "node": {"type":"object"}, "position": {"type":["string","object"]}}),
                serde_json::json!(["parent_path", "node", "position"]),
            ),
            t(
                "remove_aem_translated_node",
                "Remove the node at `path` from its parent's child list (the root cannot be removed). Invalidates the package.",
                serde_json::json!({"path": {"type":"string"}}),
                serde_json::json!(["path"]),
            ),
            // §5 output
            t(
                "build_aem_package",
                "Build the AEM FileVault package (ZIP) from the current AEM tree. Requires an AEM tree (run convert_structured_to_aem first). Stores it for upload/export.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "get_package_info",
                "Size and file list of the built package.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "read_package_file",
                "Read a file from the built package by path.",
                serde_json::json!({"path": {"type":"string"}}),
                serde_json::json!(["path"]),
            ),
            t(
                "validate_aem_package",
                "Validate the built package: checks the required FileVault structure (META-INF + jcr_root boilerplate) and validates the form and DAM .content.xml against the AEM contract (well-formedness, escaping, JCR/CQ/FD/Sling structure). Run after build_aem_package, before upload_to_aem.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "review_output",
                "Fidelity review: compare the input (the engine's merged structured parse) against the converted AEM tree and report input text/elements missing from the output, with a coverage score. Compares the master language only (spot-check other languages with search_xfa). Reads the AEM tree, so edits made only to the content XML are not reflected. Run after convert_structured_to_aem and before finish; investigate every miss (fix the tree, or confirm it was intentionally dropped) and re-run.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "generate_xsd",
                "Generate the XSD schema for the current structured tree.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "generate_html",
                "Generate an HTML preview of the current structured tree.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            // §6 deploy + verify
            t(
                "upload_to_aem",
                "Upload and install the built package on the configured AEM instance.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "fetch_aem_form_html",
                "Fetch the rendered Adaptive Form HTML from AEM (after upload) for verification.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "fetch_aem_dor_pdf",
                "Fetch the Document-of-Record PDF from AEM and view its first page.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            // §7 references
            t(
                "list_reference_forms",
                "List the profile's reference forms (hand-built, known-good worked examples). \
                 Consult references BEFORE building: they show the expected JCR structure, \
                 dictionary setup and DoR conventions for this profile's forms.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "search_references",
                "Semantic search for precedent forms by MEANING, not by name. The query must be a \
                 natural-language DESCRIPTION of the input you are building — the form's (or the \
                 current section's) purpose, the kinds of fields it contains and how they are \
                 grouped — NOT a form name or a single keyword. References are matched by embedding \
                 this description against each reference's stored description (a literal substring \
                 fallback over descriptions + package XML is folded in). Run this first (before \
                 building), section by section; each hit carries a ref_id to pass to \
                 get_reference_package / read_reference_file. Optional top_k caps hits per signal \
                 (default 3).",
                serde_json::json!({"query": {"type":"string"}, "top_k": {"type":"integer"}}),
                serde_json::json!(["query"]),
            ),
            t(
                "grep_references",
                "Literal/regex substring search over reference descriptions + AEM package XML — the \
                 grep counterpart to search_references. Use it to find a specific string (a field \
                 name, label, or AEM resource type) verbatim; use search_references when looking \
                 for a form that resembles your input by meaning.",
                serde_json::json!({"query": {"type":"string"}, "regex": {"type":"boolean"}}),
                serde_json::json!(["query"]),
            ),
            t(
                "read_reference_file",
                "Read a reference's description ('description') or a package file by path (get the \
                 path from get_reference_package). Use it to study how a known-good form was built \
                 and mirror its structure.",
                serde_json::json!({"ref_id": {"type":"string"}, "path": {"type":"string"}, "offset": {"type":"integer"}, "limit": {"type":"integer"}}),
                serde_json::json!(["ref_id", "path"]),
            ),
            t(
                "get_reference_package",
                "List the package files (known-good output) of a reference by its ref_id (from \
                 list_reference_forms / search_references), then read individual files with \
                 read_reference_file.",
                serde_json::json!({"ref_id": {"type":"string"}}),
                serde_json::json!(["ref_id"]),
            ),
            t(
                "list_reference_docs",
                "List the profile's reference documentation (.md/.txt).",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "read_reference_doc",
                "Read a reference documentation doc by id.",
                serde_json::json!({"doc_id": {"type":"string"}, "offset": {"type":"integer"}, "limit": {"type":"integer"}}),
                serde_json::json!(["doc_id"]),
            ),
            t(
                "grep_reference_docs",
                "Regex/substring search over reference documentation.",
                serde_json::json!({"query": {"type":"string"}, "regex": {"type":"boolean"}}),
                serde_json::json!(["query"]),
            ),
            // §8 control
            t(
                "get_schema",
                "Return the JSON schema for a working tree: 'aem_translated' (what set_aem_translated and the AEM editors take) or 'structured' (what set_structured and the structured editors take).",
                serde_json::json!({"kind": {"type":"string","enum":["aem_translated","structured"]}}),
                serde_json::json!(["kind"]),
            ),
            t(
                "get_profile_info",
                "Profile/AEM config: form_code, languages, JCR paths, binding flags.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "finish",
                "Terminal step — call this once, last, after the package is built, validated and reviewed (review_output) — and uploaded if an AEM connection is configured — to persist the structured + AEM trees + package as the result and end the run.",
                serde_json::json!({"summary": {"type":"string"}}),
                serde_json::json!([]),
            ),
            t(
                "submit_review",
                "Terminal REVIEW step (Reviewer role) — call once, last, after building/validating/reviewing. approved=true means the form is fully correct and ends the run; approved=false returns your detailed issue list to the author for a fix round.",
                serde_json::json!({
                    "approved": {"type": "boolean"},
                    "report": {"type": "string", "description": "When not approved: a detailed, actionable list of every issue, with node paths where possible."}
                }),
                serde_json::json!(["approved"]),
            ),
        ]
    }

    // ── Tool execution (async: some tools hit the network) ──────────────────────

    /// Why `name` cannot run under this run's output target, if it cannot.
    ///
    /// One guard for the whole AEM family, so a mis-targeted call says what is
    /// actually wrong instead of failing deeper down with something misleading
    /// like "No AEM tree yet".
    fn target_refusal(&self, name: &str) -> Option<String> {
        let wrong_target = (AEM_ONLY_TOOLS.contains(&name)
            && self.target.target() != OutputTarget::Aem)
            || (REDACTO_ONLY_TOOLS.contains(&name)
                && self.target.target() != OutputTarget::Redacto);
        wrong_target.then(|| {
            format!(
                "{name} is not available for the {} output target.",
                self.target.target().label()
            )
        })
    }

    pub async fn execute(&mut self, name: &str, input: &serde_json::Value) -> ToolReply {
        if let Some(refusal) = self.target_refusal(name) {
            return ToolReply::Error(refusal);
        }

        match name {
            // §1 extraction
            "get_source_info" => match self.extractor(input) {
                Ok(ex) => {
                    let langs: Vec<&str> = ex.states.iter().map(|s| s.context.language()).collect();
                    // Report the cross-language merge outcome: when it fails the
                    // engine's merged tree is empty, and any output derived from
                    // it would silently be empty too.
                    let merge = match &ex.merge_error {
                        Some(e) => format!("FAILED - {e}"),
                        None => "ok".to_string(),
                    };
                    ToolReply::Text(format!(
                        "states: {}, languages: {:?}, xfa_pdfs: {}, merge: {merge}",
                        ex.states.len(),
                        dedup(langs),
                        ex.xfa.len()
                    ))
                }
                Err(e) => ToolReply::Error(e),
            },
            "explore_states" => match self.extractor(input) {
                Ok(ex) => ToolReply::Text(format!("Discovered {} state(s).", ex.states.len())),
                Err(e) => ToolReply::Error(e),
            },
            "list_states" => match self.extractor(input) {
                Ok(ex) => {
                    let list: Vec<_> = ex
                        .states
                        .iter()
                        .map(|s| serde_json::json!({"label": s.label, "pdf": s.pdf_name, "selections": s.selections}))
                        .collect();
                    ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
                }
                Err(e) => ToolReply::Error(e),
            },
            "get_xfa" => match self.extractor(input) {
                Ok(ex) if ex.xfa.is_empty() => {
                    ToolReply::Error("No XFA present in the source.".into())
                }
                Ok(ex) => ToolReply::Text(
                    ex.xfa
                        .iter()
                        .map(|(n, x)| format!("BEGIN XFA ({n})\n{x}\nEND XFA ({n})"))
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                ),
                Err(e) => ToolReply::Error(e),
            },
            "search_xfa" => {
                let query = input["query"].as_str().unwrap_or_default().to_string();
                let regex = input["regex"].as_bool().unwrap_or(false);
                match self.extractor(input) {
                    Ok(ex) => {
                        let mut out = String::new();
                        for (n, x) in &ex.xfa {
                            for line in x.lines().filter(|l| line_matches(l, &query, regex)) {
                                out.push_str(&format!("{n}: {}\n", line.trim()));
                                if out.len() > 4000 {
                                    break;
                                }
                            }
                        }
                        if out.is_empty() {
                            ToolReply::Text("No matches.".into())
                        } else {
                            ToolReply::Text(out)
                        }
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_plain_state_image" | "get_annotated_state_image" => {
                let label = input["state_label"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let annotated = name == "get_annotated_state_image";
                match self.extractor(input) {
                    Ok(ex) => match ex.find(&label) {
                        Some(rec) => {
                            // Render one image per page so no single image exceeds
                            // the vision API's size limit on tall multi-page forms.
                            let pages = if annotated {
                                rec.state.render_annotated_pages(RENDER_SCALE)
                            } else {
                                rec.state.render_plain_pages(RENDER_SCALE)
                            };
                            match pages.map_err(|e| e.to_string()).and_then(|imgs| {
                                imgs.iter()
                                    .map(|i| {
                                        crate::image_encode::encode_rgba_to_jpeg(i, 82)
                                            .map(|jpeg| base64_encode(&jpeg))
                                            .map_err(|e| e.to_string())
                                    })
                                    .collect::<Result<Vec<String>, String>>()
                            }) {
                                Ok(images) => ToolReply::Image {
                                    media_type: "image/jpeg",
                                    images,
                                },
                                Err(e) => ToolReply::Error(format!("Render failed: {e}")),
                            }
                        }
                        None => ToolReply::Error(format!("Unknown state_label: {label:?}")),
                    },
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_flattened_structure_for_state" => {
                let label = input["state_label"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                match self.extractor(input) {
                    Ok(ex) => match ex.state_structured(&label) {
                        Ok(content) => ToolReply::Text(
                            serde_json::to_string_pretty(&content).unwrap_or_default(),
                        ),
                        Err(e) => ToolReply::Error(e),
                    },
                    Err(e) => ToolReply::Error(e),
                }
            }
            // §2a structured tree (Redacto target)
            "seed_structured_from_state" => {
                let label = input["state_label"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let seeded = match self.extractor(input) {
                    Ok(ex) => ex.state_structured(&label),
                    Err(e) => Err(e),
                };
                match seeded {
                    Ok(nodes) => {
                        let count = nodes.len();
                        self.structured = nodes;
                        self.structured_edited(&format!("AI: seed structured from {label}"));
                        ToolReply::Text(format!(
                            "OK — working structured tree seeded from '{label}' \
                             ({count} top-level nodes). Use get_structured_outline to \
                             review it, then add the other languages."
                        ))
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "set_structured" => {
                let v = input.get("nodes").cloned().unwrap_or_else(|| input.clone());
                match serde_json::from_value::<Vec<StructuredNode>>(v) {
                    Ok(nodes) => {
                        let count = nodes.len();
                        self.structured = nodes;
                        self.structured_edited("AI: set structured tree");
                        ToolReply::Text(format!("OK — working structured tree set ({count} top-level nodes)."))
                    }
                    Err(e) => ToolReply::Error(format!("Invalid StructuredNode JSON: {e}")),
                }
            }
            "get_structured_outline" => {
                if self.structured.is_empty() {
                    return ToolReply::Error(NO_STRUCTURED_TREE.into());
                }
                ToolReply::Text(crate::structured_edit::outline(&self.structured))
            }
            "get_structured_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                match crate::structured_edit::resolve_mut(&mut self.structured, &path) {
                    Ok(node) => {
                        ToolReply::Text(serde_json::to_string_pretty(node).unwrap_or_default())
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "set_structured_field" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let field = input["field"].as_str().unwrap_or_default().to_string();
                let value = input.get("value").cloned().unwrap_or(serde_json::Value::Null);
                let result =
                    crate::structured_edit::set_field(&mut self.structured, &path, &field, value);
                match result {
                    Ok(msg) => {
                        self.structured_edited(&format!("AI: set {field} on {path}"));
                        ToolReply::Text(msg)
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "replace_structured_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let node = input.get("node").cloned().unwrap_or(serde_json::Value::Null);
                let result = crate::structured_edit::replace_node(&mut self.structured, &path, node);
                match result {
                    Ok(msg) => {
                        self.structured_edited(&format!("AI: replace {path}"));
                        ToolReply::Text(msg)
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "insert_structured_node" => {
                let parent = input["parent_path"].as_str().unwrap_or_default().to_string();
                let node = input.get("node").cloned().unwrap_or(serde_json::Value::Null);
                let pos = match crate::structured_edit::parse_insert_pos(
                    input.get("position").unwrap_or(&serde_json::Value::Null),
                ) {
                    Ok(p) => p,
                    Err(e) => return ToolReply::Error(e),
                };
                let result =
                    crate::structured_edit::insert_node(&mut self.structured, &parent, node, pos);
                match result {
                    Ok(msg) => {
                        self.structured_edited(&format!("AI: insert into {parent}"));
                        ToolReply::Text(msg)
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "remove_structured_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let result = crate::structured_edit::remove_node(&mut self.structured, &path);
                match result {
                    Ok(msg) => {
                        self.structured_edited(&format!("AI: remove {path}"));
                        ToolReply::Text(msg)
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "build_redacto_dump" => {
                if self.structured.is_empty() {
                    return ToolReply::Error(NO_STRUCTURED_TREE.into());
                }
                match self.build_redacto() {
                    Ok((dump, config)) => {
                        let validation = blueprint::validate_dump(&dump, &config);
                        ToolReply::Text(
                            serde_json::to_string_pretty(&serde_json::json!({
                                "document_id": config.document_id,
                                "title": config.title,
                                "languages": config.languages,
                                "header": config.header,
                                "assets": validation.counts.assets,
                                "asset_versions": validation.counts.asset_versions,
                                "document_versions": validation.counts.document_versions,
                                "rows": validation.counts.rows,
                                "problems": validation.problems,
                                "warnings": validation.warnings,
                            }))
                            .unwrap_or_default(),
                        )
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "review_redacto_output" => {
                if self.structured.is_empty() {
                    return ToolReply::Error(NO_STRUCTURED_TREE.into());
                }
                let (dump, config) = match self.build_redacto() {
                    Ok(pair) => pair,
                    Err(e) => return ToolReply::Error(e),
                };
                let source = self.source_envelope().content;
                let report = blueprint::review_redacto(&source, &dump, &config.master_language);
                ToolReply::Text(serde_json::to_string_pretty(&report).unwrap_or_default())
            }
            // §2 multilingual AEM tree (AemNodeTranslated)
            "set_aem_translated" => {
                let v = input.get("root").cloned().unwrap_or_else(|| input.clone());
                match serde_json::from_value::<AemNodeTranslated>(v) {
                    Ok(node) => {
                        if let Some(aem) = self.target.aem_mut() {
                            aem.tree = Some(node);
                        }
                        self.aem_translated_edited("AI: set AEM (translated) tree");
                        ToolReply::Text("OK — working AEM tree set (package invalidated).".into())
                    }
                    Err(e) => ToolReply::Error(format!("Invalid AemNodeTranslated JSON: {e}")),
                }
            }
            "get_aem_translated" => match self.aem_tree() {
                Some(n) => ToolReply::Text(serde_json::to_string_pretty(n).unwrap_or_default()),
                None => ToolReply::Error(NO_AEM_TREE.into()),
            },
            "get_aem_translated_outline" => match self.aem_tree() {
                Some(n) => ToolReply::Text(crate::aem_translated_edit::outline(n)),
                None => ToolReply::Error(NO_AEM_TREE.into()),
            },
            "get_aem_translated_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                match self.aem_tree_mut() {
                    Some(root) => match crate::aem_translated_edit::resolve_mut(root, &path) {
                        Ok(node) => {
                            ToolReply::Text(serde_json::to_string_pretty(node).unwrap_or_default())
                        }
                        Err(e) => ToolReply::Error(e),
                    },
                    None => ToolReply::Error(NO_AEM_TREE.into()),
                }
            }
            "set_aem_translated_field" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let field = input["field"].as_str().unwrap_or_default().to_string();
                if field.is_empty() {
                    return ToolReply::Error("`field` must not be empty.".into());
                }
                let value = input.get("value").cloned().unwrap_or(serde_json::Value::Null);
                let result = match self.aem_tree_mut() {
                    Some(root) => crate::aem_translated_edit::set_field(root, &path, &field, value),
                    None => return ToolReply::Error(NO_AEM_TREE.into()),
                };
                match result {
                    Ok(msg) => {
                        self.aem_translated_edited(&format!("AI: set {field} on {path}"));
                        ToolReply::Text(msg)
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "replace_aem_translated_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let node = input.get("node").cloned().unwrap_or(serde_json::Value::Null);
                let result = match self.aem_tree_mut() {
                    Some(root) => crate::aem_translated_edit::replace_node(root, &path, node),
                    None => return ToolReply::Error(NO_AEM_TREE.into()),
                };
                match result {
                    Ok(msg) => {
                        self.aem_translated_edited(&format!("AI: replace {path}"));
                        ToolReply::Text(msg)
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "insert_aem_translated_node" => {
                let parent = input["parent_path"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let node = input.get("node").cloned().unwrap_or(serde_json::Value::Null);
                let pos = match crate::aem_translated_edit::parse_insert_pos(&input["position"]) {
                    Ok(p) => p,
                    Err(e) => return ToolReply::Error(e),
                };
                let result = match self.aem_tree_mut() {
                    Some(root) => crate::aem_translated_edit::insert_node(root, &parent, node, pos),
                    None => return ToolReply::Error(NO_AEM_TREE.into()),
                };
                match result {
                    Ok(msg) => {
                        self.aem_translated_edited(&format!("AI: insert into {parent}"));
                        ToolReply::Text(msg)
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "remove_aem_translated_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let result = match self.aem_tree_mut() {
                    Some(root) => crate::aem_translated_edit::remove_node(root, &path),
                    None => return ToolReply::Error(NO_AEM_TREE.into()),
                };
                match result {
                    Ok(msg) => {
                        self.aem_translated_edited(&format!("AI: remove {path}"));
                        ToolReply::Text(msg)
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }

            // §5 output
            "build_aem_package" => {
                let cfg = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                let (aem, translations) = match self.lower_aem_translated() {
                    Ok(pair) => pair,
                    Err(e) => return ToolReply::Error(e),
                };
                // Re-emit each loaded node's fidelity passthrough (raw attrs +
                // unmodeled children) so a template's load→edit→save round-trip
                // preserves what the typed model doesn't represent. Empty for
                // from-XFA trees, so their output is unchanged.
                let passthrough = self
                    .aem_tree()
                    .map(|t| t.passthrough_map())
                    .unwrap_or_default();
                let pkg = blueprint::to_aem_package_from_node_with_passthrough(
                    &aem,
                    &cfg,
                    translations,
                    &passthrough,
                );
                let size = pkg.len();
                if let Some(aem) = self.target.aem_mut() {
                    aem.package = Some(pkg);
                }
                ToolReply::Text(format!("Built package ({size} bytes)."))
            }
            "get_package_info" => match self.target.aem().and_then(|s| s.package.as_ref()) {
                Some(pkg) => {
                    let files = crate::references::unzip_package(pkg).unwrap_or_default();
                    let paths: Vec<&String> = files.iter().map(|(p, _)| p).collect();
                    ToolReply::Text(format!(
                        "size: {} bytes\nfiles:\n{}",
                        pkg.len(),
                        serde_json::to_string_pretty(&paths).unwrap_or_default()
                    ))
                }
                None => ToolReply::Error("No package built yet; call build_aem_package.".into()),
            },
            "read_package_file" => {
                let path = input["path"].as_str().unwrap_or_default();
                match self.target.aem().and_then(|s| s.package.as_ref()) {
                    Some(pkg) => match crate::references::unzip_package(pkg) {
                        Ok(files) => match files.iter().find(|(p, _)| p == path) {
                            Some((_, c)) => ToolReply::Text(c.clone()),
                            None => ToolReply::Error(format!("No such file: {path:?}")),
                        },
                        Err(e) => ToolReply::Error(e),
                    },
                    None => ToolReply::Error("No package built yet.".into()),
                }
            }
            "validate_aem_package" => {
                let Some(pkg) = self.target.aem().and_then(|s| s.package.clone()) else {
                    return ToolReply::Error(
                        "No package built yet; call build_aem_package.".into(),
                    );
                };
                match validate_package_bytes(&pkg) {
                    Ok(msg) => ToolReply::Text(msg),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "review_output" => {
                let (aem, _) = match self.lower_aem_translated() {
                    Ok(pair) => pair,
                    Err(e) => return ToolReply::Error(e),
                };
                let merged = match self.extractor(&serde_json::Value::Null) {
                    Ok(ex) => ex.merged.content.clone(),
                    Err(e) => return ToolReply::Error(e),
                };
                let config = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                let master = config.master_language.clone();
                let report = blueprint::review_output(&merged, &aem, &config, &master);
                ToolReply::Text(serde_json::to_string_pretty(&report).unwrap_or_default())
            }
            "generate_xsd" => {
                let p = match self.profile.clone() {
                    Some(p) if blueprint::has_xsd_config(&p) => p,
                    _ => return ToolReply::Error("This profile has no XSD config.".into()),
                };
                match blueprint::load_xsd_config(&p) {
                    Ok(mut cfg) => {
                        if let Ok(c) = self.config() {
                            cfg.form_code = Some(c.form_code.clone());
                        }
                        ToolReply::Text(blueprint::to_xsd(&self.structured, &cfg))
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "generate_html" => {
                let p = match self.profile.clone() {
                    Some(p) if blueprint::has_html_config(&p) => p,
                    _ => return ToolReply::Error("This profile has no HTML config.".into()),
                };
                match blueprint::load_html_custom_styles(&p) {
                    Ok(styles) => {
                        let cfg = blueprint::HtmlConfig {
                            custom_styles: Some(styles),
                            ..blueprint::HtmlConfig::default()
                        };
                        ToolReply::Text(blueprint::to_html(&self.structured, &cfg))
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }

            // §6 deploy + verify (network)
            "upload_to_aem" => {
                let Some(conn) = self.conn.clone() else {
                    return ToolReply::Error("No AEM connection configured.".into());
                };
                let Some(pkg) = self.target.aem().and_then(|s| s.package.clone()) else {
                    return ToolReply::Error(
                        "No package built yet; call build_aem_package.".into(),
                    );
                };
                let cfg = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                match crate::aem_client::upload_and_install_package(&conn, pkg, &cfg.form_code)
                    .await
                {
                    Ok(()) => {
                        if let Some(aem) = self.target.aem_mut() {
                            aem.uploaded = true;
                            aem.form_path = Some(form_jcr_path(&cfg));
                        }
                        ToolReply::Text("Uploaded and installed on AEM.".into())
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "fetch_aem_form_html" => {
                let (Some(conn), Ok(cfg)) = (self.conn.clone(), self.config()) else {
                    return ToolReply::Error("No AEM connection / profile configured.".into());
                };
                let path = form_jcr_path(&cfg);
                match crate::aem_client::fetch_form_html(&conn, &path).await {
                    Ok(html) => ToolReply::Text(truncate(&html, 8000)),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "fetch_aem_dor_pdf" => {
                let (Some(conn), Ok(cfg)) = (self.conn.clone(), self.config()) else {
                    return ToolReply::Error("No AEM connection / profile configured.".into());
                };
                let path = form_jcr_path(&cfg);
                match crate::aem_client::fetch_dor_pdf(&conn, &path).await {
                    Ok(pdf) => match render_pdf_pages(&pdf) {
                        Ok(images) => ToolReply::Image {
                            media_type: "image/jpeg",
                            images,
                        },
                        Err(e) => ToolReply::Error(e),
                    },
                    Err(e) => ToolReply::Error(e),
                }
            }

            // §7 references
            "list_reference_forms" => {
                let profile = self.profile.clone().unwrap_or_default();
                let list: Vec<_> = crate::references::list_references(&profile)
                    .into_iter()
                    .map(|r| serde_json::json!({"ref_id": r.ref_id, "label": r.label, "description": r.description, "pdf_count": r.pdf_count, "files": r.files}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
            }
            "search_references" => {
                let profile = self.profile.clone().unwrap_or_default();
                let query = input["query"].as_str().unwrap_or_default().to_string();
                if query.trim().is_empty() {
                    return ToolReply::Error(
                        "search_references requires a non-empty query — pass a description of the \
                         input form/section, not an empty string."
                            .into(),
                    );
                }
                let top_k = input["top_k"].as_u64().unwrap_or(3).max(1) as usize;
                let matcher = match self.matcher() {
                    Ok(m) => m,
                    Err(e) => return ToolReply::Error(e),
                };
                let hits: Vec<_> =
                    crate::references::search_references(&profile, &query, matcher, top_k)
                        .into_iter()
                        .map(|h| serde_json::json!({"ref_id": h.ref_id, "label": h.label, "where": h.location, "matched": h.matched, "score": h.score, "snippet": h.snippet}))
                        .collect();
                ToolReply::Text(serde_json::to_string_pretty(&hits).unwrap_or_default())
            }
            "grep_references" => {
                let profile = self.profile.clone().unwrap_or_default();
                let query = input["query"].as_str().unwrap_or_default();
                let regex = input["regex"].as_bool().unwrap_or(false);
                let hits: Vec<_> = crate::references::grep_references(&profile, query, regex)
                    .into_iter()
                    .map(|h| serde_json::json!({"ref_id": h.ref_id, "label": h.label, "where": h.location, "snippet": h.snippet}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&hits).unwrap_or_default())
            }
            "read_reference_file" => {
                let ref_id = input["ref_id"].as_str().unwrap_or_default();
                let path = input["path"].as_str().unwrap_or_default();
                let offset = input["offset"].as_u64().unwrap_or(0) as usize;
                let limit = input["limit"].as_u64().unwrap_or(0) as usize;
                match crate::references::read_reference_file(ref_id, path, offset, limit) {
                    Ok(t) => ToolReply::Text(t),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_reference_package" => {
                let ref_id = input["ref_id"].as_str().unwrap_or_default();
                let files = crate::references::get_reference_package_files(ref_id);
                let paths: Vec<&String> = files.iter().map(|(p, _)| p).collect();
                ToolReply::Text(serde_json::to_string_pretty(&paths).unwrap_or_default())
            }
            "list_reference_docs" => {
                let profile = self.profile.clone().unwrap_or_default();
                let list: Vec<_> = crate::references::list_docs(&profile)
                    .into_iter()
                    .map(|d| serde_json::json!({"doc_id": d.doc_id, "label": d.label}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
            }
            "read_reference_doc" => {
                let doc_id = input["doc_id"].as_str().unwrap_or_default();
                let offset = input["offset"].as_u64().unwrap_or(0) as usize;
                let limit = input["limit"].as_u64().unwrap_or(0) as usize;
                match crate::references::read_doc(doc_id, offset, limit) {
                    Ok(t) => ToolReply::Text(t),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "grep_reference_docs" => {
                let profile = self.profile.clone().unwrap_or_default();
                let query = input["query"].as_str().unwrap_or_default();
                let regex = input["regex"].as_bool().unwrap_or(false);
                let hits: Vec<_> = crate::references::grep_docs(&profile, query, regex)
                    .into_iter()
                    .map(|(doc_id, label, snippet)| serde_json::json!({"doc_id": doc_id, "label": label, "snippet": snippet}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&hits).unwrap_or_default())
            }

            // §8 control
            "get_schema" => {
                // Unknown/absent `kind` keeps returning the AEM schema, which is
                // what every caller predating the structured target expects.
                let schema = match input["kind"].as_str() {
                    Some("structured") => blueprint::structured_schema(),
                    _ => blueprint::aem_translated_schema(),
                };
                ToolReply::Text(serde_json::to_string_pretty(&schema).unwrap_or_default())
            }
            "get_profile_info" => match self.config() {
                Ok(c) => ToolReply::Text(format!(
                    "form_code: {}\nlanguages: {:?}\nmaster_language: {}\nform_path: {}\nform_dir: {}\nbind_to_xsd: {}\nuse_fragments: {}",
                    c.form_code,
                    c.languages,
                    c.master_language,
                    c.form_path,
                    c.form_dir,
                    c.bind_to_xsd,
                    c.use_fragments
                )),
                Err(e) => ToolReply::Error(e),
            },
            "finish" => {
                self.finished = true;
                ToolReply::Text("Finalized.".into())
            }
            "submit_review" => {
                let approved = input["approved"].as_bool().unwrap_or(false);
                let report = input["report"].as_str().unwrap_or_default().to_string();
                self.review = Some(ReviewResult { approved, report });
                ToolReply::Text(if approved {
                    "Review recorded: approved.".into()
                } else {
                    "Review recorded: changes requested — returning to the author.".into()
                })
            }

            other => ToolReply::Error(format!("Unknown tool: {other}")),
        }
    }
}

// ── Small helpers ────────────────────────────────────────────────────────────

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::prelude::BASE64_STANDARD.encode(bytes)
}

fn dedup(mut v: Vec<&str>) -> Vec<String> {
    v.sort();
    v.dedup();
    v.into_iter().map(String::from).collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
}

fn line_matches(line: &str, query: &str, regex: bool) -> bool {
    if regex {
        regex_lite::Regex::new(query)
            .map(|re| re.is_match(line))
            .unwrap_or(false)
    } else {
        line.to_lowercase().contains(&query.to_lowercase())
    }
}

/// The form's JCR node path from its AEM config.
fn form_jcr_path(cfg: &AemConfig) -> String {
    join_form_path(&cfg.form_path, &cfg.form_dir)
}

fn join_form_path(form_path: &str, form_dir: &str) -> String {
    format!(
        "/content/forms/af/{}/{}",
        form_path.trim_matches('/'),
        form_dir.trim_matches('/')
    )
}

/// Render the DoR PDF to one base64 JPEG per page via the engine.
fn render_pdf_pages(pdf: &[u8]) -> Result<Vec<String>, String> {
    let mut bp =
        blueprint::Blueprint::from_pdf_bytes(pdf).map_err(|e| format!("PDF parse: {e}"))?;
    let states = bp.states().map_err(|e| format!("states: {e}"))?;
    let state = states.iter().next().ok_or("no state in DoR PDF")?;
    let pages = state
        .render_plain_pages(RENDER_SCALE)
        .map_err(|e| format!("render: {e}"))?;
    pages
        .iter()
        .map(|img| {
            crate::image_encode::encode_rgba_to_jpeg(img, 82)
                .map(|jpeg| base64_encode(&jpeg))
                .map_err(|e| format!("encode: {e}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh agent authors the AEM tree directly and never fills
    /// `structured`, so anything deriving output from the source document must
    /// go through `source_structured` instead. Regression guard: exporting from
    /// `structured()` silently produced an empty document.
    #[test]
    fn source_structured_holds_the_converted_document_while_structured_is_empty() {
        let pdf = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../core/input/AAEV_019_EN.pdf");
        let bytes = std::fs::read(&pdf).expect("read AAEV_019_EN.pdf");

        let mut agent = ConversionAgent::new(
            Some("ubs".into()),
            vec![("AAEV_019_EN.pdf".to_string(), bytes)],
            None,
            "test-source-structured".into(),
            OutputTarget::Aem,
        );

        assert!(
            agent.structured().is_empty(),
            "a fresh agent has no working structured tree"
        );
        assert!(
            !agent.source_structured().is_empty(),
            "the converted source document must be reachable for non-AEM exports"
        );
    }

    fn fixture(name: &str) -> (String, Vec<u8>) {
        let pdf = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/input").join(name);
        let bytes = std::fs::read(&pdf).unwrap_or_else(|e| panic!("read {name}: {e}"));
        (name.to_string(), bytes)
    }

    /// A single-PDF source keeps the whole merged envelope, not just its
    /// content. Regression guard for the master-page header: `Context::header`
    /// is set only by `merged_structured()`, so an output built from
    /// `agent.context()` (which is `Blueprint::context()`, always
    /// `header: None`) silently loses it.
    #[test]
    fn source_envelope_carries_the_recovered_header() {
        let mut agent = ConversionAgent::new(
            Some("ubs".into()),
            vec![fixture("AAEV_019_EN.pdf")],
            None,
            "test-source-envelope".into(),
            OutputTarget::Aem,
        );

        assert!(
            agent.context().header.is_none(),
            "the agent's own context is taken before any analysis has run"
        );
        assert!(
            agent.source_envelope().context.header.is_some(),
            "the merged envelope must carry the header the analysis recovered"
        );
        assert!(agent.source_merge_error().is_none(), "one PDF needs no merge");
    }

    /// Regression: `Extractor::build` used to swallow a cross-language merge
    /// failure with `unwrap_or_default()`, leaving an empty merged tree that was
    /// indistinguishable from a document with no content. Every output derived
    /// from it — the Redacto dump in particular — then came out silently empty.
    #[test]
    fn extractor_merge_failure_is_reported_not_swallowed() {
        // Two unrelated forms: far below the structural-similarity threshold
        // `merge_translations` requires of language variants of one document.
        let mut agent = ConversionAgent::new(
            Some("ubs".into()),
            vec![fixture("AAAA_019_DE.pdf"), fixture("AABH_019_EN.pdf")],
            None,
            "test-merge-error".into(),
            OutputTarget::Aem,
        );

        let reason = agent
            .source_merge_error()
            .expect("a failed merge must be reported");
        assert!(
            reason.to_lowercase().contains("similar"),
            "the reason must name the structural-similarity check, got: {reason}"
        );
        assert!(
            agent.source_structured().is_empty(),
            "a failed merge yields no content — which is precisely why it must be reported"
        );
    }

    /// The app never offers an out-of-scope tool to a role, but MCP serves the
    /// flat catalog, so the target split has to refuse them itself — and say why
    /// rather than reporting a missing tree.
    #[test]
    fn aem_tools_are_refused_under_the_redacto_target() {
        let agent = ConversionAgent::new(
            Some("ubs".into()),
            Vec::new(),
            None,
            "test-redacto-guard".into(),
            OutputTarget::Redacto,
        );

        assert_eq!(agent.target(), OutputTarget::Redacto);
        assert!(agent.aem_translated().is_none());
        assert!(agent.package().is_none());
        assert!(!agent.aem_uploaded());
        assert!(agent.aem_session().is_none());
        assert!(agent.form_code().is_none());

        for tool in AEM_ONLY_TOOLS {
            let refusal = agent
                .target_refusal(tool)
                .unwrap_or_else(|| panic!("{tool} must be refused under the Redacto target"));
            assert!(
                refusal.contains("not available for the Redacto"),
                "the refusal must name the target, got: {refusal}"
            );
        }
    }

    /// The guard is symmetric: building a Redacto dump makes no sense in a run
    /// that is authoring an AEM form.
    #[test]
    fn redacto_tools_are_refused_under_the_aem_target() {
        let agent = ConversionAgent::new(
            Some("ubs".into()),
            Vec::new(),
            None,
            "test-aem-only-guard".into(),
            OutputTarget::Aem,
        );

        for tool in REDACTO_ONLY_TOOLS {
            let refusal = agent
                .target_refusal(tool)
                .unwrap_or_else(|| panic!("{tool} must be refused under the AEM target"));
            assert!(refusal.contains("not available for the AEM"), "{refusal}");
        }
    }

    /// The structured editors belong to neither target exclusively: a resumed
    /// AEM session seeds the same tree.
    #[test]
    fn structured_editors_are_available_under_both_targets() {
        for target in [OutputTarget::Aem, OutputTarget::Redacto] {
            let agent = ConversionAgent::new(
                Some("ubs".into()),
                Vec::new(),
                None,
                format!("test-shared-{}", target.as_str()),
                target,
            );
            for tool in [
                "set_structured",
                "get_structured_outline",
                "get_structured_node",
                "set_structured_field",
                "seed_structured_from_state",
            ] {
                assert!(
                    agent.target_refusal(tool).is_none(),
                    "{tool} must be available under {target:?}"
                );
            }
        }
    }

    /// `get_schema` declared a `kind` argument but ignored it, so the structured
    /// schema was unreachable even though `blueprint::structured_schema()` had
    /// always been there.
    #[tokio::test]
    async fn get_schema_dispatches_on_kind() {
        let mut agent = ConversionAgent::new(
            Some("ubs".into()),
            Vec::new(),
            None,
            "test-schema".into(),
            OutputTarget::Redacto,
        );

        let structured = reply_text(agent.execute("get_schema", &serde_json::json!({"kind": "structured"})).await);
        assert!(
            structured.contains("StructuredNode"),
            "expected the structured schema, got: {}",
            &structured[..200.min(structured.len())]
        );

        // Absent or unknown `kind` keeps the historical AEM answer.
        for input in [serde_json::json!({}), serde_json::json!({"kind": "nonsense"})] {
            let aem = reply_text(agent.execute("get_schema", &input).await);
            assert!(aem.contains("AemNodeTranslated"), "got: {}", &aem[..200.min(aem.len())]);
        }
    }

    fn reply_text(reply: ToolReply) -> String {
        match reply {
            ToolReply::Text(t) => t,
            ToolReply::Error(e) => panic!("unexpected tool error: {e}"),
            ToolReply::Image { .. } => panic!("unexpected image reply"),
        }
    }

    /// The whole point of the Redacto target: the agent seeds the engine's clean
    /// per-state tree and the dump is generated from that, so the artefact that
    /// ships is the one it worked on — with the markup, footnotes and multi-column
    /// layout the engine already got right.
    #[tokio::test]
    async fn seeding_from_a_state_yields_a_shippable_redacto_dump() {
        let mut agent = ConversionAgent::new(
            Some("ubs".into()),
            vec![fixture("AAEV_019_EN.pdf")],
            None,
            "test-redacto-seed".into(),
            OutputTarget::Redacto,
        );

        // Nothing authored yet: the dump tool must say so rather than emit an
        // empty document.
        match agent.execute("build_redacto_dump", &serde_json::json!({})).await {
            ToolReply::Error(e) => assert_eq!(e, NO_STRUCTURED_TREE),
            _ => panic!("an unseeded tree must not build a dump"),
        }

        let states = reply_text(agent.execute("list_states", &serde_json::json!({})).await);
        let label = serde_json::from_str::<serde_json::Value>(&states).unwrap()[0]["label"]
            .as_str()
            .expect("a state label")
            .to_string();

        let seeded = reply_text(
            agent
                .execute(
                    "seed_structured_from_state",
                    &serde_json::json!({"state_label": label}),
                )
                .await,
        );
        assert!(seeded.starts_with("OK"), "{seeded}");
        assert!(
            !agent.structured().is_empty(),
            "seeding must fill the working tree"
        );

        let built = reply_text(agent.execute("build_redacto_dump", &serde_json::json!({})).await);
        let report: serde_json::Value = serde_json::from_str(&built).unwrap();
        assert_eq!(
            report["problems"].as_array().map(Vec::len),
            Some(0),
            "seeded content must produce a shippable dump: {built}"
        );
        assert!(
            report["assets"].as_u64().unwrap_or(0) > 5,
            "expected a text-heavy document: {built}"
        );
        assert!(agent.redacto_dump().is_some(), "the dump must be cached for finalize");
    }

    /// The same tools stay reachable under the AEM target — the guard is about
    /// the target, not about the tools.
    #[test]
    fn aem_tools_are_reachable_under_the_aem_target() {
        let agent = ConversionAgent::new(
            Some("ubs".into()),
            Vec::new(),
            None,
            "test-aem-guard".into(),
            OutputTarget::Aem,
        );

        assert_eq!(agent.target(), OutputTarget::Aem);
        for tool in AEM_ONLY_TOOLS {
            assert!(
                agent.target_refusal(tool).is_none(),
                "{tool} must be available under the AEM target"
            );
        }
    }

    #[test]
    fn source_key_defaults_to_current() {
        assert_eq!(
            ConversionAgent::source_key(&serde_json::json!({})),
            "current"
        );
        assert_eq!(
            ConversionAgent::source_key(&serde_json::json!({"source": {"reference": "abc"}})),
            "reference:abc"
        );
    }

    #[test]
    fn line_matches_literal_and_regex() {
        assert!(line_matches("Account Holder", "holder", false));
        assert!(!line_matches("Account Holder", "nope", false));
        assert!(line_matches("field_42", r"field_\d+", true));
        assert!(!line_matches("field_x", r"field_\d+", true));
        // invalid regex → no match (not a panic)
        assert!(!line_matches("anything", "(", true));
    }

    #[test]
    fn config_reflects_languages_in_seeded_structured_tree() {
        use blueprint::{InlineText, ParagraphNode, StructuredNode, TranslatedText};

        let mut agent = ConversionAgent::new(
            Some("ubs".into()),
            Vec::new(),
            None,
            "test-config-languages".into(),
            OutputTarget::Aem,
        );
        // The ubs profile templates reference a couple of xfa vars; supply the
        // minimal context so load_aem_config succeeds without a real PDF.
        let mut vars = HashMap::new();
        vars.insert("formrange_code".to_string(), "TESTFORM".to_string());
        vars.insert("formrange_entity".to_string(), "TEST".to_string());
        agent.context = blueprint::Context::new("en".to_string(), vars);

        // With no content the config falls back to the profile default.
        let before = agent.config().expect("config loads for ubs profile");
        assert_eq!(before.languages, vec!["en".to_string()]);

        // Seed a bilingual (de + en) working tree.
        let mut content = TranslatedText::empty();
        content.insert("en", InlineText::plain("Hello"));
        content.insert("de", InlineText::plain("Hallo"));
        agent.seed_structured(vec![StructuredNode::Paragraph(ParagraphNode {
            content,
            som_path: None,
            source_name: None,
        })]);

        // config() must now reflect the languages present in the tree so
        // get_profile_info and the package builder treat the form as
        // multilingual instead of collapsing it to the en-only default.
        let after = agent.config().expect("config loads");
        assert!(after.languages.contains(&"en".to_string()));
        assert!(
            after.languages.contains(&"de".to_string()),
            "config.languages must include every language in the seeded tree, got {:?}",
            after.languages
        );
    }

    #[test]
    fn form_path_trims_slashes() {
        assert_eq!(
            join_form_path("/ubs/all/", "/AF_FORM/"),
            "/content/forms/af/ubs/all/AF_FORM"
        );
        assert_eq!(
            join_form_path("ubs", "AF_FORM"),
            "/content/forms/af/ubs/AF_FORM"
        );
    }
}
