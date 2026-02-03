//! JavaScript Helper Code
//!
//! This module contains JavaScript helper code that is injected into the
//! scripting environment to provide XFA-specific functionality.

/// Global SOM resolution helper function.
///
/// When a path like "Page.SectionTitle.STP_SectionTitle.ffrb1" is accessed,
/// JavaScript property chain works for subforms but fails for floating fields.
/// This helper provides fallback resolution.
pub const XFA_RESOLVE_PATH_HELPER: &str = r#"
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
"#;

/// Exclusion group value sync helper.
///
/// Per XFA spec, when a radio button's rawValue is set, the parent exclGroup should update.
/// This stores known parent-child relationships that get synced.
pub const XFA_EXCLGROUP_HELPER: &str = r#"
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

/// Combined JavaScript helpers for XFA environment setup.
pub fn get_all_helpers() -> String {
    format!("{}\n{}", XFA_RESOLVE_PATH_HELPER, XFA_EXCLGROUP_HELPER)
}
