//! Multilingual text editor component.
//!
//! Provides a tabbed interface for editing text content in multiple languages.

use dioxus::prelude::*;
use std::collections::BTreeSet;

use blueprint::InlineText;

use super::state::{EditorAction, NodePath};

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

    // Track the currently active language tab
    // Initialize to the first available language (document or content), or "default"
    let mut active_lang = use_signal(|| {
        all_langs
            .first()
            .cloned()
            .unwrap_or_else(|| "default".to_string())
    });

    // Get current text for the active language
    let current_text = if all_langs.is_empty() || *active_lang.read() == "default" {
        props.content.0.as_plain_text()
    } else {
        props.content.0.plain_text_in(&active_lang.read())
    };

    let path = props.path.clone();
    let on_action = props.on_action.clone();

    rsx! {
        div { class: "text-editor",
            // Language tabs
            if !all_langs.is_empty() {
                div { class: "text-editor-tabs",
                    for lang in &all_langs {
                        button {
                            class: if *active_lang.read() == *lang { "text-editor-tab active" } else { "text-editor-tab" },
                            onclick: {
                                let lang = lang.clone();
                                move |_| active_lang.set(lang.clone())
                            },
                            "{lang}"
                        }
                    }
                }
            }

            // Text area
            div { class: "text-editor-content",
                textarea {
                    class: "text-editor-textarea",
                    value: "{current_text}",
                    rows: 4,
                    oninput: {
                        let path = path.clone();
                        let lang = active_lang.read().clone();
                        let on_action = on_action.clone();
                        move |evt: Event<FormData>| {
                            on_action
                                .call(EditorAction::UpdateText {
                                    path: path.clone(),
                                    content: evt.value(),
                                    language: if lang == "default" { None } else { Some(lang.clone()) },
                                });
                        }
                    },
                }
            }

            // Actions
            div { class: "text-editor-actions",
                button {
                    class: "text-editor-btn text-editor-btn-done",
                    onclick: {
                        let on_action = props.on_action.clone();
                        move |_| on_action.call(EditorAction::StopEditing)
                    },
                    "Done"
                }
            }
        }
    }
}

/// Properties for the list item editor.
#[derive(Clone, PartialEq, Props)]
pub struct ListItemEditorProps {
    /// The inline text content to edit.
    pub content: InlineTextWrapper,
    /// Path to the list node (not the item).
    pub list_path: NodePath,
    /// Index of the item in the list.
    pub item_index: usize,
    /// All languages available in the document.
    pub languages: Vec<String>,
    /// Callback when text is updated.
    pub on_action: EventHandler<EditorAction>,
}

/// Editor for a single list item with multilingual support.
#[component]
pub fn ListItemEditor(props: ListItemEditorProps) -> Element {
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

    // Track the currently active language tab
    // Initialize to the first available language (document or content), or "default"
    let mut active_lang = use_signal(|| {
        all_langs
            .first()
            .cloned()
            .unwrap_or_else(|| "default".to_string())
    });

    // Get current text for the active language
    let current_text = if all_langs.is_empty() || *active_lang.read() == "default" {
        props.content.0.as_plain_text()
    } else {
        props.content.0.plain_text_in(&active_lang.read())
    };

    let list_path = props.list_path.clone();
    let item_index = props.item_index;
    let on_action = props.on_action.clone();

    rsx! {
        div { class: "text-editor list-item-editor",
            // Language tabs
            if !all_langs.is_empty() {
                div { class: "text-editor-tabs",
                    for lang in &all_langs {
                        button {
                            class: if *active_lang.read() == *lang { "text-editor-tab active" } else { "text-editor-tab" },
                            onclick: {
                                let lang = lang.clone();
                                move |_| active_lang.set(lang.clone())
                            },
                            "{lang}"
                        }
                    }
                }
            }

            // Text area
            div { class: "text-editor-content",
                textarea {
                    class: "text-editor-textarea",
                    value: "{current_text}",
                    rows: 2,
                    oninput: {
                        let list_path = list_path.clone();
                        let lang = active_lang.read().clone();
                        let on_action = on_action.clone();
                        move |evt: Event<FormData>| {
                            on_action
                                .call(EditorAction::UpdateListItem {
                                    path: list_path.clone(),
                                    item_index,
                                    content: evt.value(),
                                    language: if lang == "default" { None } else { Some(lang.clone()) },
                                });
                        }
                    },
                }
            }

            // Actions
            div { class: "text-editor-actions",
                button {
                    class: "text-editor-btn text-editor-btn-done",
                    onclick: {
                        let on_action = props.on_action.clone();
                        move |_| on_action.call(EditorAction::StopEditing)
                    },
                    "Done"
                }
            }
        }
    }
}
