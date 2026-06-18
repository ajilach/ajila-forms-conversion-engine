# Known Engine Bugs

This file lists confirmed bugs in the Blueprint conversion engine. Each entry describes the symptom, the fix applied locally, and what the engine should do instead.

When a new bug is found during conversion, append it here following the same format. Always include: the form where it was first observed, the symptom, the local fix, and the expected engine behaviour.

---

## BUG-001 — Stale child nodes placed inside fragRef panels

**First observed:** AAFB_019  
**Symptom:** The engine places extra field panels (e.g. a `removebutton` and a `panel_copy_copy` containing fields with `fd:valueCommit` scripts referencing components from a different form) as child nodes inside a panel that has a `fragRef` attribute. AEM ignores all children of a fragRef panel — they are completely invisible to the end user.  
**Local fix:** Remove all child nodes from inside fragRef panels. Any fields that genuinely belong in that section must be added as **siblings** of the fragRef panel (placed after its closing tag in the parent `<items>` block), never as children.  
**Expected engine behaviour:** The engine must never place field nodes inside a panel that carries a `fragRef` attribute. Fragment-additional fields must be emitted as sibling nodes.

---

## BUG-002 — Extra fields for affrg_IndividualBasic panels not generated as siblings

**First observed:** AAFB_019  
**Symptom:** When a customer block uses `affrg_IndividualBasic1` (which provides only Nachname + Vorname(n)), the engine does not generate the remaining customer fields (address, date of birth, tax ID, etc.) that are visible in the PDF. Those fields are simply absent from the output.  
**Local fix:** Manually add the missing fields as sibling panels after the fragRef panel: one row panel for Strasse + Nr., one row panel for PLZ + Stadt + Land, one row panel for Geburtsdatum + Steuerliche ID-Nr. Use `cq:responsive/default width` values in a 12-column grid.  
**Expected engine behaviour:** After placing the fragment reference, the engine should detect all additional fields visible in that customer block and emit them as sibling panels at the same indentation level.

---

## BUG-003 — Repeatable panel maxOccur hardcoded to 4 regardless of JSON

**First observed:** AAFB_019  
**Symptom:** The JSON `DocumentEnvelope` carries correct `minOccurrences` / `maxOccurrences` values on repeatable panels (e.g. `"maxOccurrences": 2`). The generated XML always emits `maxOccur="4"` regardless of the JSON value. The BT_Add and BT_RemoveLR scripts also hardcode the threshold `4` in their `>= 4` / `&lt;4` conditions.  
**Local fix:** In the XML, update `maxOccur` on the repeatable panel element to match the JSON value. Also update the Add-button `fd:click` and `fd:init` scripts (`>= 4` → `>= <correct>`) and the Remove-button `fd:click` script (`&lt;4` → `&lt;<correct>`).  
**Expected engine behaviour:** The engine must read `maxOccurrences` (and `minOccurrences`) from the `DocumentEnvelope` and emit the correct `maxOccur` / `minOccur` XML attributes and matching threshold values in the Add/Remove button scripts.

---

## BUG-004 — Wrong fragment assigned to non-signature sections

**First observed:** (general pattern, not yet tied to a specific form in this project)  
**Symptom:** The engine assigns a signature fragment (e.g. `affrg_SignatureGeneric1`) to a section that is not a signature section (e.g. an Ort/Datum panel or a date-only row). The fragment renders incorrectly in AEM.  
**Local fix:** Replace the wrong fragment reference with the correct one (or with hand-crafted field components if no matching fragment exists). Report under "Engine faults — fixed locally, needs engine-level change".  
**Expected engine behaviour:** Fragment selection must be gated on structural matches (XSD type / bindRef pattern), not on superficial node proximity. Signature fragments must only be assigned to panels whose XSD type maps to a known signature fragment type.

---

## BUG-005 — stateCount too low; dropdown/radio options incomplete

**First observed:** AAFB_019 (stateCount=2, missed Firma/GbR options)  
**Symptom:** `stateCount` in the JSON root is lower than the number of distinct option combinations visible in the PDF. The engine did not explore all form states, so some dropdown or radio options are absent from the JSON.  
**Local fix:** Check the XML — if it also has incomplete options, fix both JSON and XML and regenerate. If the XML somehow already has the correct options (e.g. from a prior manual correction), leave the XML as-is and note the discrepancy.  
**Expected engine behaviour:** The exhaustive-search pass must explore all reachable states, including all options of every dropdown and radio group, so that `stateCount` reflects the full combinatorial space.
