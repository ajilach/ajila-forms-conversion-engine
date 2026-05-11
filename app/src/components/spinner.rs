//! Reusable spinner component.

use dioxus::prelude::*;

/// A CSS-animated spinner.
///
/// `size` controls the CSS class suffix: `"sm"`, `"md"`, or `"lg"`.
/// Defaults to `"md"` if omitted.
#[component]
pub fn Spinner(#[props(default = "md".to_string())] size: String) -> Element {
    let class = format!("spinner spinner-{size}");
    rsx! {
        span { class: "{class}" }
    }
}
