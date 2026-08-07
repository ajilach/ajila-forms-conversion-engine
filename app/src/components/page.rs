//! Shared chrome for the full-page views (settings, reference forms): the page
//! shell, its header with a Close button, the scrolling content area, and the
//! label/description column every settings-style row starts with.

use dioxus::prelude::*;

/// The label + description column of a `.row`, on either full-page view.
#[component]
pub fn RowInfo(label: &'static str, desc: String) -> Element {
    rsx! {
        div { class: "row-info",
            span { class: "row-label", "{label}" }
            span { class: "row-desc", "{desc}" }
        }
    }
}

/// A full-page view under the persistent app header.
#[component]
pub fn FullPage(
    title: &'static str,
    /// Optional line under the title, e.g. a summary count.
    subtitle: Option<String>,
    on_close: EventHandler<()>,
    /// Tab bar and content — everything below the header.
    children: Element,
) -> Element {
    rsx! {
        div { class: "page",
            div { class: "page-header",
                div {
                    h2 { "{title}" }
                    if let Some(subtitle) = subtitle.as_ref() {
                        span { class: "page-subtitle", "{subtitle}" }
                    }
                }
                button {
                    class: "btn btn-secondary",
                    onclick: move |_| on_close.call(()),
                    "✕ Close"
                }
            }
            {children}
        }
    }
}
