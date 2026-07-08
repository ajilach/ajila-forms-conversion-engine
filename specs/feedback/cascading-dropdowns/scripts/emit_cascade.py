"""Emitter for cascading-dropdown AEM variants.

Starting point, not a black box: read references/visibility-encoding.md and
confirm the exact visibility-JS field-reference idiom against the profile's
precedent (e.g. AACF_019_SP) before generating the full variant set. Adapt
`build_visibility_js` to match what that precedent actually does — this
module only guarantees the *encoding pipeline* is correct, not the JS idiom.

Usage sketch (adapt field names/paths from a freshly-read get_aem_xml_outline,
never hard-code them from a previous run):

    table = json.load(open("cascade_table.json"))  # Phase 2 shape
    for fragment in emit_all_variants(table, trigger_field="DD_Bereich_8b487b50"):
        print(fragment.field_name)
        print(fragment.xml)
"""

import json
from dataclasses import dataclass, field


def escape_visibility_attribute(pattern_a_array):
    """Apply the 3-layer inverse pipeline: JSON -> FileVault -> XML.

    Order matters: double every backslash BEFORE escaping commas, otherwise
    the backslash just inserted for a comma gets doubled too.
    """
    json_string = json.dumps(pattern_a_array)
    fv_escaped = json_string.replace("\\", "\\\\").replace(",", "\\,")
    xml_escaped = (
        fv_escaped.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
    )
    return xml_escaped


def build_visibility_js(conditions, helper_show="window.forms.ubs.showAFShowDor(this);",
                         helper_hide="window.forms.ubs.hideAFHideDor(this);"):
    """conditions: list of lists of (field, op, value) tuples.

    Each inner list is AND'd together; the outer list is OR'd. E.g. for
    "(parent1 in {A,B}) AND (parent2 == X)" pass:
        [[("parent1", "==", "A"), ("parent1", "==", "B")], [("parent2", "==", "X")]]
    and set `and_groups=True` semantics are expressed by calling this once per
    AND-group and joining with " && " yourself if the shape gets more complex
    than the helper below — this is a starting point, not a general solver.

    IMPORTANT: the field-reference syntax below ("field.rawValue" style) is a
    placeholder. Confirm the real idiom against the precedent .content.xml
    (see references/visibility-encoding.md) before using this in anger.
    """
    or_groups = []
    for and_group in conditions:
        or_groups.append(" || ".join(f'{f} == "{v}"' for f, _, v in and_group))
    condition_str = " && ".join(f"({g})" for g in or_groups)
    return (
        f"if ({condition_str}) {{\n"
        f"  {helper_show}\n"
        f"}} else {{\n"
        f"  {helper_hide}\n"
        f"}}"
    )


def build_scriptmodel_attribute(field_path, js_content):
    pattern_a = [
        {
            "script": {
                "field": field_path,
                "event": "Visibility",
                "model": {"nodeName": "SHOW_EXPRESSION"},
                "content": js_content,
            },
            "nodeName": "SCRIPTMODEL",
            "version": 1,
            "enabled": True,
        }
    ]
    return escape_visibility_attribute(pattern_a)


@dataclass
class DropdownVariant:
    field_name: str
    title: str
    options: list
    locked: bool
    visibility_js: str
    field_path: str
    resource_type: str = "dam/formsanddocuments/af/rulesengine/fd/af/components/guideContainer/guideFieldSet/guideDropdown"
    xml: str = field(default="", init=False)

    def render(self):
        visible_attr = build_scriptmodel_attribute(self.field_path, self.visibility_js)
        options_xml = "\n".join(
            f'    <option value="{escape_xml_attr(opt)}" jcr:primaryType="nt:unstructured"/>'
            for opt in self.options
        )
        access = "protected" if self.locked and len(self.options) == 1 else "open"
        self.xml = f"""<{self.field_name}
    jcr:primaryType="nt:unstructured"
    sling:resourceType="{self.resource_type}"
    jcr:title="{escape_xml_attr(self.title)}"
    fieldType="drop-down list"
    access="{access}"
    visible="{{Boolean}}false">
    <cq:responsive jcr:primaryType="nt:unstructured"/>
    <fd:rules jcr:primaryType="nt:unstructured"/>
    <fd:scripts
        jcr:primaryType="nt:unstructured"
        fd:visible="{visible_attr}"/>
{options_xml}
</{self.field_name}>"""
        return self.xml


def escape_xml_attr(value):
    return (
        value.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
    )


def emit_all_variants(cascade_table, trigger_field):
    """Walk the Phase 2 cascade table and yield one DropdownVariant per
    child group and one per leaf group. Field names/paths here are examples —
    build the real ones from a freshly-read get_aem_xml_outline.
    """
    variants = []
    for entry in cascade_table["cascade"]:
        child_field = cascade_table["child_field_template"].format(group=entry["child_group"])
        parent_conditions = [[(trigger_field, "==", v) for v in entry["parent_values"]]]
        child_js = build_visibility_js(parent_conditions)
        variants.append(
            DropdownVariant(
                field_name=child_field,
                title=entry["child_group"],
                options=[o["label"] for o in entry["child_options"]],
                locked=False,
                visibility_js=child_js,
                field_path=child_field,
            )
        )
        for opt in entry["child_options"]:
            leaf = cascade_table["leaves"][opt["leaf_group"]]
            grandchild_field = cascade_table["grandchild_field_template"].format(leaf=opt["leaf_group"])
            leaf_conditions = [
                [(trigger_field, "==", v) for v in entry["parent_values"]],
                [(child_field, "==", opt["label"])],
            ]
            leaf_js = build_visibility_js(leaf_conditions)
            variants.append(
                DropdownVariant(
                    field_name=grandchild_field,
                    title=opt["label"],
                    options=leaf["options"],
                    locked=leaf["locked"],
                    visibility_js=leaf_js,
                    field_path=grandchild_field,
                )
            )
    for v in variants:
        v.render()
    return variants


if __name__ == "__main__":
    import sys

    table_path = sys.argv[1] if len(sys.argv) > 1 else "cascade_table.json"
    trigger = sys.argv[2] if len(sys.argv) > 2 else "DD_TRIGGER_PLACEHOLDER"
    with open(table_path) as f:
        table = json.load(f)
    for variant in emit_all_variants(table, trigger):
        print(f"<!-- {variant.field_name} -->")
        print(variant.xml)
        print()
