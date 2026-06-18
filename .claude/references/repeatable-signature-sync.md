# Repeatable → Signature Sync Pattern

Use this when a form has a repeatable customer/person section that must stay in sync with a signature section (same count, name pre-filled).

The conversion engine does **not** generate these scripts. They must be added as XML-only patches in Step 6.

---

## Detection (from PDF renders)

Apply this pattern when you see **both**:
- A repeatable section: a person/customer block with visible "Add" / "Remove" buttons
- A signature section that should have one signature slot per customer

---

## Finding component names (unzip the generated ZIP first)

```bash
unzip -o <name>_merged.zip -d _pkg_tmp
XMLFILE=_pkg_tmp/jcr_root/content/forms/af/**/**/**/.content.xml
```

| What you need | How to find it |
|---------------|----------------|
| Repeatable customer panel name | `grep -n 'maxOccur' $XMLFILE` → look for `name="PN_..."` on the same element |
| Signature repeatable panel name | `grep -n 'affrg_ClientSignature1\|affrg_LegalGuardianSignature1\|affrg_ARSignature1' $XMLFILE` → the parent panel with `maxOccur` |
| Add button path | `grep -n 'BT_Add' $XMLFILE` → look at its nesting in the XML to build the guide path |
| Remove button path | `grep -n 'BT_Remove' $XMLFILE` |
| Vorname / Nachname field names | `grep -n 'Vorname\|Nachname\|FirstName\|LastName' $XMLFILE` → the `name=` attribute |
| Name display field inside signature | `grep -n 'TXT_Name_Sign\|name_sign\|Name_Sign' $XMLFILE` |
| Wizard panel containing signature | The top-level `<items>` child that holds the signature section — its `name=` attribute |

**Guide path format:** `guide.guideRootPanel.<wiz_panel_name>.<...>.<button_name>`
Build it by reading the XML nesting from `<items>` down to the button.

**Nesting depth for `this.parent` chain:**
Count levels from the name field up to the repeatable panel node (the one with `maxOccur`). Each level = one `.parent`. The default `this.parent.parent.parent.instanceIndex` assumes depth 3 — verify and adjust if needed.

---

## XML attribute templates

All three use the SCRIPTMODEL JSON format (same as `fd:init`). The `content` strings use `\n` for newlines and `\"` for inner quotes (XML-escaped as `\&quot;`).

### 1. Add button — `fd:click` on `BT_Add`

```xml
fd:click="[{&quot;script&quot;:{&quot;content&quot;:&quot;PN_REPEAT.instanceManager.addInstance();\nPN_SIGN.instanceManager.addInstance();\n\nif (len &gt;= 4) {this.visible = false;}\n&quot;,&quot;event&quot;:&quot;Click&quot;,&quot;field&quot;:&quot;GUIDE_PATH_BT_ADD&quot;},&quot;nodeName&quot;:&quot;SCRIPTMODEL&quot;,&quot;version&quot;:1,&quot;enabled&quot;:true}]"
```

Replace:
- `PN_REPEAT` → repeatable customer panel name (from `maxOccur` grep)
- `PN_SIGN` → signature repeatable panel name
- `GUIDE_PATH_BT_ADD` → full guide path of the Add button

### 2. Remove button — `fd:click` on `BT_Remove` / `BT_RemoveLR`

```xml
fd:click="[{&quot;script&quot;:{&quot;content&quot;:&quot;PN_REPEAT.instanceManager.removeInstance(PN_REPEAT.instanceIndex);\nPN_SIGN.instanceManager.removeInstance(PN_SIGN.instanceIndex);&quot;,&quot;event&quot;:&quot;Click&quot;,&quot;field&quot;:&quot;GUIDE_PATH_BT_REMOVE&quot;,&quot;model&quot;:{&quot;nodeName&quot;:&quot;EVENT_SCRIPTS&quot;}},&quot;nodeName&quot;:&quot;SCRIPTMODEL&quot;,&quot;version&quot;:1,&quot;enabled&quot;:true},{&quot;script&quot;:{&quot;content&quot;:&quot;BT_Add.visible = true;&quot;,&quot;event&quot;:&quot;Click&quot;,&quot;field&quot;:&quot;GUIDE_PATH_BT_REMOVE&quot;},&quot;nodeName&quot;:&quot;SCRIPTMODEL&quot;,&quot;version&quot;:1,&quot;enabled&quot;:true}]"
```

Replace: `PN_REPEAT`, `PN_SIGN`, `GUIDE_PATH_BT_REMOVE`.

### 3. Name sync — `fd:valueCommit` on Vorname AND Nachname fields

Add this **same attribute** to both the Vorname and Nachname text field nodes:

```xml
fd:valueCommit="[{&quot;script&quot;:{&quot;content&quot;:&quot;var currentIndex = this.parent.parent.parent.instanceIndex;\nvar currentFirstname = TXT_FIRSTNAME.value ? TXT_FIRSTNAME.value : \&quot;\&quot;;\nvar currentLastname = TXT_LASTNAME.value ? TXT_LASTNAME.value : \&quot;\&quot;;\nPN_SIGCONTAINER.PN_SIGN.instanceManager.instances[currentIndex].TXT_NAME_SIGN.value = currentFirstname + \&quot; \&quot; + currentLastname;&quot;,&quot;event&quot;:&quot;Value Commit&quot;,&quot;field&quot;:&quot;GUIDE_PATH_FIELD&quot;},&quot;nodeName&quot;:&quot;SCRIPTMODEL&quot;,&quot;version&quot;:1,&quot;enabled&quot;:true}]"
```

Replace:
- `TXT_FIRSTNAME` / `TXT_LASTNAME` → AEM names of the two name fields (sibling scope — no path prefix needed)
- `PN_SIGCONTAINER` → wizard panel containing the signature section
- `PN_SIGN` → signature repeatable panel name
- `TXT_NAME_SIGN` → name display field inside the signature fragment
- `GUIDE_PATH_FIELD` → full guide path of the specific field (Vorname or Nachname)
- `this.parent.parent.parent` → adjust depth to match XML nesting

---

## Signature fragment types

Each Formular-Adressat type uses a different signature fragment. The engine usually generates the conditional sub-panels and `fd:visible` expressions. Only add the `instanceManager` scripts — do not touch the visibility logic.

| Formular-Adressat | Fragment |
|-------------------|---------|
| Privatkunde | `affrg_ClientSignature1` |
| Minderjährige | `affrg_LegalGuardianSignature1` |
| Firma / GbR | `affrg_ARSignature1` |

---

## Fragment embedding

Always keep signature fragments as `fragRef=` references. Only embed (break the fragment ref and inline the content) if a field must be added or removed from the fragment structure. Embedding creates maintenance overhead — avoid unless necessary.

---

## Concrete reference

`references/AAGO_019_DE.zip` — unzip and read the `.content.xml` to see the exact attribute values in a working form. Search for `instanceManager` or `valueCommit` to find the relevant lines quickly.
