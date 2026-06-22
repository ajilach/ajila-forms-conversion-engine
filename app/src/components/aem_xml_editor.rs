//! Plain-text editor for the final AEM `.content.xml`.
//!
//! The structured editor edits the `StructuredNode` tree and the AEM editor
//! edits the `AemNode` tree; this editor sits one layer below them and edits the
//! generated JCR `.content.xml` **as text**. It mirrors the AEM editor's layout
//! and history machinery: a left pane (here a single textarea) and a 260px edit
//! history sidebar on the right, backed by a dedicated `<session>#aem-xml`
//! history session — the same session the conversion agent records its XML edits
//! into, so manual and agent XML history share one timeline.
//!
//! On **Apply** the edited XML is validated for well-formedness and, if valid,
//! the package is rebuilt from it verbatim via
//! [`blueprint::to_aem_package_from_node_with_xml`] (the XSD, translations and
//! DAM metadata still derive from the node tree).

use std::collections::HashMap;

use dioxus::prelude::*;
use uuid::Uuid;

use super::{AemConfigWrapper, AemRootWrapper};
use crate::db::{self, EditInfo};

/// Wrapper so the form-content translations map can be a prop without requiring
/// a meaningful `PartialEq` (the editor never diffs on it).
#[derive(Clone)]
pub struct TranslationsWrapper(pub HashMap<String, HashMap<String, String>>);
impl PartialEq for TranslationsWrapper {
    fn eq(&self, _: &Self) -> bool {
        false
    }
}

#[derive(Clone, PartialEq, Props)]
pub struct AemXmlEditorProps {
    /// The AEM node tree — rebuilds the package's XSD/translations/DAM on Apply.
    pub root: AemRootWrapper,
    /// The loaded AEM config.
    pub aem_config: AemConfigWrapper,
    /// Form-content translations (master-text → lang map) for the package's
    /// dictionaries on Apply.
    pub translations: TranslationsWrapper,
    /// The `.content.xml` generated from the node tree, shown as the initial text.
    pub initial_xml: String,
    /// Edit-history session id (desktop only; `None` on web). Conventionally
    /// `<structured-session>#aem-xml`.
    pub session_id: Option<String>,
    /// Called with the built package bytes when the user applies their edits.
    pub on_apply: EventHandler<Vec<u8>>,
    pub on_cancel: EventHandler<()>,
}

#[component]
pub fn AemXmlEditor(props: AemXmlEditorProps) -> Element {
    // `content` is the committed XML (also the textarea's stable `value`, so
    // typing doesn't reset the cursor); `local_text` is the live buffer.
    let mut content = use_signal(|| props.initial_xml.clone());
    let mut local_text = use_signal(|| props.initial_xml.clone());
    let mut status_msg = use_signal(|| None::<(bool, String)>);

    // Edit-history session (desktop only). Mirrors the AEM editor's handling of
    // the structured→AEM relationship, one layer down: the content XML is
    // generated from the AEM tree, so if the tree changed since the last
    // snapshot the freshly generated XML is appended as a normal
    // "Regenerated from AEM tree" entry instead of overwriting prior history
    // (prior raw edits stay reachable via the history sidebar / undo).
    let session_id = use_signal(|| -> Option<String> {
        let xml = props.initial_xml.clone();
        match props.session_id.clone() {
            Some(sid) => {
                let latest = db::latest_seq(&sid).and_then(|seq| db::snapshot_at(&sid, seq));
                match latest {
                    Some(prev) if prev == xml => {}
                    Some(_) => {
                        db::insert_edit(&sid, "Regenerated from AEM tree", &xml);
                    }
                    None => {
                        db::insert_edit(&sid, "Initial content XML", &xml);
                    }
                }
                Some(sid)
            }
            None => {
                let sid = Uuid::new_v4().to_string();
                db::insert_edit(&sid, "Initial content XML", &xml)?;
                Some(sid)
            }
        }
    });
    let mut undo_seq = use_signal(|| {
        session_id
            .read()
            .as_ref()
            .and_then(|sid| db::latest_seq(sid))
            .unwrap_or(0)
    });
    let mut history_version = use_signal(|| 0u64);

    let root = use_signal(|| props.root.0.clone());
    let aem_config = use_signal(|| props.aem_config.0.clone());
    let translations = use_signal(|| props.translations.0.clone());
    let on_apply = props.on_apply;
    let on_cancel = props.on_cancel;

    // Commit the live buffer into `content` and snapshot it, if it changed.
    // `local_text` is intentionally never read in the render body, so typing
    // into it triggers no re-render — that is what keeps the textarea from
    // being reset to `content` (the previous value) on every keystroke.
    let mut commit = move || {
        let new = local_text.read().clone();
        if new != *content.read() {
            content.set(new.clone());
            // A fresh edit clears any stale "cannot apply" error.
            status_msg.set(None);
            if let Some(sid) = session_id.read().clone() {
                let after_seq = *undo_seq.read();
                if let Some(seq) = db::record_edit(&sid, after_seq, "Edit content XML", &new) {
                    undo_seq.set(seq);
                    history_version += 1;
                }
            }
        }
    };

    // ── History state (history_version is the refresh dependency) ───────────
    let _history_version = *history_version.read();
    let has_session = session_id.read().is_some();
    let max_seq = session_id
        .read()
        .as_ref()
        .and_then(|sid| db::latest_seq(sid))
        .unwrap_or(0);
    let current_seq = *undo_seq.read();
    let can_undo = has_session && current_seq > 0;
    let can_redo = has_session && current_seq < max_seq;
    let history_entries: Vec<EditInfo> = session_id
        .read()
        .as_ref()
        .map(|sid| db::list_edits(sid))
        .unwrap_or_default();

    // Load a snapshot into the editor (undo/redo and history clicks).
    let mut load_snapshot = move |target_seq: usize| {
        let Some(sid) = session_id.read().clone() else {
            return;
        };
        let Some(xml) = db::snapshot_at(&sid, target_seq) else {
            return;
        };
        content.set(xml.clone());
        local_text.set(xml);
        undo_seq.set(target_seq);
        history_version += 1;
    };
    let do_undo = move |_| {
        let cur = *undo_seq.read();
        if cur > 0 {
            load_snapshot(cur - 1);
        }
    };
    let do_redo = move |_| {
        let cur = *undo_seq.read();
        load_snapshot(cur + 1);
    };

    // Live well-formedness status of the committed text.
    let validation = blueprint::validate_xml_wellformed(&content.read());
    let byte_len = content.read().len();

    rsx! {
        div { class: "aem-editor-shell",
            div { class: "aem-editor aem-xml-editor",
                // Header
                div { class: "editor-header",
                    h2 { "Edit content XML" }
                    div { class: "editor-header-actions",
                        if has_session {
                            button {
                                class: "editor-btn",
                                title: "Undo",
                                disabled: !can_undo,
                                onclick: do_undo,
                                "↶ Undo"
                            }
                            button {
                                class: "editor-btn",
                                title: "Redo",
                                disabled: !can_redo,
                                onclick: do_redo,
                                "↷ Redo"
                            }
                        }
                        button {
                            class: "editor-btn editor-btn-secondary",
                            onclick: move |_| on_cancel.call(()),
                            "Cancel"
                        }
                        button {
                            class: "editor-btn editor-btn-primary",
                            onclick: move |_| {
                                commit();
                                let xml = content.read().clone();
                                match blueprint::validate_xml_wellformed(&xml) {
                                    Ok(()) => {
                                        let zip = blueprint::to_aem_package_from_node_with_xml(
                                            &root.read(),
                                            &aem_config.read(),
                                            translations.read().clone(),
                                            xml,
                                        );
                                        on_apply.call(zip);
                                    }
                                    Err(e) => {
                                        status_msg.set(Some((false, format!("Cannot apply — invalid XML: {e}"))));
                                    }
                                }
                            },
                            "Apply"
                        }
                    }
                }

                // Status line: explicit apply error, else live well-formedness.
                if let Some((ok, msg)) = status_msg.read().clone() {
                    div { class: if ok { "aem-status aem-status-ok" } else { "aem-status aem-status-err" }, "{msg}" }
                } else if let Err(e) = &validation {
                    div { class: "aem-status aem-status-err", "⚠ Not well-formed: {e}" }
                } else {
                    div { class: "aem-status aem-status-ok", "Well-formed · {byte_len} bytes" }
                }

                // Editor textarea (value bound to committed `content` so typing
                // doesn't reset the cursor; load_snapshot updates it).
                textarea {
                    class: "aem-xml-textarea",
                    spellcheck: false,
                    autocomplete: "off",
                    // `value` is bound to the committed text, which only changes
                    // on commit (blur) and undo/redo/history — never mid-typing —
                    // so the buffer below can collect keystrokes without the DOM
                    // value being reset under the cursor.
                    value: "{content}",
                    oninput: move |evt: Event<FormData>| {
                        local_text.set(evt.value());
                    },
                    onblur: move |_| commit(),
                }
            } // end aem-xml-editor

            // ── Edit history sidebar (desktop only) ─────────────────────────
            if has_session {
                aside { class: "history-sidebar",
                    div { class: "history-header",
                        h3 { "History" }
                    }
                    div { class: "history-list",
                        for entry in history_entries.iter().rev() {
                            {
                                let seq = entry.seq;
                                let is_current = seq == current_seq;
                                let is_future = seq > current_seq;
                                let mut item_class = String::from("history-item");
                                if is_current {
                                    item_class.push_str(" history-item-current");
                                }
                                if is_future {
                                    item_class.push_str(" history-item-future");
                                }
                                rsx! {
                                    button {
                                        key: "{seq}",
                                        class: "{item_class}",
                                        onclick: move |_| load_snapshot(seq),
                                        div { class: "history-item-label", "{entry.action_label}" }
                                        div { class: "history-item-time", "{db::format_timestamp(&entry.created_at)}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } // end aem-editor-shell
    }
}
