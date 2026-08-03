# The signature-fragment name contract

Reference data for [PROBLEM-signature-panel-ref](consistent-problems.md) (issue #80).

A signature fragment pre-fills the signer's name by resolving the person's **repeatable
data panel by a hardcoded identifier**:

```js
var pnIndividual = PN_AHRP.instanceManager.instances[…].PN_IndividualBasic.PN_Name_Individual;
var lastName = pnIndividual.TXT_LastName.value;
```

Forms in this corpus carry no `bindRef`, so the component `name` *is* the binding. That
identifier is therefore a hard contract: a host form whose panel is named anything else
gets a `ReferenceError` and an empty signature field.

## How this table is produced — do not hand-edit it

`data/signature_contract.json` is **derived** from the UBS fragment library packages:

```bash
python3 .claude/scripts/extract_signature_contract.py <fragment-pkg.zip> [more.zip …]
```

`signature_contract.py` loads that JSON. **Re-run the extractor whenever the UBS fragment 
library changes**, and commit the regenerated JSON in the same PR. To detect drift 
without rewriting anything (suitable for CI):

```bash
python3 .claude/scripts/extract_signature_contract.py --check <fragment-pkg.zip>
```

Exit 1 plus a `drift` map means the committed table no longer matches the fragments.

### Two rules the extractor encodes

**The code is authoritative, not the comment.** Fragments carry a `//Expecting <role>
panel fragment with name <X>` comment that is frequently stale copy-paste — see the
mismatch column below. Only the identifier before `.instanceManager` is extracted.

**Keyed by `<library>/<fragment>`, never by fragment name alone.** The same fragment name
means different things per library: `affrg_UBSEuropeSignature1` resolves `PN_EURP` in the
germany library and `PN_AHRP` in the italy one.

## The contract (16 fragments carry a name-calc; 15 are unambiguous)

| Library | Fragment | Resolves | Comment says (if it disagrees) |
|---|---|---|---|
| germany | `affrg_ClientSignature1` | `PN_AHRP` | — |
| germany | `affrg_LegalGuardianSignature1` | `PN_LGA` | — |
| germany | `affrg_ARSignature1` | `PN_ARP` | ⚠ `PN_LGA` |
| germany | `affrg_DepositorSignature1` | `PN_AHRP` | — |
| germany | `affrg_Signature_Account_Holder` | `PN_AHRP` | — |
| germany | `affrg_Sign_authorizedsignatory` | `PN_AHRP` | — |
| germany | `affrg_AuthorizedSignature_Frag` | `PN_ARP` | — |
| germany | `affrg_BOSignature1` | `PN_BORP` | ⚠ `PN_AHRP` |
| germany | `affrg_UBSEuropeSignature1` | `PN_EURP` | ⚠ `PN_AHRP` |
| germany | `affrg_ARPerson_1` | `PN_Authorized_Person` | ⚠ `PN_ARP` |
| italy | `affrg_ClientSignature1` | `PN_AHRP` | — |
| italy | `affrg_LegalGuardianSignature1` | `PN_LGA` | — |
| italy | `affrg_LegalRepresentativeSignature1` | `PN_LRP` | ⚠ `PN_LGA` |
| italy | `affrg_Sign_authorizedsignatory` | `PN_AHRP` | — |
| italy | `affrg_UBSEuropeSignature1` | `PN_AHRP` | — |
| ubs | `affrg_SignatureGeneric1` | **ambiguous** — resolves both `PN_AHGRP` and `PN_Sign_AHGRP` | — |

`affrg_SignatureGeneric1` is deliberately left out of the contract: a fragment resolving
two panels has no single answer, and the sweep must not act on a guess.

## Fragment-side defects — reported to UBS 2026-07-28

Scanning the library turned up defects on the fragment side; they were reported to UBS and
are being fixed there, so they are **not** tracked as repo work. Recorded here because they
explain the corpus:

- **`PN_ARRP` is a typo for `PN_ARP`** in `germany/affrg_AuthRep` and `affrg_AuthRep1`.
  `PN_ARRP` exists in **0** of the 48 forms that reference those fragments; `PN_ARP` exists
  in all 48 — so their add/remove logic throws on every click. 34 of those forms also use
  `affrg_ARSignature1`, which resolves `PN_ARP`, so no single panel name could satisfy both:
  the contradiction was unfixable on the form side. **UBS is renaming `PN_ARRP` → `PN_ARP`**,
  after which this table is unchanged and the affected set stays as-is.
- **`affrg_UBSEuropeSignature1` carries two different contracts under one name** (germany 
  `PN_EURP`, italy `PN_AHRP`) — the reason this table is keyed by library.
- **Five fragments' `//Expecting` comments name a different panel than their code** (⚠
  above). Not a runtime defect — the code runs — but actively misleading, and the reason
  the detector treats the comment as a scope signal only. It also explains a class of
  corpus confusion: forms were hand-edited to match the comment rather than the code.

## Out of scope: signature fragments with no name-calc

27 of the 42 `*Sign*` fragments never look up a name, so this rule does not apply to them.
They are the "Signature / Place / Date / Name" family where the name is typed by the user,
plus the internal-bank-use blocks: everything under `afforms_global_fragmentlib`, the
`affrg_italy_*_signature_place_date_name` family, `affrg_germany_Client_Signature`,
`affrg_germany_InternalBankUse_OURef_Signature`,
`affrg_german_ClientAdvisorSignature_Signature_Place_Date_Name`,
`affrg_italy_Client_Signature`, `affrg_accountHolderSignature` and
`affrg_signatureLegalRepresen`.

A completeness check was run for the opposite risk — a name-calc that resolves its panel
some other way than `.instanceManager`. The 10 hits are all `fd:validate` / `fd:init`
scripts inside *data* fragments (`affrg_IndividualBasic1`,
`affrg_germany_PersonalDetails_Name`), not signature name-fills, so requiring
`.instanceManager` loses nothing.

## Standard procedure when the fragment library changes

1. Export the fragment library package(s) from AEM.
2. `extract_signature_contract.py <pkg.zip …>` — regenerates `data/signature_contract.json`.
3. Review the printed `ambiguous` and `comment_disagrees` lists; a new ambiguous fragment
   needs a human decision before the sweep can touch forms that reference it.
4. `find_signature_panel_ref.py` — re-scan the corpus; the affected set moves with the table.
5. If forms became affected, run the sweep for issue #80 (pilot → **AEM visual check** → continue).
6. Commit the regenerated JSON together with any accept-list change.
