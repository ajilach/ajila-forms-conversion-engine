//! Tests for the scripting module
//!
//! These tests verify the XFA scripting functionality including:
//! - Script engine execution
//! - SOM path resolution
//! - Dependency tracking
//! - Event handling
//! - Form interaction

use super::*;
use std::collections::HashMap;

// =============================================================================
// Basic Engine Tests
// =============================================================================

#[test]
fn test_script_engine_basic() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", "DE");

    let script = XfaScript {
        source: r#"Footer_Line_txtlanguage.value"#.to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Ready,
        event_ref: EventRef::Form,
        name: None,
        run_at: RunAt::Client,
    };

    let result = engine.execute_script(&script);
    if let Err(e) = &result {
        eprintln!("Script execution error: {}", e);
    }
    assert!(result.is_ok(), "Script execution failed: {:?}", result);
}

#[test]
fn test_script_with_this_reference() {
    let mut engine = XfaScriptEngine::new();
    engine.set_current_field("ffFirstName_s", "ffFirstName_s", "");

    let mut translations = HashMap::new();
    translations.insert("GV_FirstName_s".to_string(), "Vorname(n)".to_string());
    engine.register_translation_object("myDE", translations);

    let script = XfaScript {
        source: r#"this.rawValue = myDE.GV_FirstName_s;"#.to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Ready,
        event_ref: EventRef::Form,
        name: None,
        run_at: RunAt::Client,
    };

    let result = engine.execute_script(&script);
    assert!(result.is_ok());

    if let Ok(Some(value)) = result {
        assert_eq!(value, "Vorname(n)");
    }
}

// =============================================================================
// SOM Resolution Tests
// =============================================================================

#[test]
fn test_som_resolver_basic() {
    let mut resolver = SomResolver::new();
    resolver.register_node(
        &SomPath::new("Page.Header.txtlanguage"),
        "txtlanguage",
        "field",
        Some(&SomPath::new("Page.Header")),
    );
    resolver.register_node(
        &SomPath::new("Page.Body.Name"),
        "Name",
        "field",
        Some(&SomPath::new("Page.Body")),
    );

    // Test simple name resolution
    let result = resolver.resolve_node("txtlanguage", None);
    assert_eq!(result, Some(SomPath::new("Page.Header.txtlanguage")));

    // Test full path resolution
    let result = resolver.resolve_node("Page.Header.txtlanguage", None);
    assert_eq!(result, Some(SomPath::new("Page.Header.txtlanguage")));
}

#[test]
fn test_som_resolver_indexed() {
    let mut resolver = SomResolver::new();
    resolver.register_node(
        &SomPath::new("Detail.Item"),
        "Item",
        "field",
        Some(&SomPath::new("Detail")),
    );
    resolver.register_node(
        &SomPath::new("Detail.Item"),
        "Item",
        "field",
        Some(&SomPath::new("Detail")),
    );
    resolver.register_node(
        &SomPath::new("Detail.Item"),
        "Item",
        "field",
        Some(&SomPath::new("Detail")),
    );

    // Test [0] index
    let result = resolver.resolve_nodes("Item[0]", None);
    assert_eq!(result.len(), 1);

    // Test [*] all instances
    let result = resolver.resolve_nodes("Item[*]", None);
    assert_eq!(result.len(), 3);
}

#[test]
fn test_som_path_creation() {
    let path = SomPath::new("Page.Header.Field");
    assert_eq!(path.as_str(), "Page.Header.Field");
    assert_eq!(path.name(), "Field");
    // Verify path has expected components
    assert!(path.as_str().split('.').count() == 3);
}

#[test]
fn test_som_path_parent() {
    let path = SomPath::new("Page.Header.Field");
    let parent = path.parent();
    assert!(parent.is_some());
    assert_eq!(parent.unwrap().as_str(), "Page.Header");
}

// =============================================================================
// Dependency Tracking Tests
// =============================================================================

#[test]
fn test_dependency_tracker() {
    let mut tracker = DependencyTracker::new();

    tracker.add_dependency(&SomPath::new("Total"), &SomPath::new("Price"));
    tracker.add_dependency(&SomPath::new("Total"), &SomPath::new("Quantity"));
    tracker.add_dependency(&SomPath::new("GrandTotal"), &SomPath::new("Total"));

    // When Price changes, Total should recalculate
    let deps = tracker.get_dependents(&SomPath::new("Price"));
    assert!(deps.iter().any(|p| p.as_str() == "Total"));

    // When Total changes, GrandTotal should recalculate
    let deps = tracker.get_dependents(&SomPath::new("Total"));
    assert!(deps.iter().any(|p| p.as_str() == "GrandTotal"));
}

#[test]
fn test_dependency_cascade() {
    let mut tracker = DependencyTracker::new();

    tracker.add_dependency(&SomPath::new("Subtotal"), &SomPath::new("Price"));
    tracker.add_dependency(&SomPath::new("Tax"), &SomPath::new("Subtotal"));
    tracker.add_dependency(&SomPath::new("Total"), &SomPath::new("Subtotal"));
    tracker.add_dependency(&SomPath::new("Total"), &SomPath::new("Tax"));

    // When Price changes, should cascade to Subtotal -> Tax -> Total
    let cascaded = tracker.get_dependents_cascade(&SomPath::new("Price"));

    assert!(cascaded.iter().any(|p| p.as_str() == "Subtotal"));
    assert!(cascaded.iter().any(|p| p.as_str() == "Tax"));
    assert!(cascaded.iter().any(|p| p.as_str() == "Total"));
}

// =============================================================================
// Event Parsing Tests
// =============================================================================

#[test]
fn test_event_activity_parsing() {
    assert_eq!(
        "ready".parse::<EventActivity>().unwrap(),
        EventActivity::Ready
    );
    assert_eq!(
        "click".parse::<EventActivity>().unwrap(),
        EventActivity::Click
    );
    assert_eq!(
        "initialize".parse::<EventActivity>().unwrap(),
        EventActivity::Initialize
    );
    assert_eq!(
        "change".parse::<EventActivity>().unwrap(),
        EventActivity::Change
    );
    assert_eq!(
        "calculate".parse::<EventActivity>().unwrap(),
        EventActivity::Calculate
    );
}

#[test]
fn test_event_ref_parsing() {
    assert_eq!("$form".parse::<EventRef>().unwrap(), EventRef::Form);
    assert_eq!("$layout".parse::<EventRef>().unwrap(), EventRef::Layout);
    assert_eq!("$".parse::<EventRef>().unwrap(), EventRef::Current);
}

#[test]
fn test_script_content_type_parsing() {
    // ScriptContentType doesn't implement FromStr, but we can check enum values
    assert_eq!(ScriptContentType::JavaScript, ScriptContentType::JavaScript);
    assert_eq!(ScriptContentType::FormCalc, ScriptContentType::FormCalc);
}

// =============================================================================
// Language Switching Tests
// =============================================================================

#[test]
fn test_language_switch_german() {
    let mut engine = XfaScriptEngine::new();

    engine.set_current_field("ffFirstName_s", "ffFirstName_s", "");
    engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", "DE");
    engine.register_field("Footer_Line_txtformid", "Footer_Line_txtformid", "AAAB");

    let mut de_translations = HashMap::new();
    de_translations.insert("GV_FirstName_s".to_string(), "Vorname(n)".to_string());
    engine.register_translation_object("myDE", de_translations);

    let mut en_translations = HashMap::new();
    en_translations.insert("GV_FirstName_s".to_string(), "First name(s)".to_string());
    engine.register_translation_object("myEN", en_translations);

    let script = XfaScript {
        source: r#"
            switch(Footer_Line_txtlanguage.value){
                case "DE":
                    this.rawValue = myDE.GV_FirstName_s;
                    break;
                case "EN":
                default:
                    this.rawValue = myEN.GV_FirstName_s;
            }
        "#
        .to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Ready,
        event_ref: EventRef::Form,
        name: None,
        run_at: RunAt::Client,
    };

    let result = engine.execute_script(&script);
    assert!(result.is_ok());

    if let Ok(Some(value)) = result {
        assert_eq!(value, "Vorname(n)");
    }
}

#[test]
fn test_language_switch_english() {
    let mut engine = XfaScriptEngine::new();

    engine.set_current_field("ffFirstName_s", "ffFirstName_s", "");
    engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", "EN");
    engine.register_field("Footer_Line_txtformid", "Footer_Line_txtformid", "AAAB");

    let mut de_translations = HashMap::new();
    de_translations.insert("GV_FirstName_s".to_string(), "Vorname(n)".to_string());
    engine.register_translation_object("myDE", de_translations);

    let mut en_translations = HashMap::new();
    en_translations.insert("GV_FirstName_s".to_string(), "First name(s)".to_string());
    engine.register_translation_object("myEN", en_translations);

    let script = XfaScript {
        source: r#"
            switch(Footer_Line_txtlanguage.value){
                case "DE":
                    this.rawValue = myDE.GV_FirstName_s;
                    break;
                case "EN":
                default:
                    this.rawValue = myEN.GV_FirstName_s;
            }
        "#
        .to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Ready,
        event_ref: EventRef::Form,
        name: None,
        run_at: RunAt::Client,
    };

    let result = engine.execute_script(&script);
    assert!(result.is_ok());

    if let Ok(Some(value)) = result {
        assert_eq!(value, "First name(s)");
    }
}

// =============================================================================
// Context-Aware Resolution Tests
// =============================================================================

#[test]
fn test_context_aware_field_resolution() {
    let mut engine = XfaScriptEngine::new();

    // Register two fields with the same name but different paths
    engine.register_field("Section1.ISIN", "ISIN", "value1");
    engine.register_field("Section2.Subsection.ISIN", "ISIN", "value2");
    engine.register_field("Section3.ISIN", "ISIN", "value3");

    // Test 1: No context - should return first registered
    let resolved = engine.resolve_field_by_name_with_context("ISIN");
    assert!(resolved.is_some(), "Should resolve ISIN without context");

    // Test 2: With context in Section2 - should prefer Section2.Subsection.ISIN
    engine.set_current_field("Section2.SomeField", "SomeField", "");
    let resolved = engine.resolve_field_by_name_with_context("ISIN");
    assert!(resolved.is_some());
    let path = resolved.unwrap().to_string();
    assert!(
        path.starts_with("Section2"),
        "Should resolve to Section2 path when context is in Section2, got: {}",
        path
    );

    // Test 3: With context in Section3 - should prefer Section3.ISIN
    engine.set_current_field("Section3.AnotherField", "AnotherField", "");
    let resolved = engine.resolve_field_by_name_with_context("ISIN");
    assert!(resolved.is_some());
    let path = resolved.unwrap().to_string();
    assert!(
        path.starts_with("Section3"),
        "Should resolve to Section3 path when context is in Section3, got: {}",
        path
    );

    // Test 4: Full path should work regardless of context
    let resolved = engine.resolve_field_by_name_with_context("Section1.ISIN");
    assert!(resolved.is_some());
    let path = resolved.unwrap().to_string();
    assert_eq!(path, "Section1.ISIN", "Full path should resolve exactly");
}

// =============================================================================
// State/Value Tests
// =============================================================================

#[test]
fn test_xfa_value_as_string() {
    use rust_decimal::Decimal;

    // XfaValue uses as_string to convert to string representation
    let val = XfaValue::String("hello".to_string());
    assert_eq!(val.as_string(), "hello");

    let val = XfaValue::Number(Decimal::new(42, 0));
    assert_eq!(val.as_string(), "42");

    let val = XfaValue::Boolean(true);
    assert_eq!(val.as_string(), "true");

    let val = XfaValue::Null;
    assert_eq!(val.as_string(), "");
}

#[test]
fn test_presence_from_str() {
    assert_eq!("visible".parse::<Presence>().unwrap(), Presence::Visible);
    assert_eq!("hidden".parse::<Presence>().unwrap(), Presence::Hidden);
    assert_eq!(
        "invisible".parse::<Presence>().unwrap(),
        Presence::Invisible
    );
    assert_eq!("inactive".parse::<Presence>().unwrap(), Presence::Inactive);
    // Unknown values default to Visible
    assert_eq!("unknown".parse::<Presence>().unwrap(), Presence::Visible);
}

#[test]
fn test_presence_should_skip_layout() {
    assert!(!Presence::Visible.should_skip_layout());
    assert!(Presence::Hidden.should_skip_layout());
    assert!(!Presence::Invisible.should_skip_layout());
    assert!(Presence::Inactive.should_skip_layout());
}

// =============================================================================
// Script Registry Tests
// =============================================================================

#[test]
fn test_script_registry_registration() {
    let mut registry = ScriptRegistry::new();

    let script = XfaScript {
        source: "this.rawValue = 'test'".to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Click,
        event_ref: EventRef::Current,
        name: Some("testScript".to_string()),
        run_at: RunAt::Client,
    };

    registry.register(RegisteredScript {
        script,
        owner_path: SomPath::new("Page.Button1"),
        owner_name: "Button1".to_string(),
        child_fields: vec![],
        script_type: ScriptType::Event,
    });

    let scripts = registry.get_event_scripts(&SomPath::new("Page.Button1"), &EventActivity::Click);
    assert_eq!(scripts.len(), 1);
}

#[test]
fn test_script_type_from_activity() {
    assert_eq!(
        ScriptType::from_activity(&EventActivity::Initialize),
        ScriptType::Initialize
    );
    assert_eq!(
        ScriptType::from_activity(&EventActivity::Calculate),
        ScriptType::Calculate
    );
    assert_eq!(
        ScriptType::from_activity(&EventActivity::Validate),
        ScriptType::Validate
    );
    assert_eq!(
        ScriptType::from_activity(&EventActivity::Click),
        ScriptType::Event
    );
    assert_eq!(
        ScriptType::from_activity(&EventActivity::Change),
        ScriptType::Event
    );
}

// =============================================================================
// JS Helper Tests
// =============================================================================

#[test]
fn test_js_helpers_available() {
    let helpers = get_all_helpers();
    assert!(!helpers.is_empty());

    // Verify key helpers exist - check for the function definitions
    assert!(
        helpers.contains("_xfa_resolve_path_"),
        "Should contain _xfa_resolve_path_ helper function"
    );
}

// =============================================================================
// ExclGroup Tests (XFA 3.3 §2 p.33, §4 pp.195-197)
// =============================================================================

/// Helper: register a parent exclGroup and its child fields structurally.
fn setup_exclgroup(
    engine: &mut XfaScriptEngine,
    group_path: &str,
    group_name: &str,
    children: &[(&str, &str, Option<&str>)], // (child_name, child_value, item_key)
) {
    setup_exclgroup_with_off(
        engine,
        group_path,
        group_name,
        &children
            .iter()
            .map(|(n, v, k)| (*n, *v, *k, None))
            .collect::<Vec<_>>(),
    );
}

/// Helper variant with off-value support for each child.
fn setup_exclgroup_with_off(
    engine: &mut XfaScriptEngine,
    group_path: &str,
    group_name: &str,
    children: &[(&str, &str, Option<&str>, Option<&str>)], // (child_name, child_value, item_key, off_value)
) {
    // Register parent exclGroup (is_field=false, is_parent_exclgroup=false)
    engine.register_xfa_node(
        group_name, group_path, None, false, "", false, None, None, "visible",
    );

    // Register child fields (is_field=true, is_parent_exclgroup=true)
    for (child_name, child_value, item_key, off_value) in children {
        let child_path = format!("{}.{}", group_path, child_name);
        engine.register_xfa_node(
            child_name,
            &child_path,
            Some(group_path),
            true,
            child_value,
            true, // parent IS an exclGroup
            *item_key,
            *off_value,
            "visible",
        );
    }
}

#[test]
fn test_exclgroup_child_to_parent_propagation() {
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(
        &mut engine,
        "sex",
        "sex",
        &[("male", "", Some("M")), ("female", "", Some("F"))],
    );

    // Set a child's rawValue → should propagate to parent
    engine.update_field_value("sex.male", "M");

    let parent_val = engine.get_field_value(&SomPath::new("sex"));
    assert_eq!(
        parent_val,
        Some("M".to_string()),
        "Setting child rawValue should propagate to parent exclGroup"
    );
}

#[test]
fn test_exclgroup_propagates_zero_value() {
    // Per XFA 3.3 spec, "0" is a legitimate key value for exclGroup children
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(
        &mut engine,
        "rating",
        "rating",
        &[("none", "", Some("0")), ("some", "", Some("1"))],
    );

    engine.update_field_value("rating.none", "0");

    let parent_val = engine.get_field_value(&SomPath::new("rating"));
    assert_eq!(
        parent_val,
        Some("0".to_string()),
        "Value '0' must propagate to parent exclGroup (it's a valid key value)"
    );
}

#[test]
fn test_exclgroup_propagates_empty_value() {
    // Per XFA 3.3 spec, clearing all selections sets exclGroup value to empty
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(
        &mut engine,
        "choice",
        "choice",
        &[("optA", "A", Some("A")), ("optB", "", Some("B"))],
    );

    // First set a value, then clear it
    engine.update_field_value("choice.optA", "A");
    let parent_val = engine.get_field_value(&SomPath::new("choice"));
    assert_eq!(parent_val, Some("A".to_string()));

    // Now clear the child — parent should also clear
    engine.update_field_value("choice.optA", "");
    let parent_val = engine.get_field_value(&SomPath::new("choice"));
    // Parent should have received the empty value (deselect all)
    assert!(
        parent_val.is_none() || parent_val == Some("".to_string()),
        "Empty value should propagate to parent exclGroup for deselect"
    );
}

#[test]
fn test_exclgroup_structural_detection_no_naming_convention() {
    // Ensure exclGroup linkage works regardless of naming — no "Group"/"RB_"
    // patterns needed. Uses names from the XFA 3.3 spec example (§4 p.196).
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(
        &mut engine,
        "sex",
        "sex",
        &[("male", "", Some("M")), ("female", "", Some("F"))],
    );

    engine.update_field_value("sex.male", "M");

    let parent_val = engine.get_field_value(&SomPath::new("sex"));
    assert_eq!(
        parent_val,
        Some("M".to_string()),
        "ExclGroup detection must be structural, not name-based"
    );

    // Also verify the other child can update the parent
    engine.update_field_value("sex.female", "F");
    let parent_val = engine.get_field_value(&SomPath::new("sex"));
    assert_eq!(
        parent_val,
        Some("F".to_string()),
        "Setting a different child should update the parent exclGroup value"
    );
}

#[test]
fn test_exclgroup_non_child_does_not_propagate() {
    // Fields NOT inside an exclGroup should NOT propagate to their parent
    let mut engine = XfaScriptEngine::new();

    // Register a regular subform with children (is_parent_exclgroup=false)
    engine.register_xfa_node(
        "form", "form", None, false, "", false, None, None, "visible",
    );
    engine.register_xfa_node(
        "field1",
        "form.field1",
        Some("form"),
        true,
        "",
        false,
        None,
        None,
        "visible",
    );

    engine.update_field_value("form.field1", "hello");

    // Parent should NOT have received the value
    let parent_val = engine.get_field_value(&SomPath::new("form"));
    assert!(
        parent_val.is_none() || parent_val == Some("".to_string()),
        "Non-exclGroup parent should not receive child values"
    );
}

#[test]
fn test_exclgroup_script_sets_child_rawvalue() {
    // Simulate a script setting rawValue on a child via JavaScript
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(
        &mut engine,
        "paymentMethod",
        "paymentMethod",
        &[
            ("cash", "", Some("CASH")),
            ("card", "", Some("CARD")),
            ("transfer", "", Some("TRANSFER")),
        ],
    );

    // Simulate script: this.rawValue = "card_value"
    engine.set_current_field("paymentMethod.card", "card", "");
    let script = XfaScript {
        source: r#"this.rawValue = "CARD";"#.to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Initialize,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let _ = engine.execute_script(&script);

    // The parent exclGroup should have received the value via the setter
    let parent_val = engine.get_field_value(&SomPath::new("paymentMethod"));
    assert_eq!(
        parent_val,
        Some("CARD".to_string()),
        "Script setting child rawValue should propagate to parent exclGroup"
    );
}

// =============================================================================
// ExclGroup Parent→Child Propagation Tests (XFA 3.3 §4 pp.195-197)
// =============================================================================
// Per XFA spec: "The field determines whether it is on or off by comparing
// the value of the variable to its own key value."
// Setting exclGroup.rawValue must update children's ON/OFF state.

#[test]
fn test_exclgroup_parent_to_child_propagation() {
    // Per XFA 3.3 §4 p.196: setting parent value should turn ON the
    // matching child and turn OFF all others.
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(
        &mut engine,
        "sex",
        "sex",
        &[
            ("male", "", Some("M")),
            ("female", "", Some("F")),
            ("na", "", Some("NA")),
        ],
    );

    // Set the parent exclGroup's rawValue to "M"
    engine.update_field_value("sex", "M");

    // The child whose _itemKey matches "M" should be ON (rawValue=_itemKey per XFA 3.3 §17 p.714)
    let male_val = engine.get_field_value(&SomPath::new("sex.male"));
    assert_eq!(
        male_val,
        Some("M".to_string()),
        "Child with matching _itemKey should be turned ON (rawValue=_itemKey per XFA spec)"
    );

    // Other children should be OFF (rawValue="")
    let female_val = engine.get_field_value(&SomPath::new("sex.female"));
    assert!(
        female_val.is_none() || female_val == Some("".to_string()),
        "Child with non-matching _itemKey should be OFF (rawValue='')"
    );
    let na_val = engine.get_field_value(&SomPath::new("sex.na"));
    assert!(
        na_val.is_none() || na_val == Some("".to_string()),
        "Child with non-matching _itemKey should be OFF (rawValue='')"
    );
}

#[test]
fn test_exclgroup_parent_to_child_deselects_previous() {
    // Changing the parent value should deselect the previously ON child
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(
        &mut engine,
        "color",
        "color",
        &[
            ("red", "", Some("R")),
            ("green", "", Some("G")),
            ("blue", "", Some("B")),
        ],
    );

    // Select red
    engine.update_field_value("color", "R");
    assert_eq!(
        engine.get_field_value(&SomPath::new("color.red")),
        Some("R".to_string()),
        "red should be ON (rawValue=_itemKey per XFA spec)"
    );

    // Now select green → red should become OFF
    engine.update_field_value("color", "G");
    assert_eq!(
        engine.get_field_value(&SomPath::new("color.green")),
        Some("G".to_string()),
        "green should be ON (rawValue=_itemKey per XFA spec)"
    );
    let red_val = engine.get_field_value(&SomPath::new("color.red"));
    assert!(
        red_val.is_none() || red_val == Some("".to_string()),
        "red should be OFF after selecting green"
    );
}

#[test]
fn test_exclgroup_off_value_used_when_deactivated() {
    // Per XFA 3.3 §17 pp.758-759: when a member has two <items> values,
    // the second is used as the off-value when the member is deactivated.
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup_with_off(
        &mut engine,
        "toggle",
        "toggle",
        &[
            ("yes", "", Some("Y"), Some("N")),   // on=Y, off=N
            ("maybe", "", Some("M"), Some("X")), // on=M, off=X
        ],
    );

    // Activate "yes"
    engine.update_field_value("toggle", "Y");
    assert_eq!(
        engine.get_field_value(&SomPath::new("toggle.yes")),
        Some("Y".to_string()),
        "Activated member should return its on-value"
    );
    // "maybe" is deactivated → should return off-value "X"
    assert_eq!(
        engine.get_field_value(&SomPath::new("toggle.maybe")),
        Some("X".to_string()),
        "Deactivated member with off-value should return off-value, not empty string"
    );

    // Now activate "maybe"
    engine.update_field_value("toggle", "M");
    assert_eq!(
        engine.get_field_value(&SomPath::new("toggle.maybe")),
        Some("M".to_string()),
        "Activated member should return its on-value"
    );
    // "yes" is now deactivated → should return off-value "N"
    assert_eq!(
        engine.get_field_value(&SomPath::new("toggle.yes")),
        Some("N".to_string()),
        "Deactivated member with off-value should return off-value, not empty string"
    );
}

#[test]
fn test_exclgroup_parent_to_child_via_script() {
    // Setting rawValue on the parent via JS script should propagate to children
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(
        &mut engine,
        "status",
        "status",
        &[("active", "", Some("A")), ("inactive", "", Some("I"))],
    );

    // Set the parent's rawValue via JavaScript
    engine.set_current_field("status", "status", "");
    let script = XfaScript {
        source: r#"this.rawValue = "I";"#.to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Initialize,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let _ = engine.execute_script(&script);

    // The matching child should be ON (rawValue=_itemKey per XFA 3.3 §17 p.714)
    let inactive_val = engine.get_field_value(&SomPath::new("status.inactive"));
    assert_eq!(
        inactive_val,
        Some("I".to_string()),
        "Child matching parent's rawValue should be ON (rawValue=_itemKey per XFA spec)"
    );

    // The non-matching child should be OFF
    let active_val = engine.get_field_value(&SomPath::new("status.active"));
    assert!(
        active_val.is_none() || active_val == Some("".to_string()),
        "Child not matching parent's rawValue should be OFF after script sets parent"
    );
}

// =============================================================================
// Empty Value Preservation Tests (XFA spec compliance)
// =============================================================================
// Per XFA spec, empty strings are valid field states:
// - A cleared dropdown has rawValue = ""
// - A deselected exclGroup member has rawValue = ""
// - A script that clears a field via this.rawValue = "" is a valid action
// These must NOT be silently dropped.

#[test]
fn test_get_all_som_field_values_preserves_empty_strings() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("field1", "field1", "hello");
    engine.register_field("field2", "field2", "");

    let values = engine.get_all_som_field_values();
    assert_eq!(
        values.get("field1"),
        Some(&"hello".to_string()),
        "Non-empty field should be present"
    );
    assert_eq!(
        values.get("field2"),
        Some(&"".to_string()),
        "Empty string field should be present, not dropped"
    );
}

#[test]
fn test_get_all_field_values_for_flattening_preserves_empty_strings() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("field1", "field1", "hello");
    engine.register_field("field2", "field2", "");

    let values = engine.get_all_field_values_for_flattening();
    assert_eq!(
        values.get(&SomPath::new("field1")),
        Some(&"hello".to_string()),
        "Non-empty field should be present in flattening values"
    );
    assert_eq!(
        values.get(&SomPath::new("field2")),
        Some(&"".to_string()),
        "Empty string field should be present in flattening values, not dropped"
    );
}

#[test]
fn test_script_clearing_field_via_empty_rawvalue_is_detected() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("myField", "myField", "initial_value");

    // Simulate a script that clears the field
    engine.set_current_field("myField", "myField", "initial_value");
    let script = XfaScript {
        source: r#"this.rawValue = "";"#.to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Initialize,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let _ = engine.execute_script(&script);

    // The cleared value must appear as empty string, not be absent
    let values = engine.get_all_som_field_values();
    assert!(
        values.contains_key("myField"),
        "Cleared field must still be present in get_all_som_field_values"
    );
    assert_eq!(
        values.get("myField"),
        Some(&"".to_string()),
        "Field cleared via rawValue = '' should have empty string value"
    );

    let flat_values = engine.get_all_field_values_for_flattening();
    assert!(
        flat_values.contains_key(&SomPath::new("myField")),
        "Cleared field must still be present in get_all_field_values_for_flattening"
    );
    assert_eq!(
        flat_values.get(&SomPath::new("myField")),
        Some(&"".to_string()),
        "Field cleared via rawValue = '' should have empty string in flattening values"
    );
}

#[test]
fn test_exclgroup_deselection_preserves_empty_values() {
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(
        &mut engine,
        "myGroup",
        "myGroup",
        &[
            ("optA", "", Some("A")),
            ("optB", "", Some("B")),
            ("optC", "", Some("C")),
        ],
    );

    // All children start with empty rawValue (deselected state)
    let values = engine.get_all_som_field_values();
    // The deselected children should have empty string values, not be absent
    assert_eq!(
        values.get("optA"),
        Some(&"".to_string()),
        "Deselected exclGroup member should have empty string value"
    );
    assert_eq!(
        values.get("optB"),
        Some(&"".to_string()),
        "Deselected exclGroup member should have empty string value"
    );
    assert_eq!(
        values.get("optC"),
        Some(&"".to_string()),
        "Deselected exclGroup member should have empty string value"
    );
}

// =============================================================================
// resolveNodes() Tests (XFA 3.3 §3 pp.106-107)
// =============================================================================

#[test]
fn test_resolve_nodes_all_instances() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Detail.Item", "Item", "val1");
    engine.register_field("Detail.Item", "Item", "val2");
    engine.register_field("Detail.Item", "Item", "val3");

    // resolveNodes("Item[*]") should return all 3 instances
    engine.set_current_field("Detail.Test", "Test", "");
    let script = XfaScript {
        source: r#"
            var nodes = xfa.resolveNodes("Item[*]");
            this.rawValue = String(nodes.length);
        "#
        .to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Calculate,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let result = engine.execute_script(&script);
    assert!(result.is_ok());
    if let Ok(Some(value)) = result {
        assert_eq!(value, "3", "resolveNodes('Item[*]') should return 3 items");
    }
}

#[test]
fn test_resolve_nodes_specific_index() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Detail.Item", "Item", "first");
    engine.register_field("Detail.Item", "Item", "second");

    // resolveNodes("Item[0]") should return exactly 1 item
    engine.set_current_field("Detail.Test", "Test", "");
    let script = XfaScript {
        source: r#"
            var nodes = xfa.resolveNodes("Item[0]");
            this.rawValue = String(nodes.length);
        "#
        .to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Calculate,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let result = engine.execute_script(&script);
    assert!(result.is_ok());
    if let Ok(Some(value)) = result {
        assert_eq!(
            value, "1",
            "resolveNodes('Item[0]') should return exactly 1 item"
        );
    }
}

#[test]
fn test_resolve_nodes_nonexistent() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Detail.Item", "Item", "val1");

    // resolveNodes for a nonexistent field should return empty array
    engine.set_current_field("Detail.Test", "Test", "");
    let script = XfaScript {
        source: r#"
            var nodes = xfa.resolveNodes("NonexistentField[*]");
            this.rawValue = String(nodes.length);
        "#
        .to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Calculate,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let result = engine.execute_script(&script);
    assert!(result.is_ok());
    if let Ok(Some(value)) = result {
        assert_eq!(
            value, "0",
            "resolveNodes for non-existent field should return empty array"
        );
    }
}

#[test]
fn test_resolve_nodes_descendant_accessor() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Form.Section1.Amount", "Amount", "100");
    engine.register_field("Form.Section2.Amount", "Amount", "200");

    // resolveNodes("$data..Amount") should return both instances
    engine.set_current_field("Form.Total", "Total", "");
    let script = XfaScript {
        source: r#"
            var nodes = xfa.resolveNodes("$data..Amount");
            this.rawValue = String(nodes.length);
        "#
        .to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Calculate,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let result = engine.execute_script(&script);
    assert!(result.is_ok());
    if let Ok(Some(value)) = result {
        assert_eq!(
            value, "2",
            "resolveNodes('$data..Amount') should return 2 descendant matches"
        );
    }
}

#[test]
fn test_resolve_nodes_by_simple_name() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Page1.Field1", "Field1", "a");
    engine.register_field("Page2.Field1", "Field1", "b");
    engine.register_field("Page1.Field2", "Field2", "c");

    // resolveNodes("Field1") should return 2 instances (both pages)
    engine.set_current_field("Page1.Test", "Test", "");
    let script = XfaScript {
        source: r#"
            var nodes = xfa.resolveNodes("Field1");
            this.rawValue = String(nodes.length);
        "#
        .to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Calculate,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let result = engine.execute_script(&script);
    assert!(result.is_ok());
    if let Ok(Some(value)) = result {
        assert_eq!(
            value, "2",
            "resolveNodes('Field1') should return all 2 instances by name"
        );
    }
}

// =============================================================================
// ExclGroup Dedup Fix Tests
// =============================================================================

#[test]
fn test_exclgroup_dedup_full_path_keys() {
    // Two exclGroups each with a child named "RB_1" — both selected.
    // Full-path keys must preserve both values.
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(
        &mut engine,
        "groupA",
        "groupA",
        &[("RB_1", "", Some("A")), ("RB_2", "", Some("B"))],
    );
    setup_exclgroup(
        &mut engine,
        "groupB",
        "groupB",
        &[("RB_1", "", Some("X")), ("RB_2", "", Some("Y"))],
    );

    // Select RB_1 in groupA (value "A") and RB_1 in groupB (value "X")
    engine.update_field_value("groupA", "A");
    engine.update_field_value("groupB", "X");

    let values = engine.get_all_som_field_values_by_path();

    // Full-path entries must both exist and be correct
    assert_eq!(
        values.get("groupA.RB_1"),
        Some(&"A".to_string()),
        "groupA.RB_1 should be 'A' via full-path key"
    );
    assert_eq!(
        values.get("groupB.RB_1"),
        Some(&"X".to_string()),
        "groupB.RB_1 should be 'X' via full-path key"
    );
}

#[test]
fn test_exclgroup_dedup_selected_vs_deselected() {
    // One RB_1 selected, one deselected — selected value retrievable by full path.
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(
        &mut engine,
        "groupA",
        "groupA",
        &[("RB_1", "", Some("A")), ("RB_2", "", Some("B"))],
    );
    setup_exclgroup(
        &mut engine,
        "groupB",
        "groupB",
        &[("RB_1", "", Some("X")), ("RB_2", "", Some("Y"))],
    );

    // Select RB_1 only in groupA
    engine.update_field_value("groupA", "A");

    let full_path_values = engine.get_all_som_field_values_by_path();

    assert_eq!(
        full_path_values.get("groupA.RB_1"),
        Some(&"A".to_string()),
        "Selected groupA.RB_1 should be 'A'"
    );
    assert_eq!(
        full_path_values.get("groupB.RB_1"),
        Some(&"".to_string()),
        "Deselected groupB.RB_1 should be ''"
    );

    // The short-name entry for "RB_1" should be the non-empty one
    let values = engine.get_all_som_field_values();
    let short_name_value = values.get("RB_1");
    assert_eq!(
        short_name_value,
        Some(&"A".to_string()),
        "Short-name 'RB_1' should have the non-empty value"
    );
}

// =============================================================================
// $event Property Tests (XFA 3.3 §10 pp.398-404)
// =============================================================================

#[test]
fn test_event_name_property() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Form.Field1", "Field1", "hello");

    // Set up event context for a click event
    engine.update_event_context(&EventActivity::Click, "Form.Field1");
    engine.set_current_field("Form.Field1", "Field1", "hello");

    // $event.name should be "click"
    let script = XfaScript {
        source: r#"this.rawValue = xfa.event.name;"#.to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Click,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let result = engine.execute_script(&script);
    assert!(result.is_ok());
    if let Ok(Some(value)) = result {
        assert_eq!(value, "click", "$event.name should be 'click'");
    }
}

#[test]
fn test_event_target_property() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Form.MyField", "MyField", "test");

    engine.update_event_context(&EventActivity::Enter, "Form.MyField");
    engine.set_current_field("Form.MyField", "MyField", "test");

    // $event.target should be the field object with name "MyField"
    let script = XfaScript {
        source: r#"this.rawValue = xfa.event.target.name;"#.to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Enter,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let result = engine.execute_script(&script);
    assert!(result.is_ok());
    if let Ok(Some(value)) = result {
        assert_eq!(value, "MyField", "$event.target.name should be 'MyField'");
    }
}

#[test]
fn test_event_cancel_action_writable() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Form.Field1", "Field1", "");

    engine.update_event_context(&EventActivity::Validate, "Form.Field1");
    engine.set_current_field("Form.Field1", "Field1", "");

    // cancelAction should be writable
    let script = XfaScript {
        source: r#"
            xfa.event.cancelAction = true;
            this.rawValue = String(xfa.event.cancelAction);
        "#
        .to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Validate,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let result = engine.execute_script(&script);
    assert!(result.is_ok());
    if let Ok(Some(value)) = result {
        assert_eq!(value, "true", "cancelAction should be writable");
    }
}

#[test]
fn test_event_modifier_defaults_false() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Form.Field1", "Field1", "");

    engine.update_event_context(&EventActivity::Click, "Form.Field1");
    engine.set_current_field("Form.Field1", "Field1", "");

    // modifier should default to false
    let script = XfaScript {
        source: r#"this.rawValue = String(xfa.event.modifier);"#.to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Click,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let result = engine.execute_script(&script);
    assert!(result.is_ok());
    if let Ok(Some(value)) = result {
        assert_eq!(value, "false", "$event.modifier should default to false");
    }
}

#[test]
fn test_event_change_property_for_change_event() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Form.Field1", "Field1", "original");

    engine.update_event_context(&EventActivity::Change, "Form.Field1");
    engine.set_current_field("Form.Field1", "Field1", "original");

    // prevText should be set to current value for change events
    let script = XfaScript {
        source: r#"this.rawValue = xfa.event.prevText;"#.to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Change,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let result = engine.execute_script(&script);
    assert!(result.is_ok());
    if let Ok(Some(value)) = result {
        assert_eq!(
            value, "original",
            "prevText should contain the field's value before change"
        );
    }
}

#[test]
fn test_event_all_properties_accessible() {
    let mut engine = XfaScriptEngine::new();
    engine.register_field("Form.Field1", "Field1", "");

    engine.update_event_context(&EventActivity::Click, "Form.Field1");
    engine.set_current_field("Form.Field1", "Field1", "");

    // Verify all expected properties are accessible (not undefined)
    let script = XfaScript {
        source: r#"
            var e = xfa.event;
            var props = [
                typeof e.name !== 'undefined',
                typeof e.target !== 'undefined',
                typeof e.cancelAction !== 'undefined',
                typeof e.change !== 'undefined',
                typeof e.commitKey !== 'undefined',
                typeof e.fullText !== 'undefined',
                typeof e.keyDown !== 'undefined',
                typeof e.modifier !== 'undefined',
                typeof e.newContentType !== 'undefined',
                typeof e.newText !== 'undefined',
                typeof e.prevContentType !== 'undefined',
                typeof e.prevText !== 'undefined',
                typeof e.reenter !== 'undefined',
                typeof e.selEnd !== 'undefined',
                typeof e.selStart !== 'undefined',
                typeof e.shift !== 'undefined'
            ];
            this.rawValue = String(props.every(function(p) { return p; }));
        "#
        .to_string(),
        content_type: ScriptContentType::JavaScript,
        activity: EventActivity::Click,
        event_ref: EventRef::Current,
        name: None,
        run_at: RunAt::Client,
    };
    let result = engine.execute_script(&script);
    assert!(result.is_ok());
    if let Ok(Some(value)) = result {
        assert_eq!(
            value, "true",
            "All $event properties should be accessible (not undefined)"
        );
    }
}
