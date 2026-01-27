//! XFA Scripting Module - Spec-Conformant Implementation (XFA 3.3)
//!
//! This module implements script execution for XFA forms following XFA 3.3 specification.
//!
//! ## XFA 3.3 Spec Implementation:
//!
//! ### Chapter 3 - Scripting Object Model (SOM)
//! - `resolveNode(somExpression)` - Returns single node matching SOM path (page 106-107)
//! - `resolveNodes(somExpression)` - Returns list of nodes matching SOM path
//! - SOM paths: `$form.Receipt.Tax`, `Detail[0].Total_Price`, `$data..fieldName`
//! - `$` refers to current container in SOM expressions (inside resolveNode)
//! - `this` refers to current container in native JavaScript expressions (page 109)
//!
//! ### Chapter 10 - Automation Objects (page 378-408)
//! - Dependency tracking for cascading calculations
//! - Execution order: (1) events, (2) calculate, (3) validate
//! - Calculate objects re-activated when dependent values change
//!
//! ### Chapter 11 - Scripting (page 410-416)
//! - JavaScript (`application/x-javascript`): `this` = current container
//! - FormCalc (`application/x-formcalc`): `$` = current container (spec default)
//! - Named script objects in `<variables>`: functions become methods, vars become properties

use boa_engine::{
    Context, JsArgs, JsValue, NativeFunction,
    js_string, Source, JsString,
    object::{JsObject, ObjectInitializer},
    property::{Attribute, PropertyKey},
};
use boa_gc::{Finalize, Trace, GcRefCell};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::str::FromStr;

/// Script content type as per XFA spec
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScriptContentType {
    JavaScript,
    FormCalc,
}

impl ScriptContentType {
    pub fn from_content_type(s: &str) -> Option<Self> {
        match s {
            "application/x-javascript" => Some(ScriptContentType::JavaScript),
            "application/x-formcalc" => Some(ScriptContentType::FormCalc),
            _ => None,
        }
    }
}

/// XFA Event activity types
/// Per XFA spec section 10, "Events"
#[derive(Debug, Clone, PartialEq)]
pub enum EventActivity {
    Ready,
    Initialize,
    Enter,
    Exit,
    Change,
    Click,
    Calculate,
    Validate,
    PreSubmit,
    PostSubmit,
    DocReady,
    IndexChange,
    Other(String),
}

impl FromStr for EventActivity {
    type Err = ();
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "ready" => EventActivity::Ready,
            "initialize" => EventActivity::Initialize,
            "enter" => EventActivity::Enter,
            "exit" => EventActivity::Exit,
            "change" => EventActivity::Change,
            "click" => EventActivity::Click,
            "calculate" => EventActivity::Calculate,
            "validate" => EventActivity::Validate,
            "preSubmit" => EventActivity::PreSubmit,
            "postSubmit" => EventActivity::PostSubmit,
            "docReady" => EventActivity::DocReady,
            "indexChange" => EventActivity::IndexChange,
            _ => EventActivity::Other(s.to_string()),
        })
    }
}

/// XFA Event reference target
#[derive(Debug, Clone, PartialEq)]
pub enum EventRef {
    Form,
    Layout,
    Data,
    Current,
    Named(String),
}

impl FromStr for EventRef {
    type Err = ();
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "$form" | "xfa.form" => EventRef::Form,
            "$layout" | "xfa.layout" => EventRef::Layout,
            "$data" | "xfa.data" => EventRef::Data,
            "$" => EventRef::Current,
            _ => EventRef::Named(s.to_string()),
        })
    }
}

/// Represents a script attached to an event
#[derive(Debug, Clone)]
pub struct XfaScript {
    pub source: String,
    pub content_type: ScriptContentType,
    pub activity: EventActivity,
    pub event_ref: EventRef,
    pub name: Option<String>,
    pub run_at: RunAt,
}

/// Where the script should be executed
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RunAt {
    #[default]
    Client,
    Server,
    Both,
}

impl FromStr for RunAt {
    type Err = ();
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "server" => RunAt::Server,
            "both" => RunAt::Both,
            _ => RunAt::Client,
        })
    }
}

/// Value wrapper for XFA field values
#[derive(Debug, Clone)]
pub enum XfaValue {
    Null,
    String(String),
    Number(Decimal),
    Boolean(bool),
}

impl XfaValue {
    pub fn to_js_value(&self, _context: &mut Context) -> JsValue {
        match self {
            XfaValue::Null => JsValue::null(),
            XfaValue::String(s) => JsValue::from(js_string!(s.as_str())),
            XfaValue::Number(n) => {
                let f: f64 = n.to_f64().unwrap_or(0.0);
                JsValue::from(f)
            }
            XfaValue::Boolean(b) => JsValue::from(*b),
        }
    }
    
    pub fn from_js_value(value: &JsValue, context: &mut Context) -> Self {
        if value.is_null_or_undefined() {
            XfaValue::Null
        } else if let Some(b) = value.as_boolean() {
            XfaValue::Boolean(b)
        } else if let Some(n) = value.as_number() {
            XfaValue::Number(Decimal::from_str(&n.to_string()).unwrap_or(Decimal::ZERO))
        } else if let Ok(s) = value.to_string(context) {
            XfaValue::String(s.to_std_string_escaped())
        } else {
            XfaValue::Null
        }
    }
    
    pub fn as_string(&self) -> String {
        match self {
            XfaValue::Null => String::new(),
            XfaValue::String(s) => s.clone(),
            XfaValue::Number(n) => n.to_string(),
            XfaValue::Boolean(b) => b.to_string(),
        }
    }
}

// =============================================================================
// SOM Resolver - XFA 3.3 Chapter 3 (pages 86-120)
// =============================================================================

/// Node information for SOM resolution
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub name: String,
    pub path: String,
    pub parent_path: Option<String>,
    pub index: usize,
    pub class_name: String, // "field", "subform", etc.
}

/// SOM (Scripting Object Model) Resolver
/// Implements resolveNode() and resolveNodes() per XFA 3.3 spec Chapter 3
pub struct SomResolver {
    /// All registered nodes indexed by full path
    nodes: HashMap<String, NodeInfo>,
    /// Nodes indexed by name (may have duplicates)
    nodes_by_name: HashMap<String, Vec<String>>,
    /// Parent-child relationships
    children: HashMap<String, Vec<String>>,
}

impl SomResolver {
    pub fn new() -> Self {
        SomResolver {
            nodes: HashMap::new(),
            nodes_by_name: HashMap::new(),
            children: HashMap::new(),
        }
    }
    
    /// Register a node in the SOM tree
    pub fn register_node(&mut self, path: &str, name: &str, class_name: &str, parent_path: Option<&str>) {
        let index = self.nodes_by_name.get(name).map(|v| v.len()).unwrap_or(0);
        
        let info = NodeInfo {
            name: name.to_string(),
            path: path.to_string(),
            parent_path: parent_path.map(|s| s.to_string()),
            index,
            class_name: class_name.to_string(),
        };
        
        self.nodes.insert(path.to_string(), info);
        self.nodes_by_name
            .entry(name.to_string())
            .or_default()
            .push(path.to_string());
        
        if let Some(parent) = parent_path {
            self.children
                .entry(parent.to_string())
                .or_default()
                .push(path.to_string());
        }
    }
    
    /// Resolve a SOM expression to a single node path
    /// Per XFA 3.3 spec page 106-107
    pub fn resolve_node(&self, som_expression: &str, context_path: Option<&str>) -> Option<String> {
        let paths = self.resolve_nodes(som_expression, context_path);
        paths.into_iter().next()
    }
    
    /// Resolve a SOM expression to multiple node paths
    /// Per XFA 3.3 spec page 106-107
    pub fn resolve_nodes(&self, som_expression: &str, context_path: Option<&str>) -> Vec<String> {
        let expr = som_expression.trim();
        
        // Handle shortcuts
        let expr = if expr.starts_with("$form.") {
            &expr[6..] // Strip "$form."
        } else if expr.starts_with("$data.") {
            &expr[6..] // Strip "$data."
        } else if expr == "$" {
            // $ = current context
            return context_path.map(|p| vec![p.to_string()]).unwrap_or_default();
        } else if let Some(relative) = expr.strip_prefix("$.") {
            // $.foo = relative to current context
            if let Some(ctx) = context_path {
                return self.resolve_relative(ctx, relative);
            }
            return Vec::new();
        } else {
            expr
        };
        
        // Handle descendant accessor (..)
        if expr.contains("..") {
            return self.resolve_descendant(expr);
        }
        
        // Handle array index notation [n]
        if expr.contains('[') {
            return self.resolve_indexed(expr);
        }
        
        // Simple path lookup - try direct path first
        if self.nodes.get(expr).is_some() {
            return vec![expr.to_string()];
        }
        
        // Try to match by building path from parts
        let parts: Vec<&str> = expr.split('.').collect();
        self.resolve_path_parts(&parts, None)
    }
    
    /// Resolve relative path from context
    fn resolve_relative(&self, context_path: &str, relative: &str) -> Vec<String> {
        let full_path = format!("{}.{}", context_path, relative);
        if self.nodes.contains_key(&full_path) {
            vec![full_path]
        } else {
            // Search children of context
            if let Some(children) = self.children.get(context_path) {
                children.iter()
                    .filter(|p| p.ends_with(&format!(".{}", relative)))
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            }
        }
    }
    
    /// Resolve descendant accessor (e.g., "$data..fieldName")
    fn resolve_descendant(&self, expr: &str) -> Vec<String> {
        let parts: Vec<&str> = expr.split("..").collect();
        if parts.len() == 2 {
            let target_name = parts[1];
            // Find all nodes with this name
            self.nodes_by_name.get(target_name)
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    }
    
    /// Resolve indexed expression (e.g., "Detail[0]" or "Item[*]")
    fn resolve_indexed(&self, expr: &str) -> Vec<String> {
        // Parse "Name[index]" pattern
        if let Some(bracket_pos) = expr.find('[') {
            let name = &expr[..bracket_pos];
            let index_part = &expr[bracket_pos + 1..expr.len() - 1];
            
            if let Some(paths) = self.nodes_by_name.get(name) {
                if index_part == "*" {
                    // Return all instances
                    return paths.clone();
                } else if let Ok(index) = index_part.parse::<usize>() {
                    // Return specific index
                    return paths.get(index).cloned().into_iter().collect();
                }
            }
        }
        Vec::new()
    }
    
    /// Resolve path parts recursively
    fn resolve_path_parts(&self, parts: &[&str], _from: Option<&str>) -> Vec<String> {
        if parts.is_empty() {
            return Vec::new();
        }
        
        // Try matching by simple name for single-part paths
        if parts.len() == 1 {
            return self.nodes_by_name.get(parts[0])
                .cloned()
                .unwrap_or_default();
        }
        
        // Try building full path
        let full_path = parts.join(".");
        if self.nodes.contains_key(&full_path) {
            return vec![full_path];
        }
        
        // Search for partial matches
        self.nodes.keys()
            .filter(|p| p.ends_with(&full_path))
            .cloned()
            .collect()
    }
}

// =============================================================================
// Dependency Tracker - XFA 3.3 Chapter 10 (pages 379-380)
// =============================================================================

/// Tracks dependencies between calculated fields
/// Per XFA 3.3 spec: "Calculate objects that refer to the changed object 
/// will then be executed"
pub struct DependencyTracker {
    /// Map from source field -> fields that depend on it
    dependencies: HashMap<String, HashSet<String>>,
    /// Map from field -> fields it depends on (reverse lookup)
    reverse_deps: HashMap<String, HashSet<String>>,
}

impl DependencyTracker {
    pub fn new() -> Self {
        DependencyTracker {
            dependencies: HashMap::new(),
            reverse_deps: HashMap::new(),
        }
    }
    
    /// Record that `dependent` depends on `source`
    pub fn add_dependency(&mut self, dependent: &str, source: &str) {
        self.dependencies
            .entry(source.to_string())
            .or_default()
            .insert(dependent.to_string());
        
        self.reverse_deps
            .entry(dependent.to_string())
            .or_default()
            .insert(source.to_string());
    }
    
    /// Get all fields that should recalculate when `source` changes
    pub fn get_dependents(&self, source: &str) -> Vec<String> {
        self.dependencies
            .get(source)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }
    
    /// Get all fields that this field depends on
    pub fn get_sources(&self, dependent: &str) -> Vec<String> {
        self.reverse_deps
            .get(dependent)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }
    
    /// Clear dependencies for a field (useful when script changes)
    pub fn clear_for_field(&mut self, field: &str) {
        // Remove from reverse deps
        if let Some(sources) = self.reverse_deps.remove(field) {
            for source in sources {
                if let Some(deps) = self.dependencies.get_mut(&source) {
                    deps.remove(field);
                }
            }
        }
        // Remove as source
        self.dependencies.remove(field);
    }
}

// =============================================================================
// Form State
// =============================================================================

/// Node presence values per XFA spec
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[derive(Default)]
pub enum Presence {
    #[default]
    Visible,
    Invisible,
    Hidden,
    Inactive,
}


impl Presence {
    pub fn from_str(s: &str) -> Self {
        match s {
            "invisible" => Presence::Invisible,
            "hidden" => Presence::Hidden,
            "inactive" => Presence::Inactive,
            _ => Presence::Visible,
        }
    }
    
    /// Returns the XFA string representation of this presence value
    pub fn as_str(&self) -> &'static str {
        match self {
            Presence::Visible => "visible",
            Presence::Invisible => "invisible",
            Presence::Hidden => "hidden",
            Presence::Inactive => "inactive",
        }
    }
    
    /// Returns true if this presence should skip rendering
    pub fn should_skip_render(&self) -> bool {
        matches!(self, Presence::Hidden | Presence::Invisible | Presence::Inactive)
    }
    
    /// Returns true if this presence should skip layout (not take up space)
    pub fn should_skip_layout(&self) -> bool {
        matches!(self, Presence::Hidden | Presence::Inactive)
    }
}

/// Shared form data state that scripts can read/write
#[derive(Debug, Clone)]
pub struct FormState {
    /// Field values indexed by SOM path
    pub values: HashMap<String, XfaValue>,
    /// Field presence (visible/invisible/hidden/inactive)
    pub presence: HashMap<String, Presence>,
    /// Scripts can declare global variables
    pub global_variables: HashMap<String, XfaValue>,
}

impl FormState {
    pub fn new() -> Self {
        FormState {
            values: HashMap::new(),
            presence: HashMap::new(),
            global_variables: HashMap::new(),
        }
    }
    
    pub fn get_value(&self, path: &str) -> Option<&XfaValue> {
        if let Some(v) = self.values.get(path) {
            return Some(v);
        }
        let field_name = path.rsplit('.').next().unwrap_or(path);
        for (key, value) in &self.values {
            if key.ends_with(&format!(".{}", field_name)) || key == field_name {
                return Some(value);
            }
        }
        self.global_variables.get(path)
    }
    
    pub fn set_value(&mut self, path: String, value: XfaValue) {
        self.values.insert(path, value);
    }
    
    pub fn get_presence(&self, path: &str) -> Presence {
        self.presence.get(path).copied().unwrap_or_default()
    }
    
    pub fn set_presence(&mut self, path: String, presence: Presence) {
        self.presence.insert(path, presence);
    }
}

pub type SharedFormState = Arc<RwLock<FormState>>;

/// XFA Field object exposed to JavaScript
#[derive(Debug, Clone, Trace, Finalize)]
pub struct XfaFieldObject {
    #[unsafe_ignore_trace]
    pub name: String,
    #[unsafe_ignore_trace]
    pub path: String,
    #[unsafe_ignore_trace]
    pub raw_value: GcRefCell<String>,
}

impl XfaFieldObject {
    pub fn new(name: String, path: String, initial_value: String) -> Self {
        XfaFieldObject {
            name,
            path,
            raw_value: GcRefCell::new(initial_value),
        }
    }
}

// =============================================================================
// XFA Scripting Engine
// =============================================================================

/// XFA Scripting Engine with XFA 3.3 spec compliance
pub struct XfaScriptEngine {
    context: Context,
    form_state: SharedFormState,
    current_field_path: Option<String>,
    /// SOM resolver for resolveNode()/resolveNodes()
    som_resolver: SomResolver,
    /// Dependency tracker for cascading calculations
    dependencies: DependencyTracker,
    /// Registered field JS objects for resolveNode results
    field_objects: HashMap<String, JsObject>,
    /// Maps child field names to their unique IDs in the current context
    /// This is used to store computed values by ID instead of name to avoid collisions
    /// when multiple subforms have same-named children.
    child_name_to_id: HashMap<String, String>,
    /// Maps radio button paths to their parent exclGroup paths
    /// Per XFA spec, when a radio button's rawValue is set to its "on" value,
    /// the parent exclGroup's rawValue should also be updated.
    exclgroup_child_to_parent: HashMap<String, String>,
}

impl XfaScriptEngine {
    pub fn new() -> Self {
        let context = Context::default();
        let form_state = Arc::new(RwLock::new(FormState::new()));
        
        let mut engine = XfaScriptEngine {
            context,
            form_state,
            current_field_path: None,
            som_resolver: SomResolver::new(),
            dependencies: DependencyTracker::new(),
            field_objects: HashMap::new(),
            child_name_to_id: HashMap::new(),
            exclgroup_child_to_parent: HashMap::new(),
        };
        
        engine.setup_environment();
        engine
    }
    
    pub fn with_state(form_state: SharedFormState) -> Self {
        let context = Context::default();
        
        let mut engine = XfaScriptEngine {
            context,
            form_state,
            current_field_path: None,
            som_resolver: SomResolver::new(),
            dependencies: DependencyTracker::new(),
            field_objects: HashMap::new(),
            child_name_to_id: HashMap::new(),
            exclgroup_child_to_parent: HashMap::new(),
        };
        
        engine.setup_environment();
        engine
    }
    
    fn setup_environment(&mut self) {
        self.setup_xfa_object();
        self.setup_shortcuts();
        self.setup_field_registry();
        self.setup_som_fallback();
    }
    
    /// Create a global field registry that resolveNode can access
    fn setup_field_registry(&mut self) {
        // Create _xfa_fields_ global object that maps field names to their JS objects
        // This allows resolveNode to find fields by name
        let registry = ObjectInitializer::new(&mut self.context).build();
        self.context.register_global_property(js_string!("_xfa_fields_"), registry, Attribute::all()).ok();
    }
    
    /// Set up a fallback mechanism for SOM path resolution.
    /// 
    /// In XFA, embedded fields appear in the SOM at their embed location. For example,
    /// a floating field "ffrb1" embedded in "Page.SectionTitle.STP_SectionTitle" should
    /// be accessible as "Page.SectionTitle.STP_SectionTitle.ffrb1".
    /// 
    /// Since tracking exact embed locations is complex, we provide a global helper
    /// that scripts can use when direct property access fails.
    fn setup_som_fallback(&mut self) {
        // Create helper functions for SOM resolution and exclGroup sync
        let helpers_js = r#"
            // Global SOM resolution helper
            // When a path like "Page.SectionTitle.STP_SectionTitle.ffrb1" is accessed,
            // JavaScript property chain works for subforms but fails for floating fields.
            // This helper provides fallback resolution.
            function _xfa_resolve_path_(path) {
                var parts = path.split('.');
                var obj = this; // Start from global
                
                // Try to traverse the path
                for (var i = 0; i < parts.length; i++) {
                    var part = parts[i];
                    if (obj && typeof obj[part] !== 'undefined') {
                        obj = obj[part];
                    } else {
                        // Path traversal failed - try looking up the last part in the registry
                        var lastPart = parts[parts.length - 1];
                        if (typeof _xfa_fields_ !== 'undefined' && _xfa_fields_[lastPart]) {
                            return _xfa_fields_[lastPart];
                        }
                        return null;
                    }
                }
                return obj;
            }
            
            // Exclusion group value sync helper
            // Per XFA spec, when a radio button's rawValue is set, the parent exclGroup should update.
            // This stores known parent-child relationships that get synced.
            var _xfa_exclgroup_map_ = {};
            
            // Register an exclGroup parent-child relationship
            function _xfa_register_exclgroup_(childPath, parentPath) {
                _xfa_exclgroup_map_[childPath] = parentPath;
            }
            
            // Sync all exclGroup values based on their children
            function _xfa_sync_exclgroups_() {
                for (var childPath in _xfa_exclgroup_map_) {
                    var parentPath = _xfa_exclgroup_map_[childPath];
                    try {
                        // Navigate to child object
                        var childParts = childPath.split('.');
                        var child = this;
                        for (var i = 0; i < childParts.length; i++) {
                            if (child && child[childParts[i]]) {
                                child = child[childParts[i]];
                            } else {
                                child = null;
                                break;
                            }
                        }
                        
                        // Navigate to parent object
                        var parentParts = parentPath.split('.');
                        var parent = this;
                        for (var i = 0; i < parentParts.length; i++) {
                            if (parent && parent[parentParts[i]]) {
                                parent = parent[parentParts[i]];
                            } else {
                                parent = null;
                                break;
                            }
                        }
                        
                        // If child has a value, set parent's value
                        if (child && parent && child.rawValue && child.rawValue !== '' && child.rawValue !== '0') {
                            parent.rawValue = child.rawValue;
                        }
                    } catch (e) {
                        // Ignore errors during sync
                    }
                }
            }
        "#;
        
        let _ = self.execute_variable_script(helpers_js);
    }
    
    /// Create the xfa root object with resolveNode/resolveNodes
    fn setup_xfa_object(&mut self) {
        let xfa = ObjectInitializer::new(&mut self.context).build();
        
        // Create xfa.form (Form DOM)
        let form = ObjectInitializer::new(&mut self.context).build();
        
        // Create xfa.datasets and xfa.datasets.data (Data DOM)
        let data = ObjectInitializer::new(&mut self.context).build();
        let datasets = ObjectInitializer::new(&mut self.context)
            .property(js_string!("data"), data.clone(), Attribute::all())
            .build();
        
        let template = ObjectInitializer::new(&mut self.context).build();
        let layout = ObjectInitializer::new(&mut self.context).build();
        let host = self.create_host_object();
        
        let event = ObjectInitializer::new(&mut self.context)
            .property(js_string!("name"), JsValue::from(js_string!("")), Attribute::all())
            .property(js_string!("target"), JsValue::null(), Attribute::all())
            .property(js_string!("cancelAction"), JsValue::from(false), Attribute::all())
            .build();
        
        xfa.set(PropertyKey::from(js_string!("form")), form, false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("datasets")), datasets, false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("data")), data.clone(), false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("template")), template, false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("layout")), layout, false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("host")), host, false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("event")), event, false, &mut self.context).ok();
        
        // Add resolveNode as a global function (XFA 3.3 spec page 106)
        // This function looks up nodes by name from the _xfa_fields_ registry
        let resolve_node_fn = NativeFunction::from_fn_ptr(|_this, args, context| {
            let expr = args.get_or_undefined(0).to_string(context)?;
            let expr_str = expr.to_std_string_escaped();
            
            // Extract the field name from the expression
            // Handle both simple names ("ffrb1") and paths ("Page.FormTitle.ffrb1")
            let field_name = expr_str.rsplit('.').next().unwrap_or(&expr_str);
            
            // Look up in the global field registry
            if let Ok(registry) = context.global_object()
                .get(PropertyKey::from(js_string!("_xfa_fields_")), context)
                && let Some(registry_obj) = registry.as_object()
                    && let Ok(field_obj) = registry_obj.get(
                        PropertyKey::from(JsString::from(field_name)), 
                        context
                    )
                        && !field_obj.is_undefined() && !field_obj.is_null() {
                            return Ok(field_obj);
                        }
            
            // Also try looking up as a global (for backward compatibility)
            if let Ok(global_field) = context.global_object()
                .get(PropertyKey::from(JsString::from(field_name)), context)
                && !global_field.is_undefined() && !global_field.is_null() {
                    // Check if it has rawValue (is a field object)
                    if let Some(obj) = global_field.as_object()
                        && let Ok(raw) = obj.get(PropertyKey::from(js_string!("rawValue")), context)
                            && !raw.is_undefined() {
                                return Ok(global_field);
                            }
                }
            
            // Return null if not found
            Ok(JsValue::null())
        });
        xfa.set(
            PropertyKey::from(js_string!("resolveNode")),
            resolve_node_fn.to_js_function(self.context.realm()),
            false,
            &mut self.context
        ).ok();
        
        // Add resolveNodes function
        let resolve_nodes_fn = NativeFunction::from_fn_ptr(|_this, args, context| {
            let _expr = args.get_or_undefined(0).to_string(context)?;
            // Return empty array for now
            Ok(JsValue::from(ObjectInitializer::new(context).build()))
        });
        xfa.set(
            PropertyKey::from(js_string!("resolveNodes")),
            resolve_nodes_fn.to_js_function(self.context.realm()),
            false,
            &mut self.context
        ).ok();
        
        self.context.register_global_property(js_string!("xfa"), xfa, Attribute::all()).ok();
    }
    
    fn create_host_object(&mut self) -> JsObject {
        let message_box = NativeFunction::from_fn_ptr(|_this, args, context| {
            let message = args.get_or_undefined(0).to_string(context)?;
            eprintln!("[XFA messageBox]: {}", message.to_std_string_escaped());
            Ok(JsValue::undefined())
        });
        
        let set_focus = NativeFunction::from_fn_ptr(|_this, _args, _context| {
            Ok(JsValue::undefined())
        });
        
        ObjectInitializer::new(&mut self.context)
            .property(js_string!("name"), JsValue::from(js_string!("Blueprint")), Attribute::READONLY)
            .property(js_string!("version"), JsValue::from(js_string!("1.0")), Attribute::READONLY)
            .function(message_box, js_string!("messageBox"), 1)
            .function(set_focus, js_string!("setFocus"), 1)
            .build()
    }
    
    fn setup_shortcuts(&mut self) {
        let xfa = self.context.global_object()
            .get(PropertyKey::from(js_string!("xfa")), &mut self.context)
            .unwrap_or(JsValue::undefined());
        
        if let Some(xfa_obj) = xfa.as_object() {
            if let Ok(form) = xfa_obj.get(PropertyKey::from(js_string!("form")), &mut self.context) {
                self.context.register_global_property(js_string!("$form"), form, Attribute::all()).ok();
            }
            if let Ok(datasets) = xfa_obj.get(PropertyKey::from(js_string!("datasets")), &mut self.context)
                && let Some(ds_obj) = datasets.as_object()
                    && let Ok(data) = ds_obj.get(PropertyKey::from(js_string!("data")), &mut self.context) {
                        self.context.register_global_property(js_string!("$data"), data, Attribute::all()).ok();
                    }
            if let Ok(template) = xfa_obj.get(PropertyKey::from(js_string!("template")), &mut self.context) {
                self.context.register_global_property(js_string!("$template"), template, Attribute::all()).ok();
            }
            if let Ok(layout) = xfa_obj.get(PropertyKey::from(js_string!("layout")), &mut self.context) {
                self.context.register_global_property(js_string!("$layout"), layout, Attribute::all()).ok();
            }
            if let Ok(host) = xfa_obj.get(PropertyKey::from(js_string!("host")), &mut self.context) {
                self.context.register_global_property(js_string!("$host"), host, Attribute::all()).ok();
            }
            if let Ok(event) = xfa_obj.get(PropertyKey::from(js_string!("event")), &mut self.context) {
                self.context.register_global_property(js_string!("$event"), event, Attribute::all()).ok();
            }
            self.context.register_global_property(js_string!("$xfa"), xfa, Attribute::all()).ok();
        }
    }
    
    /// Register a field with SOM resolver
    pub fn register_field(&mut self, path: &str, name: &str, value: &str) {
        // Register in SOM resolver
        let parent_path = path.rsplit_once('.').map(|(p, _)| p);
        self.som_resolver.register_node(path, name, "field", parent_path);
        
        // Store in form state
        {
            let mut state = self.form_state.write().unwrap();
            state.set_value(path.to_string(), XfaValue::String(value.to_string()));
        }
        
        // Create JavaScript object
        let field_obj = self.create_field_object(name, path, value);
        self.field_objects.insert(path.to_string(), field_obj.clone());
        
        // Register globally for naked references
        self.context.register_global_property(
            JsString::from(name),
            field_obj.clone(),
            Attribute::all()
        ).ok();
        
        // Register on $form
        let xfa = self.context.global_object()
            .get(PropertyKey::from(js_string!("xfa")), &mut self.context)
            .unwrap_or(JsValue::undefined());
        
        if let Some(xfa_obj) = xfa.as_object()
            && let Ok(form) = xfa_obj.get(PropertyKey::from(js_string!("form")), &mut self.context)
                && let Some(form_obj) = form.as_object() {
                    self.register_path_on_object(form_obj, path, field_obj);
                }
    }
    
    fn create_field_object(&mut self, name: &str, path: &str, initial_value: &str) -> JsObject {
        let name_js = js_string!(name);
        let path_js = js_string!(path);
        
        let field = ObjectInitializer::new(&mut self.context)
            .property(js_string!("name"), JsValue::from(name_js.clone()), Attribute::READONLY)
            .property(js_string!("somExpression"), JsValue::from(path_js), Attribute::READONLY)
            .build();
        
        field.set(
            PropertyKey::from(js_string!("rawValue")),
            JsValue::from(js_string!(initial_value)),
            false,
            &mut self.context
        ).ok();
        
        field.set(
            PropertyKey::from(js_string!("value")),
            JsValue::from(js_string!(initial_value)),
            false,
            &mut self.context
        ).ok();
        
        // Add presence property (XFA spec)
        field.set(
            PropertyKey::from(js_string!("presence")),
            JsValue::from(js_string!("visible")),
            false,
            &mut self.context
        ).ok();
        
        field
    }
    
    fn register_path_on_object(&mut self, root: &JsObject, path: &str, field_obj: JsObject) {
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = root.clone();
        
        for (i, part) in parts.iter().enumerate() {
            let key = PropertyKey::from(js_string!(*part));
            
            if i == parts.len() - 1 {
                current.set(key, field_obj.clone(), false, &mut self.context).ok();
            } else {
                let existing = current.get(key.clone(), &mut self.context).unwrap_or(JsValue::undefined());
                
                if existing.is_undefined() {
                    let intermediate = ObjectInitializer::new(&mut self.context).build();
                    current.set(key.clone(), intermediate.clone(), false, &mut self.context).ok();
                    current = intermediate;
                } else if let Some(obj) = existing.as_object() {
                    current = obj.clone();
                } else {
                    break;
                }
            }
        }
    }
    
    pub fn register_global_variable(&mut self, name: &str, value: JsObject) {
        self.context.register_global_property(
            JsString::from(name),
            value,
            Attribute::all()
        ).ok();
    }
    
    pub fn register_translation_object(&mut self, name: &str, translations: HashMap<String, String>) {
        let obj = ObjectInitializer::new(&mut self.context).build();
        
        for (key, value) in translations {
            obj.set(
                PropertyKey::from(JsString::from(key.as_str())),
                JsValue::from(js_string!(value.as_str())),
                false,
                &mut self.context
            ).ok();
        }
        
        self.context.register_global_property(
            JsString::from(name),
            obj,
            Attribute::all()
        ).ok();
    }
    
    /// Record a dependency for cascading calculations
    pub fn add_dependency(&mut self, dependent_field: &str, source_field: &str) {
        self.dependencies.add_dependency(dependent_field, source_field);
    }
    
    /// Get fields that need recalculation when a value changes
    pub fn get_fields_to_recalculate(&self, changed_field: &str) -> Vec<String> {
        self.dependencies.get_dependents(changed_field)
    }
    
    /// Resolve a SOM expression (for use from Rust side)
    pub fn resolve_node(&self, som_expression: &str) -> Option<String> {
        self.som_resolver.resolve_node(som_expression, self.current_field_path.as_deref())
    }
    
    /// Resolve a SOM expression to multiple nodes
    pub fn resolve_nodes(&self, som_expression: &str) -> Vec<String> {
        self.som_resolver.resolve_nodes(som_expression, self.current_field_path.as_deref())
    }
    
    pub fn execute_variable_script(&mut self, source: &str) -> Result<(), String> {
        match self.context.eval(Source::from_bytes(source)) {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("Variable script error: {}", e)),
        }
    }
    
    pub fn evaluate_expression(&mut self, source: &str) -> Result<String, String> {
        match self.context.eval(Source::from_bytes(source)) {
            Ok(val) => {
                let s = val.to_string(&mut self.context)
                    .map(|js_str| js_str.to_std_string_escaped())
                    .unwrap_or_else(|_| "<<error>>".to_string());
                Ok(s)
            }
            Err(e) => Err(format!("Evaluation error: {}", e)),
        }
    }
    
    pub fn set_current_field(&mut self, path: &str, name: &str, value: &str) {
        self.current_field_path = Some(path.to_string());
        let this_obj = self.create_field_object(name, path, value);
        // Use _xfa_this_ as the property name since "this" is a reserved keyword in JS
        // The execute_javascript method wraps scripts with .call(_xfa_this_)
        self.context.register_global_property(
            js_string!("_xfa_this_"),
            this_obj,
            Attribute::all()
        ).ok();
    }
    
    /// Set up the current field context with child fields as properties of `this`.
    /// 
    /// Per XFA 3.3 spec (Chapter 3, "Scripting Object Model"):
    /// Child elements of a container are accessible as properties on that container.
    /// This enables scripts like: `this.ffDesSignature.rawValue = mySignatureClient`
    /// where `ffDesSignature` is a child field of the current subform.
    /// The `children` parameter is a list of (child_name, child_id) pairs.
    pub fn set_current_field_with_children(&mut self, path: &str, name: &str, value: &str, children: &[(String, String)]) {
        self.current_field_path = Some(path.to_string());
        let this_obj = self.create_field_object(name, path, value);
        
        // Track which child names map to which IDs for later retrieval
        self.child_name_to_id.clear();
        
        // Add child fields as properties of `this`
        // Each child becomes accessible as this.childName with rawValue property
        for (child_name, child_id) in children {
            let child_path = format!("{}.{}", path, child_name);
            let child_obj = self.create_field_object(child_name, &child_path, "");
            
            // Track the name->id mapping for this context
            self.child_name_to_id.insert(child_name.clone(), child_id.clone());
            
            // Use define_property_or_throw for more reliable property definition
            let property_key = PropertyKey::from(JsString::from(child_name.as_str()));
            
            let set_result = this_obj.define_property_or_throw(
                property_key.clone(),
                boa_engine::property::PropertyDescriptor::builder()
                    .value(child_obj.clone())
                    .writable(true)
                    .enumerable(true)
                    .configurable(true)
                    .build(),
                &mut self.context
            );
            
            if let Err(e) = &set_result {
                eprintln!("Warning: Failed to define child property '{}': {:?}", child_name, e);
            }
            
            // Also store the child object for later value retrieval
            self.field_objects.insert(child_path, child_obj);
        }
        
        // Register as _xfa_this_ globally (used by execute_javascript's .call(_xfa_this_))
        self.context.register_global_property(
            js_string!("_xfa_this_"),
            this_obj.clone(),
            Attribute::all()
        ).ok();
    }
    
    /// Get the value of a child field that was set via `this.childName.rawValue = ...`
    /// Returns (child_id, value) if found, so values can be stored by unique ID.
    /// This is critical for avoiding collisions when multiple subforms have same-named children.
    pub fn get_child_field_value(&mut self, child_name: &str) -> Option<(String, String)> {
        // Get the child's unique ID from our mapping
        let child_id = self.child_name_to_id.get(child_name).cloned().unwrap_or_default();
        
        // First, try to get it from `_xfa_this_.childName.rawValue`
        if let Ok(this_val) = self.context.global_object()
            .get(PropertyKey::from(js_string!("_xfa_this_")), &mut self.context)
            && let Some(this_obj) = this_val.as_object()
                && let Ok(child_val) = this_obj.get(
                    PropertyKey::from(JsString::from(child_name)),
                    &mut self.context
                )
                    && let Some(child_obj) = child_val.as_object()
                        && let Ok(raw_value) = child_obj.get(
                            PropertyKey::from(js_string!("rawValue")),
                            &mut self.context
                        )
                            && !raw_value.is_undefined() && !raw_value.is_null() {
                                let value = raw_value.to_string(&mut self.context)
                                    .ok()
                                    .map(|s| s.to_std_string_escaped())?;
                                return Some((child_id, value));
                            }
        
        // Fallback: check form state
        let state = self.form_state.read().ok()?;
        state.get_value(child_name).map(|v| (child_id, v.as_string()))
    }
    
    /// Get the value of a field from the SOM hierarchy by its full path.
    /// This is used to retrieve values set via SOM path references like:
    /// `Page.FormTitle.STP_RB_Horizontal.RB_Group_Neuanlage.RB_1.rawValue = 1`
    pub fn get_som_field_value(&mut self, path: &str) -> Option<String> {
        if let Some(field_obj) = self.field_objects.get(path)
            && let Ok(raw_value) = field_obj.get(
                PropertyKey::from(js_string!("rawValue")),
                &mut self.context
            )
                && !raw_value.is_undefined() && !raw_value.is_null() {
                    return raw_value.to_string(&mut self.context)
                        .ok()
                        .map(|s| s.to_std_string_escaped());
                }
        None
    }
    
    /// Sync exclusion group values based on their children's rawValues.
    /// 
    /// Per XFA 3.3 spec: When a radio button's rawValue is set to its "on" value,
    /// the parent exclGroup's rawValue should also be updated to that value.
    /// This ensures scripts that check the exclGroup's value work correctly.
    fn sync_exclgroup_values(&mut self) {
        // Clone the mappings to avoid borrow issues
        let child_to_parent: Vec<(String, String)> = self.exclgroup_child_to_parent.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        
        for (child_path, parent_path) in child_to_parent {
            // Get the child's rawValue
            if let Some(child_obj) = self.field_objects.get(&child_path)
                && let Ok(raw_value) = child_obj.get(
                    PropertyKey::from(js_string!("rawValue")),
                    &mut self.context
                )
                    && !raw_value.is_undefined() && !raw_value.is_null()
                        && let Ok(value_str) = raw_value.to_string(&mut self.context) {
                            let value = value_str.to_std_string_escaped();
                            // If the child has a non-empty value, propagate to parent exclGroup
                            if !value.is_empty() && value != "0" {
                                // Set the parent exclGroup's rawValue
                                if let Some(parent_obj) = self.field_objects.get(&parent_path) {
                                    parent_obj.set(
                                        PropertyKey::from(js_string!("rawValue")),
                                        JsValue::from(js_string!(value.as_str())),
                                        false,
                                        &mut self.context
                                    ).ok();
                                }
                            }
                        }
        }
    }
    
    /// Get all field values from the SOM hierarchy that have been modified.
    /// Returns a map of field name -> value for all fields with non-empty values.
    /// This is useful for collecting values set via SOM path references after script execution.
    pub fn get_all_som_field_values(&mut self) -> std::collections::HashMap<String, String> {
        let mut values = std::collections::HashMap::new();
        
        for (path, obj) in &self.field_objects {
            if let Ok(raw_value) = obj.get(
                PropertyKey::from(js_string!("rawValue")),
                &mut self.context
            )
                && !raw_value.is_undefined() && !raw_value.is_null()
                    && let Ok(value_str) = raw_value.to_string(&mut self.context) {
                        let value = value_str.to_std_string_escaped();
                        if !value.is_empty() {
                            // Extract just the field name from the path for lookup
                            // e.g., "Page.FormTitle.STP_RB_Horizontal.RB_Group_Neuanlage.RB_1" -> "RB_1"
                            let field_name = path.rsplit('.').next().unwrap_or(path);
                            values.insert(field_name.to_string(), value);
                        }
                    }
        }
        
        values
    }
    
    pub fn execute_script(&mut self, script: &XfaScript) -> Result<Option<String>, String> {
        match script.content_type {
            ScriptContentType::JavaScript => self.execute_javascript(&script.source),
            ScriptContentType::FormCalc => {
                // FormCalc: Per XFA spec, this is the default language
                // A full implementation would transpile FormCalc to JS
                Err("FormCalc scripts require transpilation (not yet implemented). \
                     Per XFA 3.3 spec Chapter 11, FormCalc is the default script language.".to_string())
            }
        }
    }
    
    fn execute_javascript(&mut self, source: &str) -> Result<Option<String>, String> {
        let this_obj = self.context.global_object()
            .get(PropertyKey::from(js_string!("_xfa_this_")), &mut self.context)
            .ok();
        
        let has_this_context = this_obj.as_ref().map(|v| !v.is_undefined()).unwrap_or(false);
        
        let initial_raw_value = if let Some(ref this_val) = this_obj {
            if let Some(obj) = this_val.as_object() {
                obj.get(PropertyKey::from(js_string!("rawValue")), &mut self.context)
                    .ok()
                    .and_then(|v| v.to_string(&mut self.context).ok())
                    .map(|s| s.to_std_string_escaped())
            } else {
                None
            }
        } else {
            None
        };
        
        // Wrap the script in a function that provides proper `this` binding if available
        // We use _xfa_this_ as a global property and then call the function with it as `this`
        // The script can then use `this.fieldName.rawValue` as expected per XFA spec
        // If no this context is set, run the script without the .call() wrapper
        let wrapped_source = if has_this_context {
            format!(
                "(function() {{ {} }}).call(_xfa_this_)",
                source
            )
        } else {
            // No this context - run script in global scope
            format!("(function() {{ {} }})()", source)
        };
        
        match self.context.eval(Source::from_bytes(&wrapped_source)) {
            Ok(result) => {
                // Sync exclusion group values after script execution
                // Per XFA spec, when a radio button's rawValue is set, the parent exclGroup's value should update
                self.sync_exclgroup_values();
                
                if let Ok(this_val) = self.context.global_object()
                    .get(PropertyKey::from(js_string!("_xfa_this_")), &mut self.context)
                    && let Some(this_obj) = this_val.as_object()
                        && let Ok(raw_value) = this_obj
                            .get(PropertyKey::from(js_string!("rawValue")), &mut self.context)
                            && !raw_value.is_undefined() && !raw_value.is_null() {
                                let value_str = raw_value.to_string(&mut self.context)
                                    .map(|s| s.to_std_string_escaped())
                                    .unwrap_or_default();
                                
                                let changed = initial_raw_value.as_ref() != Some(&value_str);
                                
                                if changed {
                                    if let Some(ref path) = self.current_field_path {
                                        let mut state = self.form_state.write().unwrap();
                                        state.set_value(path.clone(), XfaValue::String(value_str.clone()));
                                    }
                                    return Ok(Some(value_str));
                                }
                            }
                
                if result.is_undefined() || result.is_null() {
                    Ok(None)
                } else {
                    Ok(Some(result.to_string(&mut self.context)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default()))
                }
            }
            Err(e) => Err(format!("JavaScript error: {}", e)),
        }
    }
    
    pub fn get_field_value(&self, path: &str) -> Option<String> {
        let state = self.form_state.read().ok()?;
        state.get_value(path).map(|v| v.as_string())
    }
    
    pub fn form_state(&self) -> &SharedFormState {
        &self.form_state
    }
    
    /// Access to dependency tracker
    pub fn dependencies(&self) -> &DependencyTracker {
        &self.dependencies
    }
    
    /// Access to SOM resolver
    pub fn som_resolver(&self) -> &SomResolver {
        &self.som_resolver
    }
    
    /// Register an XFA node (subform or field) in the SOM hierarchy.
    /// 
    /// Per XFA 3.3 spec Chapter 3 ("Scripting Object Model"):
    /// - Subforms and fields are accessible as named properties on their parent
    /// - Top-level subforms are accessible as global variables
    /// - The hierarchy enables references like `Page.FormTitle.STP_RB_Horizontal.RB_Group_Neuanlage.rawValue`
    /// 
    /// Parameters:
    /// - `name`: The node's name attribute
    /// - `path`: Full SOM path (e.g., "Page.FormTitle.STP_RB_Horizontal")
    /// - `parent_path`: Parent's path (None for top-level subforms)
    /// - `is_field`: true if this is a field, false if subform
    /// - `value`: Initial rawValue for fields
    pub fn register_xfa_node(&mut self, name: &str, path: &str, parent_path: Option<&str>, is_field: bool, value: &str) {
        // Track exclGroup parent-child relationships for value propagation
        // If the parent path contains "RB_Group" or similar exclGroup naming,
        // this field is a radio button child of an exclGroup
        if let Some(parent) = parent_path {
            // Heuristic: if parent name contains "Group" and starts with "RB_" or we're registering RB_ fields
            let is_exclgroup_child = parent.contains("_Group") && name.starts_with("RB_");
            if is_exclgroup_child {
                self.exclgroup_child_to_parent.insert(path.to_string(), parent.to_string());
                
                // Also register in JavaScript for in-script sync
                let js_register = format!(
                    "_xfa_register_exclgroup_('{}', '{}');",
                    path.replace('\'', "\\'"),
                    parent.replace('\'', "\\'")
                );
                let _ = self.context.eval(Source::from_bytes(&js_register));
            }
        }
        
        // Create the JavaScript object for this node
        let node_obj = if is_field {
            self.create_field_object(name, path, value)
        } else {
            // For subforms, create an object that can have children
            ObjectInitializer::new(&mut self.context)
                .property(js_string!("name"), JsValue::from(js_string!(name)), Attribute::READONLY)
                .property(js_string!("somExpression"), JsValue::from(js_string!(path)), Attribute::READONLY)
                .build()
        };
        
        // Store in field_objects for later lookup
        self.field_objects.insert(path.to_string(), node_obj.clone());
        
        // Register in SOM resolver
        self.som_resolver.register_node(path, name, if is_field { "field" } else { "subform" }, parent_path);
        
        // If there's a parent, add this node as a child property
        if let Some(parent) = parent_path {
            if let Some(parent_obj) = self.field_objects.get(parent) {
                parent_obj.set(
                    PropertyKey::from(JsString::from(name)),
                    node_obj.clone(),
                    false,
                    &mut self.context
                ).ok();
            }
        } else {
            // Top-level subform - register as a global variable
            // This enables scripts to reference like: Page.FormTitle...
            self.context.register_global_property(
                JsString::from(name),
                node_obj.clone(),
                Attribute::all()
            ).ok();
        }
        
        // Also register in the _xfa_fields_ registry for resolveNode() lookups
        // This allows change() functions to find fields via xfa.resolveNode("fieldName")
        if let Ok(registry) = self.context.global_object()
            .get(PropertyKey::from(js_string!("_xfa_fields_")), &mut self.context)
            && let Some(registry_obj) = registry.as_object() {
                registry_obj.set(
                    PropertyKey::from(JsString::from(name)),
                    node_obj.clone(),
                    false,
                    &mut self.context
                ).ok();
            }
        
        // For floating fields (registered without parent), also add as property on all existing subforms
        // This enables SOM path access like "Page.SectionTitle.STP_SectionTitle.ffrb1" where
        // ffrb1 is a floating field that's embedded somewhere in the STP_SectionTitle subtree
        if is_field && parent_path.is_none() {
            for subform_obj in self.field_objects.values() {
                // Only add to subforms (objects that have 'somExpression' property)
                if let Ok(som) = subform_obj.get(
                    PropertyKey::from(js_string!("somExpression")),
                    &mut self.context
                )
                    && !som.is_undefined() {
                        // This is a subform, add the floating field as a property
                        subform_obj.set(
                            PropertyKey::from(JsString::from(name)),
                            node_obj.clone(),
                            false,
                            &mut self.context
                        ).ok();
                    }
            }
        }
    }
    
    /// Get the current presence value set on `this` by a script.
    /// Returns the presence value if it was set, None otherwise.
    pub fn get_current_field_presence(&mut self) -> Option<Presence> {
        if let Ok(this_val) = self.context.global_object()
            .get(PropertyKey::from(js_string!("_xfa_this_")), &mut self.context)
            && let Some(this_obj) = this_val.as_object()
                && let Ok(presence) = this_obj.get(
                    PropertyKey::from(js_string!("presence")),
                    &mut self.context
                )
                    && !presence.is_undefined() && !presence.is_null() {
                        let presence_str = presence.to_string(&mut self.context)
                            .ok()
                            .map(|s| s.to_std_string_escaped())?;
                        // Only return if it's a valid XFA presence value
                        if matches!(presence_str.as_str(), "visible" | "invisible" | "hidden" | "inactive") {
                            return Some(Presence::from_str(&presence_str));
                        }
                    }
        None
    }
    
    /// Get the presence value of a child field that was set via `this.childName.presence = ...`
    /// Returns (child_id, presence) if found.
    pub fn get_child_field_presence(&mut self, child_name: &str) -> Option<(String, Presence)> {
        // Get the child's unique ID from our mapping
        let child_id = self.child_name_to_id.get(child_name).cloned().unwrap_or_default();
        
        // Try to get it from `_xfa_this_.childName.presence`
        if let Ok(this_val) = self.context.global_object()
            .get(PropertyKey::from(js_string!("_xfa_this_")), &mut self.context)
            && let Some(this_obj) = this_val.as_object()
                && let Ok(child_val) = this_obj.get(
                    PropertyKey::from(JsString::from(child_name)),
                    &mut self.context
                )
                    && let Some(child_obj) = child_val.as_object()
                        && let Ok(presence) = child_obj.get(
                            PropertyKey::from(js_string!("presence")),
                            &mut self.context
                        )
                            && !presence.is_undefined() && !presence.is_null() {
                                let presence_str = presence.to_string(&mut self.context)
                                    .ok()
                                    .map(|s| s.to_std_string_escaped())?;
                                // Only return if it's a valid XFA presence value
                                if matches!(presence_str.as_str(), "visible" | "invisible" | "hidden" | "inactive") {
                                    return Some((child_id, Presence::from_str(&presence_str)));
                                }
                            }
        None
    }
}

// =============================================================================
// Parsing functions
// =============================================================================

pub fn parse_events_from_node(children: &[crate::xfa::XfaNode]) -> Vec<XfaScript> {
    let mut scripts = Vec::new();
    
    for child in children {
        if let crate::xfa::XfaNodeKind::Element { tag_name, .. } = &child.kind
            && tag_name == "event"
                && let Some(script) = parse_event_element(child) {
                    scripts.push(script);
                }
    }
    
    scripts
}

fn parse_event_element(event_node: &crate::xfa::XfaNode) -> Option<XfaScript> {
    let activity = event_node.attributes.get("activity")
        .and_then(|s| s.parse().ok())
        .unwrap_or(EventActivity::Other("unknown".to_string()));
    
    let event_ref = event_node.attributes.get("ref")
        .and_then(|s| s.parse().ok())
        .unwrap_or(EventRef::Current);
    
    let name = event_node.attributes.get("name").cloned();
    
    for child in &event_node.children {
        if let crate::xfa::XfaNodeKind::Element { tag_name, text_content } = &child.kind
            && tag_name == "script" {
                let content_type = child.attributes.get("contentType")
                    .and_then(|s| ScriptContentType::from_content_type(s))
                    .unwrap_or(ScriptContentType::FormCalc);
                
                let run_at = child.attributes.get("runAt")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();
                
                let source = text_content.clone().unwrap_or_default();
                
                if !source.trim().is_empty() {
                    return Some(XfaScript {
                        source,
                        content_type,
                        activity,
                        event_ref,
                        name,
                        run_at,
                    });
                }
            }
    }
    
    None
}

pub fn parse_variables_from_node(children: &[crate::xfa::XfaNode]) -> HashMap<String, HashMap<String, String>> {
    let mut variables = HashMap::new();
    
    for child in children {
        if let crate::xfa::XfaNodeKind::Element { tag_name, .. } = &child.kind
            && tag_name == "variables" {
                for var_child in &child.children {
                    if let crate::xfa::XfaNodeKind::Element { tag_name: var_tag, .. } = &var_child.kind
                        && var_tag == "script"
                            && let Some(name) = var_child.attributes.get("name") {
                                variables.insert(name.clone(), HashMap::new());
                            }
                }
            }
    }
    
    variables
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    
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
    
    #[test]
    fn test_som_resolver_basic() {
        let mut resolver = SomResolver::new();
        resolver.register_node("Page.Header.txtlanguage", "txtlanguage", "field", Some("Page.Header"));
        resolver.register_node("Page.Body.Name", "Name", "field", Some("Page.Body"));
        
        // Test simple name resolution
        let result = resolver.resolve_node("txtlanguage", None);
        assert_eq!(result, Some("Page.Header.txtlanguage".to_string()));
        
        // Test full path resolution
        let result = resolver.resolve_node("Page.Header.txtlanguage", None);
        assert_eq!(result, Some("Page.Header.txtlanguage".to_string()));
    }
    
    #[test]
    fn test_som_resolver_indexed() {
        let mut resolver = SomResolver::new();
        resolver.register_node("Detail.Item", "Item", "field", Some("Detail"));
        resolver.register_node("Detail.Item", "Item", "field", Some("Detail"));
        resolver.register_node("Detail.Item", "Item", "field", Some("Detail"));
        
        // Test [0] index
        let result = resolver.resolve_nodes("Item[0]", None);
        assert_eq!(result.len(), 1);
        
        // Test [*] all instances
        let result = resolver.resolve_nodes("Item[*]", None);
        assert_eq!(result.len(), 3);
    }
    
    #[test]
    fn test_dependency_tracker() {
        let mut tracker = DependencyTracker::new();
        
        tracker.add_dependency("Total", "Price");
        tracker.add_dependency("Total", "Quantity");
        tracker.add_dependency("GrandTotal", "Total");
        
        // When Price changes, Total should recalculate
        let deps = tracker.get_dependents("Price");
        assert!(deps.contains(&"Total".to_string()));
        
        // When Total changes, GrandTotal should recalculate
        let deps = tracker.get_dependents("Total");
        assert!(deps.contains(&"GrandTotal".to_string()));
    }
    
    #[test]
    fn test_event_activity_parsing() {
        assert_eq!("ready".parse::<EventActivity>().unwrap(), EventActivity::Ready);
        assert_eq!("click".parse::<EventActivity>().unwrap(), EventActivity::Click);
        assert_eq!("initialize".parse::<EventActivity>().unwrap(), EventActivity::Initialize);
    }
    
    #[test]
    fn test_event_ref_parsing() {
        assert_eq!("$form".parse::<EventRef>().unwrap(), EventRef::Form);
        assert_eq!("$layout".parse::<EventRef>().unwrap(), EventRef::Layout);
        assert_eq!("$".parse::<EventRef>().unwrap(), EventRef::Current);
    }
    
    #[test]
    fn test_aaab_pattern_with_language_switch() {
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
        
        let mut sp_translations = HashMap::new();
        sp_translations.insert("GV_FirstName_s".to_string(), "Nombre(s)".to_string());
        engine.register_translation_object("mySP", sp_translations);
        
        let script = XfaScript {
            source: r#"
                if(Footer_Line_txtformid.value.match(/^CS/)){
                    this.rawValue = "Vorname(n)";
                }
                else{
                    switch(Footer_Line_txtlanguage.value){
                        case "DE":
                            this.rawValue = myDE.GV_FirstName_s;
                            break;
                        case "SP":
                            this.rawValue = mySP.GV_FirstName_s;
                            break;
                        case "EN":
                        default:
                            this.rawValue = myEN.GV_FirstName_s;
                    }
                }
            "#.to_string(),
            content_type: ScriptContentType::JavaScript,
            activity: EventActivity::Ready,
            event_ref: EventRef::Form,
            name: Some("event__form_ready".to_string()),
            run_at: RunAt::Client,
        };
        
        let result = engine.execute_script(&script);
        assert!(result.is_ok(), "Script execution failed: {:?}", result);
        
        if let Ok(Some(value)) = result {
            assert_eq!(value, "Vorname(n)");
        } else {
            panic!("Expected Some value, got: {:?}", result);
        }
    }
    
    #[test]
    fn test_aaab_pattern_english() {
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
            "#.to_string(),
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
}