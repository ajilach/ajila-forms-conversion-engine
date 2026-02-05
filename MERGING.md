Updated Plan: Subtree-Based Conditional Merging
TL;DR: Flatten conditional XFA subtrees individually, compute global font statistics across ALL flattened content, then run module passes with shared stats, and wrap each conditional subtree's structured output in ONE conditional.


Identifying conditional subtrees
important: finding the subtrees should happen at XFA level, NOT flattened. modify exhaustive to return all xfa states, instead of flattened

Steps
Identify conditional subtrees
Add Flattened::from_subtree(&XfaNode, computed_values) - flatten a single subform node using its children and layout
Flatten ALL regions upfront: stable base + each conditional subtree → collect into Vec<Flattened>
Compute GlobalFontStats from all flattened subtrees via GlobalFontStats::from_flattened_iter()
Run module passes on each Flattened with shared GlobalContext containing the global stats
Wrap each conditional subtree's structured output in ONE ConditionalNode based on its condition






Summary: Content Change Detection for XFA Merge
Problem
The heading "Neuanlage (möglich ab dem 01. des aktuellen Monats)" should be wrapped in a conditional, but it's not detected because the text is changed via JavaScript (ffrb1 field value), not by structural presence changes (visible/hidden).

Root Cause
The existing conditional boundary detection only tracks presence changes (subforms that are visible in some states and hidden in others). It doesn't detect content changes where a subform is always visible but contains different text values.

Changes Made to src/merged/mod.rs
Extended SubformVisibility struct (line ~310-320):

Added content_hashes: HashMap<usize, u64> to track content hash per state
Added is_conditional() method (line ~323-340):

Returns true if presence differs OR content hashes differ across states
Added compute_subtree_content_hash() (line ~417-440):

Computes hash of text content within a subform
Added hash_node_text_content() (line ~455-495):

Recursively extracts text from Fields and Draws
For Fields: looks up computed value by field name from computed_values
For Draws: extracts static text or xfa:embed references
Updated collect_subform_visibility() (line ~363-410):

Now accepts computed_values parameter
Computes content hash when subform is visible
Key Discovery
ffrb1 is a Field (not Draw) inside STP_SectionTitle inside SectionTitle
computed_values uses short field names as keys (e.g., "ffrb1")
State 0: ffrb1 = "Neuanlage (möglich ab dem 01. des aktuellen Monats)"
State 2: ffrb1 = "Änderung"
Unresolved Issue: Recursion Depth