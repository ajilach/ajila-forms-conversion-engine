//! Inline editor for an AEM node's label / content / title, with per-language
//! tabs. The master-language tab edits the node's own string; other language
//! tabs edit the per-node translation overlay (see [`super::editor::AemEditCtx`]).

use dioxus::prelude::*;
use uuid::Uuid;

use super::editor::AemEditCtx;
use super::state::{AemEditorAction, AemPath};

#[derive(Clone, PartialEq, Props)]
pub struct AemTextEditorProps {
    /// Master-language text of the node.
    pub initial: String,
    pub path: AemPath,
    /// Node uuid (used to key translations); `None` for the Root.
    pub uuid: Option<Uuid>,
    pub on_action: EventHandler<AemEditorAction>,
}

#[component]
pub fn AemTextEditor(props: AemTextEditorProps) -> Element {
    let ctx = use_context::<AemEditCtx>();
    let master = ctx.master_lang.clone();
    let translations = ctx.translations;
    let uuid = props.uuid;
    let initial = props.initial.clone();

    // Language tabs: master first, then the rest. Only when this node can carry
    // translations (has a uuid) and more than one language exists.
    let mut langs: Vec<String> = vec![master.clone()];
    for l in &ctx.languages {
        if *l != master && !langs.contains(l) {
            langs.push(l.clone());
        }
    }
    let multilang = uuid.is_some() && langs.len() > 1;

    let value_for = {
        let master = master.clone();
        let initial = initial.clone();
        move |lang: &str| -> String {
            if lang == master {
                initial.clone()
            } else if let Some(u) = uuid {
                translations
                    .read()
                    .get(&u)
                    .and_then(|m| m.get(lang))
                    .cloned()
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
    };

    let mut active = use_signal(|| master.clone());
    let current = value_for(&active.read());
    let mut local = use_signal(|| current.clone());

    // Keep the local buffer in sync with the active language tab.
    {
        let value_for = value_for.clone();
        use_effect(move || {
            let lang = active.read().clone();
            local.set(value_for(&lang));
        });
    }

    let on_action = props.on_action;
    let path = props.path.clone();
    let master_for_commit = master.clone();
    let commit = move || {
        let lang = active.read().clone();
        let content = local.read().clone();
        if lang == master_for_commit {
            on_action.call(AemEditorAction::UpdateText {
                path: path.clone(),
                content,
            });
        } else {
            on_action.call(AemEditorAction::UpdateTranslation {
                path: path.clone(),
                language: lang,
                text: content,
            });
        }
    };

    rsx! {
        div { class: "aem-text-editor",
            if multilang {
                div { class: "aem-text-editor-tabs",
                    for lang in langs.clone() {
                        button {
                            class: if *active.read() == lang { "aem-text-editor-tab active" } else { "aem-text-editor-tab" },
                            onclick: {
                                let lang = lang.clone();
                                let commit = commit.clone();
                                move |_| {
                                    // Commit the current tab before switching.
                                    commit();
                                    active.set(lang.clone());
                                }
                            },
                            "{lang}"
                        }
                    }
                }
            }
            textarea {
                class: "aem-text-editor-area",
                rows: 3,
                value: "{current}",
                oninput: move |evt| local.set(evt.value()),
                onblur: {
                    let commit = commit.clone();
                    move |_| commit()
                },
            }
            div { class: "aem-text-editor-actions",
                button {
                    class: "aem-text-editor-btn",
                    onclick: move |_| {
                        commit();
                        on_action.call(AemEditorAction::StopEditing);
                    },
                    "Done"
                }
            }
        }
    }
}
