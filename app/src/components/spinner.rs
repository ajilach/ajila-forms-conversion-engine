//! Reusable spinner component.

use dioxus::prelude::*;

/// The sizes the stylesheet defines a spinner for.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SpinnerSize {
    /// Inline with body text, e.g. inside a button or a timeline dot.
    Sm,
    #[default]
    Md,
}

impl SpinnerSize {
    fn class(self) -> &'static str {
        match self {
            Self::Sm => "spinner spinner-sm",
            Self::Md => "spinner spinner-md",
        }
    }
}

/// A CSS-animated spinner.
#[component]
pub fn Spinner(#[props(default)] size: SpinnerSize) -> Element {
    rsx! {
        span { class: size.class() }
    }
}
