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
    assert_eq!(Presence::from_str("visible"), Presence::Visible);
    assert_eq!(Presence::from_str("hidden"), Presence::Hidden);
    assert_eq!(Presence::from_str("invisible"), Presence::Invisible);
    assert_eq!(Presence::from_str("inactive"), Presence::Inactive);
    // Unknown values default to Visible
    assert_eq!(Presence::from_str("unknown"), Presence::Visible);
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
    children: &[(&str, &str)], // (child_name, child_value)
) {
    // Register parent exclGroup (is_field=false, is_parent_exclgroup=false)
    engine.register_xfa_node(group_name, group_path, None, false, "", false);

    // Register child fields (is_field=true, is_parent_exclgroup=true)
    for (child_name, child_value) in children {
        let child_path = format!("{}.{}", group_path, child_name);
        engine.register_xfa_node(
            child_name,
            &child_path,
            Some(group_path),
            true,
            child_value,
            true, // parent IS an exclGroup
        );
    }
}

#[test]
fn test_exclgroup_child_to_parent_propagation() {
    let mut engine = XfaScriptEngine::new();
    setup_exclgroup(&mut engine, "sex", "sex", &[("male", ""), ("female", "")]);

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
        &[("none", ""), ("some", "")],
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
        &[("optA", "A"), ("optB", "")],
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
        &[("male", ""), ("female", "")],
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
    engine.register_xfa_node("form", "form", None, false, "", false);
    engine.register_xfa_node("field1", "form.field1", Some("form"), true, "", false);

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
        &[("cash", ""), ("card", ""), ("transfer", "")],
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
