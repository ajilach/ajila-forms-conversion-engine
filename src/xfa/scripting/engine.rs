//! XFA Script Engine
//!
//! This module implements the core JavaScript execution engine for XFA forms,
//! providing XFA 3.3 spec compliance for scripting.
//!
//! ## XFA 3.3 Spec Implementation:
//! - Chapter 3: Scripting Object Model (SOM)
//! - Chapter 10: Automation Objects
//! - Chapter 11: Scripting

use super::dependency::DependencyTracker;
use super::events::{ScriptContentType, XfaScript};
use super::js_helpers;
use super::som::{SomPath, SomResolver};
use super::state::{FormState, Presence, SharedFormState, XfaValue};

use boa_engine::{
    Context, JsArgs, JsString, JsValue, NativeFunction, Source, js_string,
    object::{JsObject, ObjectInitializer},
    property::{Attribute, PropertyKey},
};
use boa_gc::{Finalize, GcRefCell, Trace};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// XFA Field object exposed to JavaScript
#[derive(Debug, Clone, Trace, Finalize)]
pub struct XfaFieldObject {
    #[unsafe_ignore_trace]
    pub name: String,
    #[unsafe_ignore_trace]
    pub path: SomPath,
    #[unsafe_ignore_trace]
    pub raw_value: GcRefCell<String>,
}

impl XfaFieldObject {
    pub fn new(name: String, path: SomPath, initial_value: String) -> Self {
        XfaFieldObject {
            name,
            path,
            raw_value: GcRefCell::new(initial_value),
        }
    }
}

/// XFA Scripting Engine with XFA 3.3 spec compliance
pub struct XfaScriptEngine {
    context: Context,
    form_state: SharedFormState,
    current_field_path: Option<SomPath>,
    /// Current script execution context path
    current_context_path: Option<SomPath>,
    /// SOM resolver for resolveNode()/resolveNodes()
    som_resolver: SomResolver,
    /// Dependency tracker for cascading calculations
    dependencies: DependencyTracker,
    /// Registered field JS objects for resolveNode results, keyed by FULL SOM path
    field_objects: HashMap<SomPath, JsObject>,
    /// Maps field NAME to list of FULL SOM paths that have that name
    field_objects_by_name: HashMap<String, Vec<SomPath>>,
    /// Maps child field names to their unique IDs in the current context
    child_name_to_id: HashMap<String, String>,

    /// Tracks which presence values have been explicitly changed by scripts
    presence_changes: HashMap<SomPath, String>,
    /// Tracks the INITIAL presence value from the XFA tree for each field object
    initial_presence: HashMap<SomPath, String>,
}

impl XfaScriptEngine {
    pub fn new() -> Self {
        let context = Context::default();
        let form_state = Arc::new(RwLock::new(FormState::new()));

        let mut engine = XfaScriptEngine {
            context,
            form_state,
            current_field_path: None,
            current_context_path: None,
            som_resolver: SomResolver::new(),
            dependencies: DependencyTracker::new(),
            field_objects: HashMap::new(),
            field_objects_by_name: HashMap::new(),
            child_name_to_id: HashMap::new(),
            presence_changes: HashMap::new(),
            initial_presence: HashMap::new(),
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
            current_context_path: None,
            som_resolver: SomResolver::new(),
            dependencies: DependencyTracker::new(),
            field_objects: HashMap::new(),
            field_objects_by_name: HashMap::new(),
            child_name_to_id: HashMap::new(),
            presence_changes: HashMap::new(),
            initial_presence: HashMap::new(),
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

    /// Get the rawValue of a specific field by its SOM path.
    /// Falls back to looking up by the short field name if the exact path isn't found.
    pub fn get_field_value(&mut self, path: &SomPath) -> Option<String> {
        // Try exact path first
        if let Some(obj) = self.field_objects.get(path) {
            let obj = obj.clone();
            if let Ok(raw_value) =
                obj.get(PropertyKey::from(js_string!("rawValue")), &mut self.context)
            {
                if !raw_value.is_undefined() && !raw_value.is_null() {
                    if let Ok(value_str) = raw_value.to_string(&mut self.context) {
                        let value = value_str.to_std_string_escaped();
                        if !value.is_empty() {
                            return Some(value);
                        }
                    }
                }
            }
        }
        // Fallback: try by field name
        let name = path.name().to_string();
        if let Some(paths) = self.field_objects_by_name.get(&name) {
            if let Some(first_path) = paths.first().cloned() {
                if let Some(obj) = self.field_objects.get(&first_path) {
                    let obj = obj.clone();
                    if let Ok(raw_value) =
                        obj.get(PropertyKey::from(js_string!("rawValue")), &mut self.context)
                    {
                        if !raw_value.is_undefined() && !raw_value.is_null() {
                            if let Ok(value_str) = raw_value.to_string(&mut self.context) {
                                let value = value_str.to_std_string_escaped();
                                if !value.is_empty() {
                                    return Some(value);
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Get all field values as a HashMap keyed by both full SOM path and short name.
    /// This produces the same dual-keyed map that `computed_values` previously maintained,
    /// suitable for passing to `Flattened::from_xfa`.
    pub fn get_all_field_values_for_flattening(&mut self) -> HashMap<SomPath, String> {
        let mut map = HashMap::new();
        let paths: Vec<(SomPath, JsObject)> = self
            .field_objects
            .iter()
            .map(|(p, o)| (p.clone(), o.clone()))
            .collect();
        for (path, obj) in paths {
            if let Ok(raw_value) =
                obj.get(PropertyKey::from(js_string!("rawValue")), &mut self.context)
            {
                if !raw_value.is_undefined() && !raw_value.is_null() {
                    if let Ok(value_str) = raw_value.to_string(&mut self.context) {
                        let value = value_str.to_std_string_escaped();
                        // Store under full SOM path (empty strings are valid per XFA spec,
                        // e.g. cleared dropdowns, deselected exclGroups)
                        map.insert(path.clone(), value.clone());
                        // Also store under short name for backward compat lookups
                        map.insert(SomPath::new(path.name()), value);
                    }
                }
            }
        }
        map
    }

    /// Create a global field registry that resolveNode can access
    fn setup_field_registry(&mut self) {
        // Create _xfa_fields_ global object that maps field names to their JS objects
        let registry = ObjectInitializer::new(&mut self.context).build();
        self.context
            .register_global_property(js_string!("_xfa_fields_"), registry, Attribute::all())
            .ok();

        // Create _xfa_fields_by_path_ that maps FULL SOM paths to JS objects
        let registry_by_path = ObjectInitializer::new(&mut self.context).build();
        self.context
            .register_global_property(
                js_string!("_xfa_fields_by_path_"),
                registry_by_path,
                Attribute::all(),
            )
            .ok();

        // Create _xfa_paths_by_name_ that maps field names to arrays of full paths
        let paths_by_name = ObjectInitializer::new(&mut self.context).build();
        self.context
            .register_global_property(
                js_string!("_xfa_paths_by_name_"),
                paths_by_name,
                Attribute::all(),
            )
            .ok();

        // Store current context path for relative resolution
        self.context
            .register_global_property(
                js_string!("_xfa_current_context_"),
                JsValue::from(js_string!("")),
                Attribute::all(),
            )
            .ok();
    }

    /// Set up JavaScript helpers for SOM resolution
    fn setup_som_fallback(&mut self) {
        let helpers_js = js_helpers::get_all_helpers();
        let _ = self.execute_variable_script(&helpers_js);
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

        // Create layout object with relayout() stub
        let relayout_fn =
            NativeFunction::from_fn_ptr(|_this, _args, _context| Ok(JsValue::undefined()));
        let layout = ObjectInitializer::new(&mut self.context)
            .function(relayout_fn, js_string!("relayout"), 0)
            .build();

        let host = self.create_host_object();

        let event = ObjectInitializer::new(&mut self.context)
            .property(
                js_string!("name"),
                JsValue::from(js_string!("")),
                Attribute::all(),
            )
            .property(js_string!("target"), JsValue::null(), Attribute::all())
            .property(
                js_string!("cancelAction"),
                JsValue::from(false),
                Attribute::all(),
            )
            .build();

        xfa.set(
            PropertyKey::from(js_string!("form")),
            form,
            false,
            &mut self.context,
        )
        .ok();
        xfa.set(
            PropertyKey::from(js_string!("datasets")),
            datasets,
            false,
            &mut self.context,
        )
        .ok();
        xfa.set(
            PropertyKey::from(js_string!("data")),
            data.clone(),
            false,
            &mut self.context,
        )
        .ok();
        xfa.set(
            PropertyKey::from(js_string!("template")),
            template,
            false,
            &mut self.context,
        )
        .ok();
        xfa.set(
            PropertyKey::from(js_string!("layout")),
            layout,
            false,
            &mut self.context,
        )
        .ok();
        xfa.set(
            PropertyKey::from(js_string!("host")),
            host,
            false,
            &mut self.context,
        )
        .ok();
        xfa.set(
            PropertyKey::from(js_string!("event")),
            event,
            false,
            &mut self.context,
        )
        .ok();

        // Add resolveNode as a global function (XFA 3.3 spec page 106-107)
        let resolve_node_fn = NativeFunction::from_fn_ptr(Self::resolve_node_impl);
        xfa.set(
            PropertyKey::from(js_string!("resolveNode")),
            resolve_node_fn.to_js_function(self.context.realm()),
            false,
            &mut self.context,
        )
        .ok();

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
            &mut self.context,
        )
        .ok();

        self.context
            .register_global_property(js_string!("xfa"), xfa, Attribute::all())
            .ok();
    }

    /// Implementation of resolveNode for the JavaScript environment
    fn resolve_node_impl(
        _this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> boa_engine::JsResult<JsValue> {
        let expr = args.get_or_undefined(0).to_string(context)?;
        let expr_str = expr.to_std_string_escaped();

        // If the expression is a full path (contains dots), try direct lookup first
        if expr_str.contains('.') {
            if let Ok(registry) = context.global_object().get(
                PropertyKey::from(js_string!("_xfa_fields_by_path_")),
                context,
            ) {
                if let Some(registry_obj) = registry.as_object() {
                    if let Ok(field_obj) = registry_obj.get(
                        PropertyKey::from(JsString::from(expr_str.as_str())),
                        context,
                    ) {
                        if !field_obj.is_undefined() && !field_obj.is_null() {
                            return Ok(field_obj);
                        }
                    }
                }
            }
        }

        // Extract the field name from the expression (last component)
        let field_name = expr_str.rsplit('.').next().unwrap_or(&expr_str);

        // Get the current execution context path
        let current_context = context
            .global_object()
            .get(
                PropertyKey::from(js_string!("_xfa_current_context_")),
                context,
            )
            .ok()
            .and_then(|v| v.to_string(context).ok())
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();

        // Look up all paths that have this field name and find best match
        if let Ok(paths_by_name) = context.global_object().get(
            PropertyKey::from(js_string!("_xfa_paths_by_name_")),
            context,
        ) {
            if let Some(paths_obj) = paths_by_name.as_object() {
                if let Ok(paths_array) =
                    paths_obj.get(PropertyKey::from(JsString::from(field_name)), context)
                {
                    if !paths_array.is_undefined()
                        && !paths_array.is_null()
                        && paths_array.as_object().is_some()
                    {
                        let paths_arr_obj = paths_array.as_object().unwrap();

                        // Get the array length
                        let length = paths_arr_obj
                            .get(PropertyKey::from(js_string!("length")), context)
                            .ok()
                            .and_then(|v| v.to_number(context).ok())
                            .unwrap_or(0.0) as usize;

                        // Collect all paths
                        let mut all_paths: Vec<String> = Vec::with_capacity(length);
                        for i in 0..length {
                            if let Ok(path_val) = paths_arr_obj.get(PropertyKey::from(i), context) {
                                if let Ok(path_str) = path_val.to_string(context) {
                                    all_paths.push(path_str.to_std_string_escaped());
                                }
                            }
                        }

                        // Find the best matching path based on context
                        let best_path =
                            Self::find_best_path_for_context(&all_paths, &current_context);

                        // Look up the field object by the best path
                        if !best_path.is_empty() {
                            if let Ok(registry) = context.global_object().get(
                                PropertyKey::from(js_string!("_xfa_fields_by_path_")),
                                context,
                            ) {
                                if let Some(registry_obj) = registry.as_object() {
                                    if let Ok(field_obj) = registry_obj.get(
                                        PropertyKey::from(JsString::from(best_path.as_str())),
                                        context,
                                    ) {
                                        if !field_obj.is_undefined() && !field_obj.is_null() {
                                            return Ok(field_obj);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Fallback: look up in the legacy _xfa_fields_ registry
        if let Ok(registry) = context
            .global_object()
            .get(PropertyKey::from(js_string!("_xfa_fields_")), context)
        {
            if let Some(registry_obj) = registry.as_object() {
                if let Ok(field_obj) =
                    registry_obj.get(PropertyKey::from(JsString::from(field_name)), context)
                {
                    if !field_obj.is_undefined() && !field_obj.is_null() {
                        return Ok(field_obj);
                    }
                }
            }
        }

        // Also try looking up as a global (for backward compatibility)
        if let Ok(global_field) = context
            .global_object()
            .get(PropertyKey::from(JsString::from(field_name)), context)
        {
            if !global_field.is_undefined() && !global_field.is_null() {
                if let Some(obj) = global_field.as_object() {
                    if let Ok(raw) = obj.get(PropertyKey::from(js_string!("rawValue")), context) {
                        if !raw.is_undefined() {
                            return Ok(global_field);
                        }
                    }
                }
            }
        }

        // Return null if not found
        Ok(JsValue::null())
    }

    /// Find the best matching path based on context
    fn find_best_path_for_context(all_paths: &[String], current_context: &str) -> String {
        if all_paths.is_empty() {
            return String::new();
        }

        if current_context.is_empty() || all_paths.len() == 1 {
            return all_paths.first().cloned().unwrap_or_default();
        }

        // Try to find a path that's a child of the current context
        if let Some(path) = all_paths
            .iter()
            .find(|p| p.starts_with(&format!("{}.", current_context)))
        {
            return path.clone();
        }

        // Try to find one that shares a common ancestor with context
        let context_parts: Vec<&str> = current_context.split('.').collect();
        all_paths
            .iter()
            .filter(|p| {
                let path_parts: Vec<&str> = p.split('.').collect();
                context_parts
                    .iter()
                    .zip(path_parts.iter())
                    .take_while(|(a, b)| a == b)
                    .count()
                    > 0
            })
            .max_by_key(|p| {
                let path_parts: Vec<&str> = p.split('.').collect();
                context_parts
                    .iter()
                    .zip(path_parts.iter())
                    .take_while(|(a, b)| a == b)
                    .count()
            })
            .cloned()
            .unwrap_or_else(|| all_paths.first().cloned().unwrap_or_default())
    }

    fn create_host_object(&mut self) -> JsObject {
        let message_box = NativeFunction::from_fn_ptr(|_this, args, context| {
            let message = args.get_or_undefined(0).to_string(context)?;
            eprintln!("[XFA messageBox]: {}", message.to_std_string_escaped());
            Ok(JsValue::undefined())
        });

        let set_focus =
            NativeFunction::from_fn_ptr(|_this, _args, _context| Ok(JsValue::undefined()));

        ObjectInitializer::new(&mut self.context)
            .property(
                js_string!("name"),
                JsValue::from(js_string!("Blueprint")),
                Attribute::READONLY,
            )
            .property(
                js_string!("version"),
                JsValue::from(js_string!("1.0")),
                Attribute::READONLY,
            )
            .function(message_box, js_string!("messageBox"), 1)
            .function(set_focus, js_string!("setFocus"), 1)
            .build()
    }

    fn setup_shortcuts(&mut self) {
        let xfa = self
            .context
            .global_object()
            .get(PropertyKey::from(js_string!("xfa")), &mut self.context)
            .unwrap_or(JsValue::undefined());

        if let Some(xfa_obj) = xfa.as_object() {
            if let Ok(form) = xfa_obj.get(PropertyKey::from(js_string!("form")), &mut self.context)
            {
                self.context
                    .register_global_property(js_string!("$form"), form, Attribute::all())
                    .ok();
            }
            if let Ok(datasets) =
                xfa_obj.get(PropertyKey::from(js_string!("datasets")), &mut self.context)
            {
                if let Some(ds_obj) = datasets.as_object() {
                    if let Ok(data) =
                        ds_obj.get(PropertyKey::from(js_string!("data")), &mut self.context)
                    {
                        self.context
                            .register_global_property(js_string!("$data"), data, Attribute::all())
                            .ok();
                    }
                }
            }
            if let Ok(template) =
                xfa_obj.get(PropertyKey::from(js_string!("template")), &mut self.context)
            {
                self.context
                    .register_global_property(js_string!("$template"), template, Attribute::all())
                    .ok();
            }
            if let Ok(layout) =
                xfa_obj.get(PropertyKey::from(js_string!("layout")), &mut self.context)
            {
                self.context
                    .register_global_property(js_string!("$layout"), layout, Attribute::all())
                    .ok();
            }
            if let Ok(host) = xfa_obj.get(PropertyKey::from(js_string!("host")), &mut self.context)
            {
                self.context
                    .register_global_property(js_string!("$host"), host, Attribute::all())
                    .ok();
            }
            if let Ok(event) =
                xfa_obj.get(PropertyKey::from(js_string!("event")), &mut self.context)
            {
                self.context
                    .register_global_property(js_string!("$event"), event, Attribute::all())
                    .ok();
            }
            self.context
                .register_global_property(js_string!("$xfa"), xfa, Attribute::all())
                .ok();
        }
    }

    /// Register a field with SOM resolver
    pub fn register_field(&mut self, path: &str, name: &str, value: &str) {
        self.register_field_with_presence(path, name, value, "visible");
    }

    /// Register a field with SOM resolver and explicit initial presence
    pub fn register_field_with_presence(
        &mut self,
        path: &str,
        name: &str,
        value: &str,
        initial_presence: &str,
    ) {
        let som_path = SomPath::new(path);
        let parent_path = som_path.parent();

        // Register in SOM resolver
        self.som_resolver
            .register_node(&som_path, name, "field", parent_path.as_ref());

        // Store initial presence for change detection
        self.initial_presence
            .insert(som_path.clone(), initial_presence.to_string());

        // Store in form state
        {
            let mut state = self.form_state.write().unwrap();
            state.set_value(som_path.clone(), XfaValue::String(value.to_string()));
        }

        // Create JavaScript object with the actual initial presence
        let field_obj = self.create_field_object_with_presence(name, path, value, initial_presence);
        self.field_objects
            .insert(som_path.clone(), field_obj.clone());

        // Track name -> paths mapping for context-aware resolution
        self.field_objects_by_name
            .entry(name.to_string())
            .or_default()
            .push(som_path.clone());

        // Register globally for naked references (legacy)
        self.context
            .register_global_property(JsString::from(name), field_obj.clone(), Attribute::all())
            .ok();

        // Register in _xfa_fields_ registry for resolveNode() lookups (legacy)
        if let Ok(registry) = self.context.global_object().get(
            PropertyKey::from(js_string!("_xfa_fields_")),
            &mut self.context,
        ) {
            if let Some(registry_obj) = registry.as_object() {
                registry_obj
                    .set(
                        PropertyKey::from(JsString::from(name)),
                        field_obj.clone(),
                        false,
                        &mut self.context,
                    )
                    .ok();
            }
        }

        // Register in _xfa_fields_by_path_ for full-path lookups
        if let Ok(registry) = self.context.global_object().get(
            PropertyKey::from(js_string!("_xfa_fields_by_path_")),
            &mut self.context,
        ) {
            if let Some(registry_obj) = registry.as_object() {
                registry_obj
                    .set(
                        PropertyKey::from(JsString::from(path)),
                        field_obj.clone(),
                        false,
                        &mut self.context,
                    )
                    .ok();
            }
        }

        // Register in _xfa_paths_by_name_ to map name -> array of full paths
        if let Ok(paths_by_name) = self.context.global_object().get(
            PropertyKey::from(js_string!("_xfa_paths_by_name_")),
            &mut self.context,
        ) {
            if let Some(paths_obj) = paths_by_name.as_object() {
                // Get or create the array for this name
                let paths_array = if let Ok(existing) =
                    paths_obj.get(PropertyKey::from(JsString::from(name)), &mut self.context)
                {
                    if !existing.is_undefined() && !existing.is_null() {
                        if let Some(arr) = existing.as_object() {
                            arr.clone()
                        } else {
                            self.create_new_paths_array(paths_obj, name)
                        }
                    } else {
                        self.create_new_paths_array(paths_obj, name)
                    }
                } else {
                    self.create_new_paths_array(paths_obj, name)
                };

                // Add this path to the array
                let length = paths_array
                    .get(PropertyKey::from(js_string!("length")), &mut self.context)
                    .ok()
                    .and_then(|v| v.to_number(&mut self.context).ok())
                    .unwrap_or(0.0) as u32;

                paths_array
                    .set(
                        PropertyKey::from(length),
                        JsValue::from(js_string!(path)),
                        false,
                        &mut self.context,
                    )
                    .ok();
            }
        }

        // Register on $form
        let xfa = self
            .context
            .global_object()
            .get(PropertyKey::from(js_string!("xfa")), &mut self.context)
            .unwrap_or(JsValue::undefined());

        if let Some(xfa_obj) = xfa.as_object() {
            if let Ok(form) = xfa_obj.get(PropertyKey::from(js_string!("form")), &mut self.context)
            {
                if let Some(form_obj) = form.as_object() {
                    self.register_path_on_object(&form_obj, path, field_obj.clone());

                    // Also register without root subform prefix
                    if let Some(dot_pos) = path.find('.') {
                        let stripped_path = &path[dot_pos + 1..];
                        self.register_path_on_object(&form_obj, stripped_path, field_obj.clone());
                    }
                }
            }
        }

        // Register first component as global
        if path.contains('.') {
            let global_obj = self.context.global_object();

            if let Some(dot_pos) = path.find('.') {
                let stripped_path = &path[dot_pos + 1..];
                if stripped_path.contains('.') {
                    self.register_path_on_object(&global_obj, stripped_path, field_obj);
                }
            }
        }
    }

    fn create_new_paths_array(&mut self, paths_obj: &JsObject, name: &str) -> JsObject {
        let new_arr = boa_engine::object::builtins::JsArray::new(&mut self.context);
        let new_arr_value: JsValue = new_arr.clone().into();
        paths_obj
            .set(
                PropertyKey::from(JsString::from(name)),
                new_arr_value,
                false,
                &mut self.context,
            )
            .ok();
        new_arr.into()
    }

    /// Update the rawValue of an existing field object in the engine.
    pub fn update_field_value(&mut self, path: &str, value: &str) {
        let som_path = SomPath::new(path);

        // Update form_state by path only
        {
            let mut state = self.form_state.write().unwrap();
            state.set_value(som_path.clone(), XfaValue::String(value.to_string()));
        }

        // Update the field object's rawValue property by PATH ONLY
        if let Some(field_obj) = self.field_objects.get(&som_path) {
            field_obj
                .set(
                    PropertyKey::from(js_string!("rawValue")),
                    JsValue::from(js_string!(value)),
                    false,
                    &mut self.context,
                )
                .ok();
        }
    }

    fn create_field_object(&mut self, name: &str, path: &str, initial_value: &str) -> JsObject {
        self.create_field_object_with_presence(name, path, initial_value, "visible")
    }

    fn create_field_object_with_presence(
        &mut self,
        name: &str,
        path: &str,
        initial_value: &str,
        initial_presence: &str,
    ) -> JsObject {
        let name_js = js_string!(name);
        let path_js = js_string!(path);

        let field = ObjectInitializer::new(&mut self.context)
            .property(
                js_string!("name"),
                JsValue::from(name_js.clone()),
                Attribute::READONLY,
            )
            .property(
                js_string!("somExpression"),
                JsValue::from(path_js),
                Attribute::READONLY,
            )
            .build();

        // Set _rawValue as the internal backing property
        field
            .set(
                PropertyKey::from(js_string!("_rawValue")),
                JsValue::from(js_string!(initial_value)),
                false,
                &mut self.context,
            )
            .ok();

        // Define rawValue with getter/setter for automatic exclGroup propagation.
        // When rawValue is set on an exclGroup child, the setter copies the value
        // to the parent exclGroup's _rawValue, avoiding the need for batch sync.
        self.context
            .global_object()
            .set(
                PropertyKey::from(js_string!("_xfa_tmp_")),
                JsValue::from(field.clone()),
                false,
                &mut self.context,
            )
            .ok();
        let _ = self.context.eval(Source::from_bytes(
            r#"Object.defineProperty(_xfa_tmp_, 'rawValue', {
                get: function() {
                    var v = this._rawValue;
                    return (v !== undefined && v !== null) ? v : '';
                },
                set: function(v) {
                    this._rawValue = v;
                    if (this._exclGroupParent) {
                        this._exclGroupParent._rawValue = v;
                    }
                },
                configurable: true,
                enumerable: true
            });"#,
        ));

        field
            .set(
                PropertyKey::from(js_string!("value")),
                JsValue::from(js_string!(initial_value)),
                false,
                &mut self.context,
            )
            .ok();

        field
            .set(
                PropertyKey::from(js_string!("presence")),
                JsValue::from(js_string!(initial_presence)),
                false,
                &mut self.context,
            )
            .ok();

        field
    }

    /// Register a path on a JS object, creating intermediate objects as needed.
    fn register_path_on_object(&mut self, root: &JsObject, path: &str, field_obj: JsObject) {
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = root.clone();
        let mut current_path = String::new();

        for (i, part) in parts.iter().enumerate() {
            let key = PropertyKey::from(js_string!(*part));

            // Build the current path for this component
            if current_path.is_empty() {
                current_path = part.to_string();
            } else {
                current_path = format!("{}.{}", current_path, part);
            }

            if i == parts.len() - 1 {
                // Final component - set the actual field object
                current
                    .set(key, field_obj.clone(), false, &mut self.context)
                    .ok();

                // Store in field_objects by the shortened path
                let som_path = SomPath::new(&current_path);
                self.field_objects
                    .insert(som_path.clone(), field_obj.clone());

                // Also track in field_objects_by_name
                self.field_objects_by_name
                    .entry(part.to_string())
                    .or_default()
                    .push(som_path);
            } else {
                let existing = current
                    .get(key.clone(), &mut self.context)
                    .unwrap_or(JsValue::undefined());

                if existing.is_undefined() {
                    let som_path = SomPath::new(&current_path);
                    let (intermediate, _reused) =
                        if let Some(existing_obj) = self.field_objects.get(&som_path) {
                            (existing_obj.clone(), true)
                        } else {
                            // Create new intermediate object with presence property
                            let new_obj = ObjectInitializer::new(&mut self.context)
                                .property(
                                    js_string!("name"),
                                    JsValue::from(js_string!(*part)),
                                    Attribute::READONLY,
                                )
                                .property(
                                    js_string!("somExpression"),
                                    JsValue::from(js_string!(current_path.as_str())),
                                    Attribute::READONLY,
                                )
                                .property(
                                    js_string!("presence"),
                                    JsValue::from(js_string!("visible")),
                                    Attribute::all(),
                                )
                                .build();

                            self.field_objects.insert(som_path.clone(), new_obj.clone());
                            self.initial_presence
                                .insert(som_path.clone(), "visible".to_string());

                            self.field_objects_by_name
                                .entry(part.to_string())
                                .or_default()
                                .push(som_path.clone());

                            (new_obj, false)
                        };

                    current
                        .set(key.clone(), intermediate.clone(), false, &mut self.context)
                        .ok();
                    current = intermediate;
                } else if let Some(obj) = existing.as_object() {
                    let som_path = SomPath::new(&current_path);
                    let in_field_objects = self.field_objects.contains_key(&som_path);

                    if !in_field_objects {
                        self.field_objects.insert(som_path.clone(), obj.clone());
                        self.initial_presence
                            .insert(som_path.clone(), "visible".to_string());
                    }

                    current = obj.clone();
                } else {
                    break;
                }
            }
        }
    }

    pub fn register_global_variable(&mut self, name: &str, value: JsObject) {
        self.context
            .register_global_property(JsString::from(name), value, Attribute::all())
            .ok();
    }

    pub fn register_translation_object(
        &mut self,
        name: &str,
        translations: HashMap<String, String>,
    ) {
        let obj = ObjectInitializer::new(&mut self.context).build();

        for (key, value) in translations {
            obj.set(
                PropertyKey::from(JsString::from(key.as_str())),
                JsValue::from(js_string!(value.as_str())),
                false,
                &mut self.context,
            )
            .ok();
        }

        self.context
            .register_global_property(JsString::from(name), obj, Attribute::all())
            .ok();
    }

    /// Record a dependency for cascading calculations
    pub fn add_dependency(&mut self, dependent_field: &SomPath, source_field: &SomPath) {
        self.dependencies
            .add_dependency(dependent_field, source_field);
    }

    /// Get fields that need recalculation when a value changes
    pub fn get_fields_to_recalculate(&self, changed_field: &SomPath) -> Vec<SomPath> {
        self.dependencies.get_dependents(changed_field)
    }

    /// Resolve a SOM expression (for use from Rust side)
    pub fn resolve_node(&self, som_expression: &str) -> Option<SomPath> {
        self.som_resolver
            .resolve_node(som_expression, self.current_field_path.as_ref())
    }

    /// Resolve a SOM expression to multiple nodes
    pub fn resolve_nodes(&self, som_expression: &str) -> Vec<SomPath> {
        self.som_resolver
            .resolve_nodes(som_expression, self.current_field_path.as_ref())
    }

    /// Resolve a field name to its full SOM path using context-aware resolution.
    pub fn resolve_field_by_name_with_context(&self, field_name: &str) -> Option<SomPath> {
        // If it's already a full path, just return it
        if field_name.contains('.') {
            return Some(SomPath::new(field_name));
        }

        // Get all paths that have this field name
        let paths = self.field_objects_by_name.get(field_name)?;

        if paths.is_empty() {
            return None;
        }

        // If there's only one, return it
        if paths.len() == 1 {
            return Some(paths[0].clone());
        }

        // Multiple paths exist - use context-aware resolution
        let context_path = self
            .current_context_path
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_default();

        if context_path.is_empty() {
            return Some(paths[0].clone());
        }

        // Try to find a path that's a child of the current context
        for path in paths {
            let path_str = path.to_string();
            if path_str.starts_with(&format!("{}.", context_path)) {
                return Some(path.clone());
            }
        }

        // Try to find one that shares a common ancestor with context
        let context_parts: Vec<&str> = context_path.split('.').collect();
        let mut best_match: Option<(&SomPath, usize)> = None;

        for path in paths {
            let path_str = path.to_string();
            let path_parts: Vec<&str> = path_str.split('.').collect();

            let shared = context_parts
                .iter()
                .zip(path_parts.iter())
                .take_while(|(a, b)| a == b)
                .count();

            if shared > 0 {
                match &best_match {
                    Some((_, best_score)) if shared > *best_score => {
                        best_match = Some((path, shared));
                    }
                    None => {
                        best_match = Some((path, shared));
                    }
                    _ => {}
                }
            }
        }

        best_match
            .map(|(path, _)| path.clone())
            .or_else(|| Some(paths[0].clone()))
    }

    /// Get the JavaScript field object for a field, using context-aware resolution.
    pub fn get_field_object_by_name(&self, field_name: &str) -> Option<&JsObject> {
        let resolved_path = self.resolve_field_by_name_with_context(field_name)?;
        self.field_objects.get(&resolved_path)
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
                let s = val
                    .to_string(&mut self.context)
                    .map(|js_str| js_str.to_std_string_escaped())
                    .unwrap_or_else(|_| "<<error>>".to_string());
                Ok(s)
            }
            Err(e) => Err(format!("Evaluation error: {}", e)),
        }
    }

    pub fn set_current_field(&mut self, path: &str, name: &str, value: &str) {
        self.current_field_path = Some(SomPath::new(path));
        self.current_context_path = Some(SomPath::new(path));

        // Update the JS global _xfa_current_context_
        self.context
            .register_global_property(
                js_string!("_xfa_current_context_"),
                JsValue::from(js_string!(path)),
                Attribute::all(),
            )
            .ok();

        // Reuse the already-registered field object so that properties like
        // _exclGroupParent (set during register_xfa_node) are preserved.
        let som_path = SomPath::new(path);
        let this_obj = if let Some(existing) = self.field_objects.get(&som_path) {
            existing.clone()
        } else {
            self.create_field_object(name, path, value)
        };
        self.context
            .register_global_property(js_string!("_xfa_this_"), this_obj, Attribute::all())
            .ok();
    }

    /// Set up the current field context with child fields as properties of `this`.
    pub fn set_current_field_with_children(
        &mut self,
        path: &str,
        name: &str,
        value: &str,
        children: &[(String, String)],
    ) {
        self.current_field_path = Some(SomPath::new(path));
        self.current_context_path = Some(SomPath::new(path));
        self.context
            .register_global_property(
                js_string!("_xfa_current_context_"),
                JsValue::from(js_string!(path)),
                Attribute::all(),
            )
            .ok();

        let this_obj = {
            let som_path = SomPath::new(path);
            if let Some(existing) = self.field_objects.get(&som_path) {
                existing.clone()
            } else {
                self.create_field_object(name, path, value)
            }
        };

        // Track which child names map to which IDs
        self.child_name_to_id.clear();

        // Add child fields as properties of `this`
        for (child_name, child_id) in children {
            let child_path = format!("{}.{}", path, child_name);
            let child_som_path = SomPath::new(&child_path);

            // Reuse existing field object if available
            let child_obj = if let Some(existing) = self.field_objects.get(&child_som_path) {
                existing.clone()
            } else {
                let new_obj = self.create_field_object(child_name, &child_path, "");
                self.field_objects.insert(child_som_path, new_obj.clone());
                new_obj
            };

            self.child_name_to_id
                .insert(child_name.clone(), child_id.clone());

            let property_key = PropertyKey::from(JsString::from(child_name.as_str()));

            this_obj
                .define_property_or_throw(
                    property_key.clone(),
                    boa_engine::property::PropertyDescriptor::builder()
                        .value(child_obj.clone())
                        .writable(true)
                        .enumerable(true)
                        .configurable(true)
                        .build(),
                    &mut self.context,
                )
                .ok();
        }

        self.context
            .register_global_property(js_string!("_xfa_this_"), this_obj.clone(), Attribute::all())
            .ok();
    }

    /// Get the value of a child field that was set via `this.childName.rawValue = ...`
    pub fn get_child_field_value(&mut self, child_name: &str) -> Option<(String, String)> {
        let child_id = self
            .child_name_to_id
            .get(child_name)
            .cloned()
            .unwrap_or_default();

        if let Ok(this_val) = self.context.global_object().get(
            PropertyKey::from(js_string!("_xfa_this_")),
            &mut self.context,
        ) {
            if let Some(this_obj) = this_val.as_object() {
                if let Ok(child_val) = this_obj.get(
                    PropertyKey::from(JsString::from(child_name)),
                    &mut self.context,
                ) {
                    if let Some(child_obj) = child_val.as_object() {
                        if let Ok(raw_value) = child_obj
                            .get(PropertyKey::from(js_string!("rawValue")), &mut self.context)
                        {
                            if !raw_value.is_undefined() && !raw_value.is_null() {
                                let value = raw_value
                                    .to_string(&mut self.context)
                                    .ok()
                                    .map(|s| s.to_std_string_escaped())?;
                                return Some((child_id, value));
                            }
                        }
                    }
                }
            }
        }

        // Fallback: check form state
        let state = self.form_state.read().ok()?;
        let child_path = SomPath::new(child_name);
        state
            .get_value(&child_path)
            .map(|v| (child_id, v.as_string()))
    }

    /// Get the value of a field from the SOM hierarchy by its full path.
    pub fn get_som_field_value(&mut self, path: &str) -> Option<String> {
        let som_path = SomPath::new(path);
        if let Some(field_obj) = self.field_objects.get(&som_path) {
            if let Ok(raw_value) =
                field_obj.get(PropertyKey::from(js_string!("rawValue")), &mut self.context)
            {
                if !raw_value.is_undefined() && !raw_value.is_null() {
                    return raw_value
                        .to_string(&mut self.context)
                        .ok()
                        .map(|s| s.to_std_string_escaped());
                }
            }
        }
        None
    }

    /// Get all field values from the SOM hierarchy that have been modified.
    pub fn get_all_som_field_values(&mut self) -> HashMap<String, String> {
        let mut values = HashMap::new();

        for (path, obj) in &self.field_objects {
            if let Ok(raw_value) =
                obj.get(PropertyKey::from(js_string!("rawValue")), &mut self.context)
            {
                if !raw_value.is_undefined() && !raw_value.is_null() {
                    if let Ok(value_str) = raw_value.to_string(&mut self.context) {
                        let value = value_str.to_std_string_escaped();
                        // Empty strings are valid per XFA spec (cleared fields, deselected exclGroups)
                        let field_name = path.name();
                        values.insert(field_name.to_string(), value);
                    }
                }
            }
        }

        values
    }

    /// Get all presence changes from the SOM hierarchy that have been modified.
    pub fn get_all_som_presence_changes(&mut self) -> HashMap<String, String> {
        let mut changes = HashMap::new();

        for (path, obj) in &self.field_objects {
            if let Ok(presence) =
                obj.get(PropertyKey::from(js_string!("presence")), &mut self.context)
            {
                if !presence.is_undefined() && !presence.is_null() {
                    if let Ok(presence_str) = presence.to_string(&mut self.context) {
                        let presence_value = presence_str.to_std_string_escaped();

                        if !presence_value.is_empty() {
                            let initial = self
                                .initial_presence
                                .get(path)
                                .map(|s| s.as_str())
                                .unwrap_or("visible");

                            if presence_value.to_lowercase() != initial.to_lowercase() {
                                changes.insert(path.to_string(), presence_value);
                            }
                        }
                    }
                }
            }
        }

        changes
    }

    /// Update the initial presence for a field so subsequent change detection
    /// correctly recognizes reverts back to the "current" baseline.
    pub fn update_initial_presence(&mut self, path: &SomPath, presence: &str) {
        self.initial_presence
            .insert(path.clone(), presence.to_string());
    }

    pub fn execute_script(&mut self, script: &XfaScript) -> Result<Option<String>, String> {
        match script.content_type {
            ScriptContentType::JavaScript => self.execute_javascript(&script.source),
            ScriptContentType::FormCalc => {
                Err("FormCalc scripts require transpilation (not yet implemented).".to_string())
            }
        }
    }

    fn execute_javascript(&mut self, source: &str) -> Result<Option<String>, String> {
        let this_obj = self
            .context
            .global_object()
            .get(
                PropertyKey::from(js_string!("_xfa_this_")),
                &mut self.context,
            )
            .ok();

        let has_this_context = this_obj
            .as_ref()
            .map(|v| !v.is_undefined())
            .unwrap_or(false);

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

        // Wrap the script in a function with proper `this` binding
        let wrapped_source = if has_this_context {
            format!("(function() {{ {} }}).call(_xfa_this_)", source)
        } else {
            format!("(function() {{ {} }})()", source)
        };

        match self.context.eval(Source::from_bytes(&wrapped_source)) {
            Ok(result) => {
                if let Ok(this_val) = self.context.global_object().get(
                    PropertyKey::from(js_string!("_xfa_this_")),
                    &mut self.context,
                ) {
                    if let Some(this_obj) = this_val.as_object() {
                        if let Ok(raw_value) = this_obj
                            .get(PropertyKey::from(js_string!("rawValue")), &mut self.context)
                        {
                            if !raw_value.is_undefined() && !raw_value.is_null() {
                                let value_str = raw_value
                                    .to_string(&mut self.context)
                                    .map(|s| s.to_std_string_escaped())
                                    .unwrap_or_default();

                                let changed = initial_raw_value.as_ref() != Some(&value_str);

                                if changed {
                                    if let Some(ref path) = self.current_field_path {
                                        let mut state = self.form_state.write().unwrap();
                                        state.set_value(
                                            path.clone(),
                                            XfaValue::String(value_str.clone()),
                                        );
                                    }
                                    return Ok(Some(value_str));
                                }
                            }
                        }
                    }
                }

                if result.is_undefined() || result.is_null() {
                    Ok(None)
                } else {
                    Ok(Some(
                        result
                            .to_string(&mut self.context)
                            .map(|s| s.to_std_string_escaped())
                            .unwrap_or_default(),
                    ))
                }
            }
            Err(e) => Err(format!("JavaScript error: {}", e)),
        }
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
    /// `is_parent_exclgroup` should be `true` when this node's parent is an
    /// `<exclGroup>` element.  The caller determines this from the XFA tree
    /// structure so that we don't need naming-convention heuristics.
    ///
    /// `item_key` is the key value from `<items><text>…</text></items>` for
    /// exclGroup children. When the parent exclGroup's rawValue is set, children
    /// whose `_itemKey` matches the new value are turned ON (rawValue="1"),
    /// others OFF (rawValue=""). Per XFA 3.3 §4 pp.195-197.
    pub fn register_xfa_node(
        &mut self,
        name: &str,
        path: &str,
        parent_path: Option<&str>,
        is_field: bool,
        value: &str,
        is_parent_exclgroup: bool,
        item_key: Option<&str>,
    ) {
        let is_exclgroup_child = is_parent_exclgroup;

        // Create the JavaScript object for this node
        let node_obj = if is_field {
            let obj = self.create_field_object(name, path, value);
            // Store the item key for exclGroup parent→child propagation.
            // Per XFA 3.3 §4 pp.195-197: each child in an exclGroup has a key
            // value from <items>. When the parent's rawValue is set, children
            // compare their key to determine ON/OFF state.
            if let Some(key) = item_key {
                obj.set(
                    PropertyKey::from(js_string!("_itemKey")),
                    JsValue::from(js_string!(key)),
                    false,
                    &mut self.context,
                )
                .ok();
            }
            obj
        } else {
            // For subforms, create an object that can have children
            let subform_obj = ObjectInitializer::new(&mut self.context)
                .property(
                    js_string!("name"),
                    JsValue::from(js_string!(name)),
                    Attribute::READONLY,
                )
                .property(
                    js_string!("somExpression"),
                    JsValue::from(js_string!(path)),
                    Attribute::READONLY,
                )
                .property(
                    js_string!("presence"),
                    JsValue::from(js_string!("visible")),
                    Attribute::all(),
                )
                .build();

            // Add stub instanceManager for dynamic subforms
            let set_instances =
                NativeFunction::from_fn_ptr(|_this, _args, _context| Ok(JsValue::undefined()));
            let instance_manager = ObjectInitializer::new(&mut self.context)
                .function(set_instances, js_string!("setInstances"), 1)
                .build();
            subform_obj
                .set(
                    PropertyKey::from(js_string!("instanceManager")),
                    instance_manager,
                    false,
                    &mut self.context,
                )
                .ok();

            // All containers can have rawValue per XFA spec (exclGroups need it
            // for child→parent value propagation).
            subform_obj
                .set(
                    PropertyKey::from(js_string!("_rawValue")),
                    JsValue::from(js_string!(value)),
                    false,
                    &mut self.context,
                )
                .ok();

            // Initialize _exclGroupChildren as an empty array on all container
            // objects. For exclGroups, children will be pushed onto this array
            // during child registration; the rawValue setter iterates it to
            // propagate parent→child state changes.
            self.context
                .global_object()
                .set(
                    PropertyKey::from(js_string!("_xfa_tmp_")),
                    JsValue::from(subform_obj.clone()),
                    false,
                    &mut self.context,
                )
                .ok();
            let _ = self
                .context
                .eval(Source::from_bytes(r#"_xfa_tmp_._exclGroupChildren = [];"#));

            self.context
                .global_object()
                .set(
                    PropertyKey::from(js_string!("_xfa_tmp_")),
                    JsValue::from(subform_obj.clone()),
                    false,
                    &mut self.context,
                )
                .ok();
            let _ = self.context.eval(Source::from_bytes(
                r#"Object.defineProperty(_xfa_tmp_, 'rawValue', {
                    get: function() {
                        var v = this._rawValue;
                        return (v !== undefined && v !== null) ? v : '';
                    },
                    set: function(v) {
                        this._rawValue = v;
                        if (this._exclGroupChildren && this._exclGroupChildren.length > 0) {
                            for (var i = 0; i < this._exclGroupChildren.length; i++) {
                                var child = this._exclGroupChildren[i];
                                if (child._itemKey !== undefined) {
                                    child._rawValue = (child._itemKey === v) ? '1' : '';
                                }
                            }
                        }
                        if (this._exclGroupParent) {
                            this._exclGroupParent._rawValue = v;
                        }
                    },
                    configurable: true,
                    enumerable: true
                });"#,
            ));

            subform_obj
        };

        let som_path = SomPath::new(path);
        let parent_som_path = parent_path.map(SomPath::new);

        // Skip re-registration if this path is already registered (e.g., duplicate
        // pageArea children in pageSet). Re-registering would create a new JS object
        // that loses child properties set up on the first registration.
        if self.field_objects.contains_key(&som_path) {
            return;
        }

        // Store in field_objects for later lookup
        if std::env::var("XFA_DEBUG").is_ok() {
            eprintln!("[REG] path={path} name={name} is_field={is_field} parent={parent_path:?}");
        }
        self.field_objects
            .insert(som_path.clone(), node_obj.clone());

        // Link child to parent exclGroup for automatic rawValue propagation.
        // The rawValue setter on the child checks _exclGroupParent and copies
        // the value to the parent's _rawValue when it's set.
        // Also push the child onto the parent's _exclGroupChildren array so
        // that parent→child propagation works (XFA 3.3 §4 pp.195-197).
        if is_exclgroup_child {
            if let Some(parent) = parent_path {
                let parent_som = SomPath::new(parent);
                if let Some(parent_obj) = self.field_objects.get(&parent_som) {
                    node_obj
                        .set(
                            PropertyKey::from(js_string!("_exclGroupParent")),
                            JsValue::from(parent_obj.clone()),
                            false,
                            &mut self.context,
                        )
                        .ok();

                    // Push this child onto parent's _exclGroupChildren array
                    self.context
                        .global_object()
                        .set(
                            PropertyKey::from(js_string!("_xfa_excl_parent_")),
                            JsValue::from(parent_obj.clone()),
                            false,
                            &mut self.context,
                        )
                        .ok();
                    self.context
                        .global_object()
                        .set(
                            PropertyKey::from(js_string!("_xfa_excl_child_")),
                            JsValue::from(node_obj.clone()),
                            false,
                            &mut self.context,
                        )
                        .ok();
                    let _ = self.context.eval(Source::from_bytes(
                        r#"_xfa_excl_parent_._exclGroupChildren.push(_xfa_excl_child_);"#,
                    ));
                }
            }
        }

        // Register in SOM resolver
        self.som_resolver.register_node(
            &som_path,
            name,
            if is_field { "field" } else { "subform" },
            parent_som_path.as_ref(),
        );

        // If there's a parent, add this node as a child property
        if let Some(ref parent_som) = parent_som_path {
            if let Some(parent_obj) = self.field_objects.get(parent_som) {
                parent_obj
                    .set(
                        PropertyKey::from(JsString::from(name)),
                        node_obj.clone(),
                        false,
                        &mut self.context,
                    )
                    .ok();
            }
        } else {
            // Top-level subform - register as a global variable
            self.context
                .register_global_property(JsString::from(name), node_obj.clone(), Attribute::all())
                .ok();
        }

        // Also register in the _xfa_fields_ registry for resolveNode() lookups
        if let Ok(registry) = self.context.global_object().get(
            PropertyKey::from(js_string!("_xfa_fields_")),
            &mut self.context,
        ) {
            if let Some(registry_obj) = registry.as_object() {
                registry_obj
                    .set(
                        PropertyKey::from(JsString::from(name)),
                        node_obj.clone(),
                        false,
                        &mut self.context,
                    )
                    .ok();
            }
        }

        // For floating fields (registered without parent), also add as property on all existing subforms
        if is_field && parent_path.is_none() {
            for subform_obj in self.field_objects.values() {
                if let Ok(som) = subform_obj.get(
                    PropertyKey::from(js_string!("somExpression")),
                    &mut self.context,
                ) {
                    if !som.is_undefined() {
                        subform_obj
                            .set(
                                PropertyKey::from(JsString::from(name)),
                                node_obj.clone(),
                                false,
                                &mut self.context,
                            )
                            .ok();
                    }
                }
            }
        }
    }

    /// Get the current presence value set on `this` by a script.
    pub fn get_current_field_presence(&mut self) -> Option<Presence> {
        if let Ok(this_val) = self.context.global_object().get(
            PropertyKey::from(js_string!("_xfa_this_")),
            &mut self.context,
        ) {
            if let Some(this_obj) = this_val.as_object() {
                if let Ok(presence) =
                    this_obj.get(PropertyKey::from(js_string!("presence")), &mut self.context)
                {
                    if !presence.is_undefined() && !presence.is_null() {
                        let presence_str = presence
                            .to_string(&mut self.context)
                            .ok()
                            .map(|s| s.to_std_string_escaped())?;
                        if matches!(
                            presence_str.as_str(),
                            "visible" | "invisible" | "hidden" | "inactive"
                        ) {
                            return Some(Presence::from_str(&presence_str));
                        }
                    }
                }
            }
        }
        None
    }

    /// Get the presence value of a child field that was set via `this.childName.presence = ...`
    pub fn get_child_field_presence(&mut self, child_name: &str) -> Option<(String, Presence)> {
        let child_id = self
            .child_name_to_id
            .get(child_name)
            .cloned()
            .unwrap_or_default();

        if let Ok(this_val) = self.context.global_object().get(
            PropertyKey::from(js_string!("_xfa_this_")),
            &mut self.context,
        ) {
            if let Some(this_obj) = this_val.as_object() {
                if let Ok(child_val) = this_obj.get(
                    PropertyKey::from(JsString::from(child_name)),
                    &mut self.context,
                ) {
                    if let Some(child_obj) = child_val.as_object() {
                        if let Ok(presence) = child_obj
                            .get(PropertyKey::from(js_string!("presence")), &mut self.context)
                        {
                            if !presence.is_undefined() && !presence.is_null() {
                                let presence_str = presence
                                    .to_string(&mut self.context)
                                    .ok()
                                    .map(|s| s.to_std_string_escaped())?;
                                if matches!(
                                    presence_str.as_str(),
                                    "visible" | "invisible" | "hidden" | "inactive"
                                ) {
                                    return Some((child_id, Presence::from_str(&presence_str)));
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }
}

impl Default for XfaScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}
