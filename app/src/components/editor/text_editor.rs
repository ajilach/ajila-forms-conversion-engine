//! Multilingual text editor component.
//!
//! Provides a tabbed interface for editing text content in multiple languages.

use dioxus::prelude::*;
use std::collections::BTreeSet;

use blueprint::InlineText;

use super::state::{EditorAction, NodePath};
use crate::markdown::inline_text_to_markdown;

/// Wrapper for InlineText that implements PartialEq.
#[derive(Clone)]
pub struct InlineTextWrapper(pub InlineText);

impl PartialEq for InlineTextWrapper {
    fn eq(&self, _other: &Self) -> bool {
        false // Always re-render
    }
}

/// Properties for the text editor.
#[derive(Clone, PartialEq, Props)]
pub struct TextEditorProps {
    /// The inline text content to edit.
    pub content: InlineTextWrapper,
    /// Path to the node being edited.
    pub path: NodePath,
    /// All languages available in the document.
    pub languages: Vec<String>,
    /// Callback when text is updated.
    pub on_action: EventHandler<EditorAction>,
}

/// Multilingual text editor with tabs for each language.
#[component]
pub fn TextEditor(props: TextEditorProps) -> Element {
    // Collect languages from the content
    let mut content_langs = BTreeSet::new();
    props.content.0.collect_languages(&mut content_langs);

    // Combine document languages with content languages
    let all_langs: Vec<String> = props
        .languages
        .iter()
        .chain(content_langs.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    // Detect which languages have missing translations
    let missing_langs = props.content.0.missing_translation_languages();

    // Track the currently active language tab
    // Initialize to the first available language (document or content), or "default"
    let mut active_lang = use_signal(|| {
        all_langs
            .first()
            .cloned()
            .unwrap_or_else(|| "default".to_string())
    });

    // Buffer the text locally to avoid controlled-textarea roundtrip issues
    // (markdown parsing can strip trailing spaces and re-rendering resets cursor position)
    let initial_text = if all_langs.is_empty() || *active_lang.read() == "default" {
        inline_text_to_markdown(&props.content.0, None)
    } else {
        inline_text_to_markdown(&props.content.0, Some(&active_lang.read()))
    };
    // local_text stores the current textarea value for retrieval on blur/done.
    // It is NOT read in the RSX template, so setting it won't trigger re-renders.
    let mut local_text = use_signal(|| initial_text.clone());

    let active_is_missing = missing_langs.contains(&*active_lang.read());

    let path = props.path.clone();
    let on_action = props.on_action;

    rsx! {
        div { class: "text-editor",
            // Language tabs
            if !all_langs.is_empty() {
                div { class: "text-editor-tabs",
                    for lang in &all_langs {
                        button {
                            class: {
                                let mut cls = String::from("text-editor-tab");
                                if *active_lang.read() == *lang {
                                    cls.push_str(" active");
                                }
                                if missing_langs.contains(lang) {
                                    cls.push_str(" missing-translation");
                                }
                                cls
                            },
                            onclick: {
                                let lang = lang.clone();
                                move |_| active_lang.set(lang.clone())
                            },
                            "{lang}"
                        }
                    }
                }
            }

            // Text area - uses initial_text (stable during typing) to avoid cursor resets
            div { class: "text-editor-content",
                textarea {
                    class: "text-editor-textarea",
                    value: "{initial_text}",
                    rows: 4,
                    oninput: move |evt: Event<FormData>| {
                        local_text.set(evt.value());
                    },
                    onblur: {
                        let path = path.clone();
                        let lang = active_lang.read().clone();
                        move |_| {
                            let content = local_text.read().clone();
                            on_action
                                .call(EditorAction::UpdateText {
                                    path: path.clone(),
                                    content,
                                    language: if lang == "default" { None } else { Some(lang.clone()) },
                                });
                        }
                    },
                }

                // Per-field inline warning for missing translation
                if active_is_missing {
                    div { class: "text-editor-warning", "⚠ Translation missing for this language" }
                }
            }

            // Actions
            div { class: "text-editor-actions",
                button {
                    class: "text-editor-btn text-editor-btn-done",
                    onclick: {
                        let path = path.clone();
                        let on_action = props.on_action;
                        let lang = active_lang.read().clone();
                        move |_| {
                            // Commit any pending text before stopping
                            let content = local_text.read().clone();
                            on_action
                                .call(EditorAction::UpdateText {
                                    path: path.clone(),
                                    content,
                                    language: if lang == "default" { None } else { Some(lang.clone()) },
                                });
                            on_action.call(EditorAction::StopEditing);
                        }
                    },
                    "Done"
                }
            }
        }
    }
}
