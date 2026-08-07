//! Shared chrome for the full-page views (settings, reference forms): the page
//! shell, its header with a Close button, and the scrolling content area.

use dioxus::prelude::*;

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
