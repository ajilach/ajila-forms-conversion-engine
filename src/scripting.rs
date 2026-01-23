//! XFA Scripting Module
//!
//! This module implements JavaScript script execution for XFA forms according to the XFA 3.3 specification.
//!
//! ## Key XFA Scripting Concepts (from XFA Spec Chapter 11):
//!
//! ### Script Languages
//! - XFA supports JavaScript (contentType="application/x-javascript") and FormCalc (contentType="application/x-formcalc")
//! - This implementation focuses on JavaScript using the Boa engine
//!
//! ### Script Context (this reference)
//! - In JavaScript, `this` refers to the current container (field, subform, or exclusion group)
//! - Per spec: "the symbol this is used" in JavaScript, while FormCalc uses "$"
//! - Naked references (e.g., `rawValue` instead of `this.rawValue`) are resolved using XFA-SOM rules
//!
//! ### XFA Scripting Object Model (SOM) Shortcuts
//! - `$data` → xfa.datasets.data (Data DOM)
//! - `$form` → xfa.form (Form DOM - joined template and data after merge)
//! - `$template` → xfa.template (Template DOM)
//! - `$host` → xfa.host (Host application methods/properties)
//! - `$event` → xfa.event (Current event properties)
//! - `$record` → Current data record
//!
//! ### Events (from XFA Spec Chapter 10, "Events")
//! Events are changes of state that trigger script execution:
//!
//! #### DOM Events (ref="$form" or ref="$layout"):
//! - `ready` - Fires after DOM finishes loading (form ready = after merge + calculations)
//!
//! #### Field Events:
//! - `initialize` - Fires after data binding is complete
//! - `enter`/`exit` - Focus events
//! - `change` - Content changed by user
//! - `click` - Mouse click
//! - `calculate` - Field calculation script
//! - `validate` - Field validation script
//!
//! ### Variable References
//! Scripts can reference other fields/variables using XFA-SOM expressions:
//! - Simple: `Footer_Line_txtlanguage.value`
//! - Qualified: `$form.Page.Header.txtlanguage.value`
//! - Array notation: `Detail[*].Total_Price`

use boa_engine::{
    Context, JsArgs, JsValue, NativeFunction,
    js_string, Source, JsString,
    object::{JsObject, ObjectInitializer},
    property::{Attribute, PropertyKey},
};
use boa_gc::{Finalize, Trace, GcRefCell};
use std::collections::HashMap;
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
    /// DOM ready event - fires after DOM finishes loading
    Ready,
    /// Field initialize event - fires after data binding
    Initialize,
    /// Field/subform enter event - gains keyboard focus
    Enter,
    /// Field/subform exit event - loses keyboard focus
    Exit,
    /// Field change event - content changed by user
    Change,
    /// Click event
    Click,
    /// Calculate event - field calculation
    Calculate,
    /// Validate event - field validation
    Validate,
    /// Pre-submit event
    PreSubmit,
    /// Post-submit event
    PostSubmit,
    /// Document ready event
    DocReady,
    /// Index change event (for dynamic arrays)
    IndexChange,
    /// Unknown/other activity
    Other(String),
}

impl EventActivity {
    pub fn from_str(s: &str) -> Self {
        match s {
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
        }
    }
}

/// XFA Event reference target
/// Per XFA spec: The `ref` attribute specifies what DOM the event applies to
#[derive(Debug, Clone, PartialEq)]
pub enum EventRef {
    /// $form - Form DOM (merged template + data)
    Form,
    /// $layout - Layout DOM
    Layout,
    /// $data - Data DOM
    Data,
    /// $ or self - Current container
    Current,
    /// Named reference to another field/subform
    Named(String),
}

impl EventRef {
    pub fn from_str(s: &str) -> Self {
        match s {
            "$form" | "xfa.form" => EventRef::Form,
            "$layout" | "xfa.layout" => EventRef::Layout,
            "$data" | "xfa.data" => EventRef::Data,
            "$" => EventRef::Current,
            _ => EventRef::Named(s.to_string()),
        }
    }
}

/// Represents a script attached to an event
#[derive(Debug, Clone)]
pub struct XfaScript {
    /// Script source code
    pub source: String,
    /// Script language (JavaScript or FormCalc)
    pub content_type: ScriptContentType,
    /// Event activity type (ready, click, etc.)
    pub activity: EventActivity,
    /// Event reference (which DOM/object the event applies to)
    pub event_ref: EventRef,
    /// Event name attribute (optional)
    pub name: Option<String>,
    /// Where to execute (client, server, both)
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

impl RunAt {
    pub fn from_str(s: &str) -> Self {
        match s {
            "server" => RunAt::Server,
            "both" => RunAt::Both,
            _ => RunAt::Client,
        }
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
    pub fn to_js_value(&self, context: &mut Context) -> JsValue {
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

/// Shared form data state that scripts can read/write
/// This represents the Form DOM values that scripts can access
#[derive(Debug, Clone)]
pub struct FormState {
    /// Field values indexed by SOM path (e.g., "Page.Header.txtlanguage")
    pub values: HashMap<String, XfaValue>,
    /// Scripts can declare global variables that persist across script executions
    pub global_variables: HashMap<String, XfaValue>,
}

impl FormState {
    pub fn new() -> Self {
        FormState {
            values: HashMap::new(),
            global_variables: HashMap::new(),
        }
    }
    
    /// Get a value by SOM path, supporting both full paths and simple names
    pub fn get_value(&self, path: &str) -> Option<&XfaValue> {
        // Try exact match first
        if let Some(v) = self.values.get(path) {
            return Some(v);
        }
        
        // Try matching just the field name (last component)
        let field_name = path.rsplit('.').next().unwrap_or(path);
        for (key, value) in &self.values {
            if key.ends_with(&format!(".{}", field_name)) || key == field_name {
                return Some(value);
            }
        }
        
        // Try global variables
        self.global_variables.get(path)
    }
    
    /// Set a value by SOM path
    pub fn set_value(&mut self, path: String, value: XfaValue) {
        self.values.insert(path, value);
    }
}

/// Thread-safe wrapper for FormState
pub type SharedFormState = Arc<RwLock<FormState>>;

/// XFA Field object exposed to JavaScript
/// Per XFA spec, fields have properties like `rawValue`, `value`, `name`, etc.
#[derive(Debug, Clone, Trace, Finalize)]
pub struct XfaFieldObject {
    /// The field's name
    #[unsafe_ignore_trace]
    pub name: String,
    /// The field's SOM path
    #[unsafe_ignore_trace]
    pub path: String,
    /// Current raw value
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

/// XFA Scripting Engine
/// Manages JavaScript execution for XFA forms
pub struct XfaScriptEngine {
    /// Boa JavaScript context
    context: Context,
    /// Shared form state
    form_state: SharedFormState,
    /// Currently executing field's path (for `this` reference)
    current_field_path: Option<String>,
}

impl XfaScriptEngine {
    /// Create a new XFA scripting engine
    pub fn new() -> Self {
        let mut context = Context::default();
        let form_state = Arc::new(RwLock::new(FormState::new()));
        
        let mut engine = XfaScriptEngine {
            context,
            form_state,
            current_field_path: None,
        };
        
        // Set up the XFA scripting environment
        engine.setup_environment();
        
        engine
    }
    
    /// Create engine with pre-existing form state
    pub fn with_state(form_state: SharedFormState) -> Self {
        let mut context = Context::default();
        
        let mut engine = XfaScriptEngine {
            context,
            form_state,
            current_field_path: None,
        };
        
        engine.setup_environment();
        
        engine
    }
    
    /// Set up the XFA scripting environment with global objects and shortcuts
    fn setup_environment(&mut self) {
        // Create the root `xfa` object
        self.setup_xfa_object();
        
        // Set up shortcuts ($form, $data, $host, etc.)
        self.setup_shortcuts();
    }
    
    /// Create the xfa root object per XFA-SOM spec
    fn setup_xfa_object(&mut self) {
        let xfa = ObjectInitializer::new(&mut self.context)
            .build();
        
        // Create xfa.form (Form DOM)
        let form = ObjectInitializer::new(&mut self.context)
            .build();
        
        // Create xfa.datasets and xfa.datasets.data (Data DOM)
        let data = ObjectInitializer::new(&mut self.context)
            .build();
        let datasets = ObjectInitializer::new(&mut self.context)
            .property(js_string!("data"), data.clone(), Attribute::all())
            .build();
        
        // Create xfa.template (Template DOM)
        let template = ObjectInitializer::new(&mut self.context)
            .build();
        
        // Create xfa.layout (Layout DOM)
        let layout = ObjectInitializer::new(&mut self.context)
            .build();
        
        // Create xfa.host (Host pseudo-DOM)
        let host = self.create_host_object();
        
        // Create xfa.event (Event pseudo-DOM)
        let event = ObjectInitializer::new(&mut self.context)
            .property(js_string!("name"), JsValue::from(js_string!("")), Attribute::all())
            .property(js_string!("target"), JsValue::null(), Attribute::all())
            .property(js_string!("cancelAction"), JsValue::from(false), Attribute::all())
            .build();
        
        // Assemble the xfa object
        xfa.set(PropertyKey::from(js_string!("form")), form, false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("datasets")), datasets, false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("data")), data.clone(), false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("template")), template, false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("layout")), layout, false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("host")), host, false, &mut self.context).ok();
        xfa.set(PropertyKey::from(js_string!("event")), event, false, &mut self.context).ok();
        
        // Set xfa as global
        self.context.register_global_property(js_string!("xfa"), xfa, Attribute::all()).ok();
    }
    
    /// Create the xfa.host object with common host methods
    fn create_host_object(&mut self) -> JsObject {
        // Create messageBox function
        let message_box = NativeFunction::from_fn_ptr(|_this, args, context| {
            let message = args.get_or_undefined(0).to_string(context)?;
            // In a real implementation, this would show a dialog
            // For now, we just log it
            eprintln!("[XFA messageBox]: {}", message.to_std_string_escaped());
            Ok(JsValue::undefined())
        });
        
        // Create setFocus function
        let set_focus = NativeFunction::from_fn_ptr(|_this, args, context| {
            // In a real implementation, this would set focus to a field
            Ok(JsValue::undefined())
        });
        
        ObjectInitializer::new(&mut self.context)
            .property(js_string!("name"), JsValue::from(js_string!("Blueprint")), Attribute::READONLY)
            .property(js_string!("version"), JsValue::from(js_string!("1.0")), Attribute::READONLY)
            .function(message_box, js_string!("messageBox"), 1)
            .function(set_focus, js_string!("setFocus"), 1)
            .build()
    }
    
    /// Set up XFA-SOM shortcuts ($form, $data, etc.)
    fn setup_shortcuts(&mut self) {
        // Get the xfa object
        let xfa = self.context.global_object().get(PropertyKey::from(js_string!("xfa")), &mut self.context)
            .unwrap_or(JsValue::undefined());
        
        if let Some(xfa_obj) = xfa.as_object() {
            // $form -> xfa.form
            if let Ok(form) = xfa_obj.get(PropertyKey::from(js_string!("form")), &mut self.context) {
                self.context.register_global_property(js_string!("$form"), form, Attribute::all()).ok();
            }
            
            // $data -> xfa.datasets.data
            if let Ok(datasets) = xfa_obj.get(PropertyKey::from(js_string!("datasets")), &mut self.context) {
                if let Some(ds_obj) = datasets.as_object() {
                    if let Ok(data) = ds_obj.get(PropertyKey::from(js_string!("data")), &mut self.context) {
                        self.context.register_global_property(js_string!("$data"), data, Attribute::all()).ok();
                    }
                }
            }
            
            // $template -> xfa.template
            if let Ok(template) = xfa_obj.get(PropertyKey::from(js_string!("template")), &mut self.context) {
                self.context.register_global_property(js_string!("$template"), template, Attribute::all()).ok();
            }
            
            // $layout -> xfa.layout
            if let Ok(layout) = xfa_obj.get(PropertyKey::from(js_string!("layout")), &mut self.context) {
                self.context.register_global_property(js_string!("$layout"), layout, Attribute::all()).ok();
            }
            
            // $host -> xfa.host
            if let Ok(host) = xfa_obj.get(PropertyKey::from(js_string!("host")), &mut self.context) {
                self.context.register_global_property(js_string!("$host"), host, Attribute::all()).ok();
            }
            
            // $event -> xfa.event
            if let Ok(event) = xfa_obj.get(PropertyKey::from(js_string!("event")), &mut self.context) {
                self.context.register_global_property(js_string!("$event"), event, Attribute::all()).ok();
            }
            
            // $xfa -> xfa (for symmetry)
            self.context.register_global_property(js_string!("$xfa"), xfa, Attribute::all()).ok();
        }
    }
    
    /// Register a field value in the scripting environment
    /// This makes the field accessible via SOM paths like "Footer_Line_txtlanguage.value"
    pub fn register_field(&mut self, path: &str, name: &str, value: &str) {
        // Store in form state
        {
            let mut state = self.form_state.write().unwrap();
            state.set_value(path.to_string(), XfaValue::String(value.to_string()));
        }
        
        // Create JavaScript object for the field
        let field_obj = self.create_field_object(name, path, value);
        
        // Register with the simple name (for naked references)
        self.context.register_global_property(
            JsString::from(name),
            field_obj.clone(),
            Attribute::all()
        ).ok();
        
        // Also register on $form for qualified paths
        let xfa = self.context.global_object().get(PropertyKey::from(js_string!("xfa")), &mut self.context)
            .unwrap_or(JsValue::undefined());
        
        if let Some(xfa_obj) = xfa.as_object() {
            if let Ok(form) = xfa_obj.get(PropertyKey::from(js_string!("form")), &mut self.context) {
                if let Some(form_obj) = form.as_object() {
                    // Build the path on $form
                    self.register_path_on_object(&form_obj, path, field_obj);
                }
            }
        }
    }
    
    /// Create a JavaScript object representing an XFA field
    fn create_field_object(&mut self, name: &str, path: &str, initial_value: &str) -> JsObject {
        let name_js = js_string!(name);
        let path_clone = path.to_string();
        let form_state = Arc::clone(&self.form_state);
        
        // Create the field object with rawValue property
        let field = ObjectInitializer::new(&mut self.context)
            .property(js_string!("name"), JsValue::from(name_js.clone()), Attribute::READONLY)
            .build();
        
        // Add rawValue as a property with getter/setter
        // For simplicity, we use a simple property that scripts can read/write
        field.set(
            PropertyKey::from(js_string!("rawValue")),
            JsValue::from(js_string!(initial_value)),
            false,
            &mut self.context
        ).ok();
        
        // Add value property (alias for rawValue in most cases)
        field.set(
            PropertyKey::from(js_string!("value")),
            JsValue::from(js_string!(initial_value)),
            false,
            &mut self.context
        ).ok();
        
        field
    }
    
    /// Register a path on an object (e.g., register "Page.Header.field" on $form)
    fn register_path_on_object(&mut self, root: &JsObject, path: &str, field_obj: JsObject) {
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = root.clone();
        
        for (i, part) in parts.iter().enumerate() {
            let key = PropertyKey::from(js_string!(*part));
            
            if i == parts.len() - 1 {
                // Last part - set the field object
                current.set(key, field_obj.clone(), false, &mut self.context).ok();
            } else {
                // Intermediate part - get or create intermediate object
                let existing = current.get(key.clone(), &mut self.context).unwrap_or(JsValue::undefined());
                
                if existing.is_undefined() {
                    // Create intermediate object
                    let intermediate = ObjectInitializer::new(&mut self.context).build();
                    current.set(key.clone(), intermediate.clone(), false, &mut self.context).ok();
                    current = intermediate;
                } else if let Some(obj) = existing.as_object() {
                    current = obj.clone();
                } else {
                    // Path conflict - existing value is not an object
                    break;
                }
            }
        }
    }
    
    /// Register a global variable (e.g., myEN, myDE, mySP for translations)
    pub fn register_global_variable(&mut self, name: &str, value: JsObject) {
        self.context.register_global_property(
            JsString::from(name),
            value,
            Attribute::all()
        ).ok();
    }
    
    /// Register translation objects (myEN, myDE, mySP pattern from AAAB)
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
    
    /// Execute a variable initialization script.
    /// According to XFA spec, scripts in <variables> elements are compiled and executed
    /// when the subform is instantiated during data binding.
    /// This makes any global variables or functions defined in the script available.
    pub fn execute_variable_script(&mut self, source: &str) -> Result<(), String> {
        match self.context.eval(Source::from_bytes(source)) {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("Variable script error: {}", e)),
        }
    }
    
    /// Evaluate a JavaScript expression and return its string result
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
    
    /// Set the current field context for `this` reference
    pub fn set_current_field(&mut self, path: &str, name: &str, value: &str) {
        self.current_field_path = Some(path.to_string());
        
        // Create `this` object representing the current field
        let this_obj = self.create_field_object(name, path, value);
        
        // Register as global `this` (in XFA scripts, `this` at global scope refers to current container)
        // Note: In strict JavaScript, `this` is handled by the engine, but for XFA compatibility
        // we also expose it as a property
        self.context.register_global_property(
            js_string!("this"),
            this_obj,
            Attribute::all()
        ).ok();
    }
    
    /// Execute a script and return the result
    pub fn execute_script(&mut self, script: &XfaScript) -> Result<Option<String>, String> {
        match script.content_type {
            ScriptContentType::JavaScript => self.execute_javascript(&script.source),
            ScriptContentType::FormCalc => {
                // FormCalc is not implemented - return error
                Err("FormCalc scripts are not supported".to_string())
            }
        }
    }
    
    /// Execute JavaScript code
    fn execute_javascript(&mut self, source: &str) -> Result<Option<String>, String> {
        // Get the 'this' object that was set up for the current field
        let this_obj = self.context.global_object()
            .get(PropertyKey::from(js_string!("this")), &mut self.context)
            .ok();
        
        // Store the initial rawValue to compare later
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
        
        // Execute the script directly (not wrapped in function to preserve 'this' binding)
        match self.context.eval(Source::from_bytes(source)) {
            Ok(result) => {
                // Check if the script set `this.rawValue`
                if let Ok(this_val) = self.context.global_object().get(PropertyKey::from(js_string!("this")), &mut self.context) {
                    if let Some(this_obj) = this_val.as_object() {
                        if let Ok(raw_value) = this_obj.get(PropertyKey::from(js_string!("rawValue")), &mut self.context) {
                            if !raw_value.is_undefined() && !raw_value.is_null() {
                                let value_str = raw_value.to_string(&mut self.context)
                                    .map(|s| s.to_std_string_escaped())
                                    .unwrap_or_default();
                                
                                // Only return if the value changed
                                let changed = initial_raw_value.as_ref() != Some(&value_str);
                                
                                if changed {
                                    // Update form state
                                    if let Some(ref path) = self.current_field_path {
                                        let mut state = self.form_state.write().unwrap();
                                        state.set_value(path.clone(), XfaValue::String(value_str.clone()));
                                    }
                                    
                                    return Ok(Some(value_str));
                                }
                            }
                        }
                    }
                }
                
                // Return script result if it's a meaningful value
                if result.is_undefined() || result.is_null() {
                    Ok(None)
                } else {
                    Ok(Some(result.to_string(&mut self.context)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default()))
                }
            }
            Err(e) => {
                Err(format!("JavaScript error: {}", e))
            }
        }
    }
    
    /// Get the computed value for a field after script execution
    pub fn get_field_value(&self, path: &str) -> Option<String> {
        let state = self.form_state.read().ok()?;
        state.get_value(path).map(|v| v.as_string())
    }
    
    /// Get the shared form state
    pub fn form_state(&self) -> &SharedFormState {
        &self.form_state
    }
}

/// Parse event elements from XFA node children
pub fn parse_events_from_node(children: &[crate::xfa::XfaNode]) -> Vec<XfaScript> {
    let mut scripts = Vec::new();
    
    for child in children {
        if let crate::xfa::XfaNodeKind::Element { tag_name, .. } = &child.kind {
            if tag_name == "event" {
                if let Some(script) = parse_event_element(child) {
                    scripts.push(script);
                }
            }
        }
    }
    
    scripts
}

/// Parse a single <event> element into an XfaScript
fn parse_event_element(event_node: &crate::xfa::XfaNode) -> Option<XfaScript> {
    let activity = event_node.attributes.get("activity")
        .map(|s| EventActivity::from_str(s))
        .unwrap_or(EventActivity::Other("unknown".to_string()));
    
    let event_ref = event_node.attributes.get("ref")
        .map(|s| EventRef::from_str(s))
        .unwrap_or(EventRef::Current);
    
    let name = event_node.attributes.get("name").cloned();
    
    // Find the <script> child element
    for child in &event_node.children {
        if let crate::xfa::XfaNodeKind::Element { tag_name, text_content } = &child.kind {
            if tag_name == "script" {
                let content_type = child.attributes.get("contentType")
                    .and_then(|s| ScriptContentType::from_content_type(s))
                    .unwrap_or(ScriptContentType::FormCalc); // Default is FormCalc per spec
                
                let run_at = child.attributes.get("runAt")
                    .map(|s| RunAt::from_str(s))
                    .unwrap_or_default();
                
                // Get script source from text content
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
    }
    
    None
}

/// Parse <variables> element which may contain script objects (like myEN, myDE, mySP)
/// Per XFA spec: Variables are defined in the template and can be referenced in scripts
pub fn parse_variables_from_node(children: &[crate::xfa::XfaNode]) -> HashMap<String, HashMap<String, String>> {
    let mut variables = HashMap::new();
    
    for child in children {
        if let crate::xfa::XfaNodeKind::Element { tag_name, .. } = &child.kind {
            if tag_name == "variables" {
                // Look for script variables (objects containing translations, etc.)
                for var_child in &child.children {
                    if let crate::xfa::XfaNodeKind::Element { tag_name: var_tag, .. } = &var_child.kind {
                        if var_tag == "script" {
                            // This is a variable definition script
                            // Parse the script to extract variable definitions
                            if let Some(name) = var_child.attributes.get("name") {
                                // For now, we'll handle this specially
                                // These scripts typically define objects like:
                                // var myEN = { GV_FirstName_s: "First name(s)", ... }
                                variables.insert(name.clone(), HashMap::new());
                            }
                        }
                    }
                }
            }
        }
    }
    
    variables
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_script_engine_basic() {
        let mut engine = XfaScriptEngine::new();
        
        // Register a field
        engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", "DE");
        
        // Execute a simple script
        let script = XfaScript {
            source: r#"Footer_Line_txtlanguage.value"#.to_string(),
            content_type: ScriptContentType::JavaScript,
            activity: EventActivity::Ready,
            event_ref: EventRef::Form,
            name: None,
            run_at: RunAt::Client,
        };
        
        let result = engine.execute_script(&script);
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_script_with_this_reference() {
        let mut engine = XfaScriptEngine::new();
        
        // Set up the current field context
        engine.set_current_field("ffFirstName_s", "ffFirstName_s", "");
        
        // Register translation objects
        let mut translations = HashMap::new();
        translations.insert("GV_FirstName_s".to_string(), "Vorname(n)".to_string());
        engine.register_translation_object("myDE", translations);
        
        // Execute a script that sets this.rawValue
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
    fn test_event_activity_parsing() {
        assert_eq!(EventActivity::from_str("ready"), EventActivity::Ready);
        assert_eq!(EventActivity::from_str("click"), EventActivity::Click);
        assert_eq!(EventActivity::from_str("initialize"), EventActivity::Initialize);
    }
    
    #[test]
    fn test_event_ref_parsing() {
        assert_eq!(EventRef::from_str("$form"), EventRef::Form);
        assert_eq!(EventRef::from_str("$layout"), EventRef::Layout);
        assert_eq!(EventRef::from_str("$"), EventRef::Current);
    }
    
    #[test]
    fn test_aaab_pattern_with_language_switch() {
        let mut engine = XfaScriptEngine::new();
        
        // Set up the current field context (the label field being populated)
        engine.set_current_field("ffFirstName_s", "ffFirstName_s", "");
        
        // Register the language control field (Footer_Line_txtlanguage)
        engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", "DE");
        
        // Register the form ID field (Footer_Line_txtformid)
        engine.register_field("Footer_Line_txtformid", "Footer_Line_txtformid", "AAAB");
        
        // Register translation objects (like in AAAB)
        let mut de_translations = HashMap::new();
        de_translations.insert("GV_FirstName_s".to_string(), "Vorname(n)".to_string());
        engine.register_translation_object("myDE", de_translations);
        
        let mut en_translations = HashMap::new();
        en_translations.insert("GV_FirstName_s".to_string(), "First name(s)".to_string());
        engine.register_translation_object("myEN", en_translations);
        
        let mut sp_translations = HashMap::new();
        sp_translations.insert("GV_FirstName_s".to_string(), "Nombre(s)".to_string());
        engine.register_translation_object("mySP", sp_translations);
        
        // Execute the AAAB-style script with language switch
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
        
        // Since language is "DE", we should get German translation
        if let Ok(Some(value)) = result {
            assert_eq!(value, "Vorname(n)");
        } else {
            panic!("Expected Some value, got: {:?}", result);
        }
    }
    
    #[test]
    fn test_aaab_pattern_english() {
        let mut engine = XfaScriptEngine::new();
        
        // Set up the current field context
        engine.set_current_field("ffFirstName_s", "ffFirstName_s", "");
        
        // Register with English language
        engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", "EN");
        engine.register_field("Footer_Line_txtformid", "Footer_Line_txtformid", "AAAB");
        
        // Register translation objects
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
        
        // Since language is "EN", we should get English translation
        if let Ok(Some(value)) = result {
            assert_eq!(value, "First name(s)");
        }
    }
}