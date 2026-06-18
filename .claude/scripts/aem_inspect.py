import json
import os
import sys
from collections import Counter

INPUT_TYPES = {
    "textbox", "datepicker", "radiobutton", "checkbox", "dropdownlist",
    "telephonebox", "emailbox", "numericbox", "datefield",
}

issues = []
all_names = []  # (name, path) for duplicate detection

# Load curated fragment coverage map: fragment name → [lowercase German labels it provides]
_coverage_path = os.path.join(os.path.dirname(__file__), "fragment_coverage.json")
try:
    with open(_coverage_path) as f:
        _fragment_coverage = json.load(f)
except Exception:
    _fragment_coverage = {}


def covered_labels(frag_ref):
    """Return set of lowercase labels known to be provided by this fragment."""
    name = frag_ref.rstrip("/").split("/")[-1]
    return {l.lower() for l in _fragment_coverage.get(name, [])}


def short_type(rt):
    return rt.split("/")[-1] if rt else "unknown"


def walk_panel(panel_node, panel_path):
    """Walk one top-level wizard panel, collect fields and fragments, then cross-check."""
    panel_fields = []   # (title, name, path)
    panel_frags  = []   # frag_ref strings

    def recurse(node, path):
        if not isinstance(node, dict):
            return
        rt    = node.get("sling:resourceType", "")
        stype = short_type(rt)
        title = node.get("jcr:title") or ""
        name  = node.get("name") or ""
        frag  = node.get("fragRef") or ""

        if "textdraw" in stype:
            val = (node.get("_value") or "").strip()
            if val:
                issues.append(f"!! STRAY TEXT at {path}: {val[:120]}")

        if "titledraw" in stype:
            val   = (node.get("_value") or "").strip()
            label = val[:80] if val else title[:80]
            print(f"    {'titledraw':20s}  {label}")

        if stype in INPUT_TYPES:
            if not title:
                issues.append(f"!! MISSING LABEL at {path} ({name or '?'})")
            else:
                print(f"    {stype:20s}  {title}  ({name})")
            if name:
                all_names.append((name, path))
            if title:
                panel_fields.append((title.strip().lower(), name, path))

        if frag:
            print(f"    FRAGMENT  {name}: {frag}")
            panel_frags.append(frag)

        for k, v in node.items():
            if isinstance(v, dict):
                recurse(v, f"{path}/{k}")

    recurse(panel_node, panel_path)

    # Cross-check: standalone fields whose label is already covered by a fragment
    if panel_frags and panel_fields:
        covered = set()
        for frag_ref in panel_frags:
            covered |= covered_labels(frag_ref)
        for (ftitle, fname, fpath) in panel_fields:
            if ftitle in covered:
                issues.append(
                    f"!! ENGINE DUPLICATE '{fname}' (label: '{ftitle}') at {fpath} — "
                    f"already provided by a fragment in this panel; remove this field"
                )


data = json.load(sys.stdin)

for panel_key, panel in data.items():
    if not isinstance(panel, dict):
        continue
    pname  = panel.get("name", panel_key)
    ptitle = panel.get("jcr:title") or ""
    print(f"\n[Panel] {pname}  {ptitle}")
    walk_panel(panel, panel_key)

# Duplicate name check across the whole form
counts = Counter(name for name, _ in all_names)
duplicates = {name for name, count in counts.items() if count > 1}
if duplicates:
    for name, path in all_names:
        if name in duplicates:
            issues.append(f"!! DUPLICATE NAME '{name}' at {path}")

if issues:
    print("\n--- Issues ---")
    for issue in issues:
        print(issue)
else:
    print("\n--- No issues found ---")
