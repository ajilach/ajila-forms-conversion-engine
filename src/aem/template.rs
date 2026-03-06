//! Tera template rendering for AEM profile variable resolution.
//!
//! Provides functions to build template contexts, resolve user-defined
//! variables iteratively, and render individual template strings.

use std::collections::HashMap;
use tera::{Context, Tera};

/// Build a Tera context from XFA variables and resolved user variables.
///
/// The context provides two namespaces:
/// - `xfa`       → `{{ xfa.formrange_code }}`, `{{ xfa.formrange_entity }}`, …
/// - `variables` → `{{ variables.entity_dir }}`, `{{ variables.prefix_dir }}`, …
pub fn build_context(
    xfa_vars: &HashMap<String, String>,
    user_vars: &HashMap<String, String>,
) -> Context {
    let mut ctx = Context::new();
    ctx.insert("xfa", xfa_vars);
    ctx.insert("variables", user_vars);
    ctx
}

/// Render a single Tera template string with the given context.
pub fn render_string(template: &str, ctx: &Context) -> Result<String, crate::Error> {
    Tera::one_off(template, ctx, false)
        .map_err(|e| crate::Error::AemConfig(format!("template render error: {}", e)))
}

/// Resolve the `[variables]` section of a profile.
///
/// Each variable value is itself a Tera template that can reference `xfa.*`
/// and previously resolved `variables.*`. Variables are resolved iteratively:
/// on each pass every variable is re-rendered with the current set of resolved
/// values. Resolution continues until values stabilise.
///
/// Returns an error if a Tera syntax error prevents rendering.
pub fn resolve_variables(
    raw_vars: &HashMap<String, String>,
    xfa_vars: &HashMap<String, String>,
) -> Result<HashMap<String, String>, crate::Error> {
    let mut resolved: HashMap<String, String> = HashMap::new();

    // Multiple passes until stable
    let max_passes = raw_vars.len() + 1;
    for _ in 0..max_passes {
        let mut changed = false;
        for (name, template) in raw_vars {
            let ctx = build_context(xfa_vars, &resolved);
            let value = render_string(template, &ctx)?;

            if resolved.get(name) != Some(&value) {
                resolved.insert(name.clone(), value);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_simple_variables() {
        let mut xfa = HashMap::new();
        xfa.insert("formrange_code".into(), "AAEI".into());
        xfa.insert("formrange_entity".into(), "019".into());

        let mut raw = HashMap::new();
        raw.insert("code".into(), "{{ xfa.formrange_code }}".into());

        let resolved = resolve_variables(&raw, &xfa).unwrap();
        assert_eq!(resolved["code"], "AAEI");
    }

    #[test]
    fn test_resolve_chained_variables() {
        let mut xfa = HashMap::new();
        xfa.insert("formrange_code".into(), "AAEI".into());

        let mut raw = HashMap::new();
        raw.insert("code".into(), "{{ xfa.formrange_code }}".into());
        raw.insert("dir".into(), "AF_{{ variables.code }}".into());

        let resolved = resolve_variables(&raw, &xfa).unwrap();
        assert_eq!(resolved["code"], "AAEI");
        assert_eq!(resolved["dir"], "AF_AAEI");
    }

    #[test]
    fn test_render_string_with_context() {
        let mut xfa = HashMap::new();
        xfa.insert("lang".into(), "DE".into());
        let user = HashMap::new();

        let ctx = build_context(&xfa, &user);
        let result = render_string("Language: {{ xfa.lang }}", &ctx).unwrap();
        assert_eq!(result, "Language: DE");
    }
}
