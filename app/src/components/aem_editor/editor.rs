//! Top-level AEM node editor component.
//!
//! Edits an `AemNode` tree directly (the source of truth for the generated
//! package), with full editing parity with the structured editor plus the
//! Smart AEM Edit flow and on-demand upload to AEM.

use std::collections::HashMap;

use dioxus::prelude::*;
use uuid::Uuid;

use blueprint::{AemConfig, AemConnection, AemNode};

use super::node_renderer::{AemNodeItem, AemNodeWrapper};
use super::smart_edit::{self, AemSmartEditResult};
use super::state::{
    AemEditorAction, AemSelectionState, add_node, apply_metadata, can_indent, can_outdent,
    children_ref, collect_paths, convert_node, delete_nodes, describe_action, duplicate_node,
    for_each_labeled, get_node, get_node_mut, indent_node, is_container, move_node, node_uuid,
    outdent_node, set_editable_text,
};
use super::toolbar::AemToolbar;

/// Per-node, per-language label overlay: node uuid → { language → text }.
pub type NodeTranslations = HashMap<Uuid, HashMap<String, String>>;

/// Shared editor context for deep components (per-language label editing).
#[derive(Clone)]
pub struct AemEditCtx {
    pub master_lang: String,
    pub languages: Vec<String>,
    pub translations: Signal<NodeTranslations>,
}

/// Build the master-text → { lang → translation } dictionary the package
/// writer expects, from the per-node overlay keyed by uuid.
fn build_translation_dict(
    root: &AemNode,
    node_tr: &NodeTranslations,
) -> HashMap<String, HashMap<String, String>> {
    let mut out = HashMap::new();
    for_each_labeled(root, |uuid, label| {
        if let Some(map) = node_tr.get(&uuid)
            && !map.is_empty()
            && !label.is_empty()
        {
            out.insert(label.to_string(), map.clone());
        }
    });
    out
}

/// Build the package from the current node tree + per-language overlay.
fn build_zip(root: &AemNode, cfg: &AemConfig, node_tr: &NodeTranslations) -> Vec<u8> {
    let dict = build_translation_dict(root, node_tr);
    blueprint::to_aem_package_from_node_with_translations(root, cfg, dict)
}

/// Wrapper so `AemNode` can be a prop.
#[derive(Clone)]
pub struct AemRootWrapper(pub AemNode);
impl PartialEq for AemRootWrapper {
    fn eq(&self, _: &Self) -> bool {
        false
    }
}

/// Wrapper so `AemConfig` can be a prop.
#[derive(Clone)]
pub struct AemConfigWrapper(pub AemConfig);
impl PartialEq for AemConfigWrapper {
    fn eq(&self, _: &Self) -> bool {
        false
    }
}

/// Wrapper so `Option<AemConnection>` can be a prop.
#[derive(Clone)]
pub struct AemConnWrapper(pub Option<AemConnection>);
impl PartialEq for AemConnWrapper {
    fn eq(&self, _: &Self) -> bool {
        false
    }
}

#[derive(Clone)]
enum SmartState {
    Idle,
    Loading,
    Preview {
        result: AemSmartEditResult,
        elapsed_ms: u128,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, PartialEq, Props)]
pub struct AemEditorProps {
    pub root: AemRootWrapper,
    pub plain_images: HashMap<String, String>,
    /// Source PDF bytes (filename → bytes) for the full Smart Edit tool set.
    pub source_pdfs: Vec<(String, Vec<u8>)>,
    pub aem_config: AemConfigWrapper,
    pub connection: AemConnWrapper,
    /// Master language code (from the AEM config).
    pub master_lang: String,
    /// All languages present in the form.
    pub languages: Vec<String>,
    /// Form-content translations from the source document (master-text → lang map),
    /// used to seed the per-language label overlay.
    pub content_translations: HashMap<String, HashMap<String, String>>,
    pub api_key: String,
    pub model: String,
    /// Active conversion profile — scopes which reference forms Smart AEM Edit
    /// can search. `None` when no profile is selected.
    pub profile: Option<String>,
    /// Called with the built package bytes when the user applies their edits.
    pub on_apply: EventHandler<Vec<u8>>,
    pub on_cancel: EventHandler<()>,
}

#[component]
pub fn AemEditor(props: AemEditorProps) -> Element {
    let mut root = use_signal(|| props.root.0.clone());
    let mut selection = use_signal(AemSelectionState::new);
    let mut smart_state = use_signal(|| SmartState::Idle);
    let mut rejected_ids = use_signal(std::collections::HashSet::<usize>::new);
    let mut feedback_text = use_signal(String::new);
    let mut status_msg = use_signal(|| None::<(bool, String)>);

    let mut undo_stack = use_signal(Vec::<AemNode>::new);
    let mut redo_stack = use_signal(Vec::<AemNode>::new);

    let aem_config = use_signal(|| props.aem_config.0.clone());
    let connection = use_signal(|| props.connection.0.clone());
    let api_key = use_signal(|| props.api_key.clone());
    let model = use_signal(|| props.model.clone());
    let profile = use_signal(|| props.profile.clone());

    // Seed the per-language label overlay (uuid → lang → text) by matching each
    // node's master label against the source document's translations.
    let mut node_translations = use_signal(|| {
        let mut out: NodeTranslations = HashMap::new();
        let content_tr = props.content_translations.clone();
        for_each_labeled(&props.root.0, |uuid, label| {
            if let Some(map) = content_tr.get(label) {
                out.insert(uuid, map.clone());
            }
        });
        out
    });

    // Make per-language editing available to the deep text editor.
    use_context_provider(|| AemEditCtx {
        master_lang: props.master_lang.clone(),
        languages: props.languages.clone(),
        translations: node_translations,
    });

    let plain_images = props.plain_images.clone();
    let has_images = !plain_images.is_empty();
    let source_pdfs = props.source_pdfs.clone();
    let has_connection = connection.read().is_some();

    // ── Toolbar flags ─────────────────────────────────────────────────────
    let (can_move_up, can_move_down) = {
        let r = root.read();
        let sel = selection.read();
        if sel.selected.len() == 1 {
            let p = sel.selected.iter().next().unwrap();
            move_flags(&r, p)
        } else {
            (false, false)
        }
    };
    let (can_indent_flag, can_outdent_flag) = {
        let r = root.read();
        let sel = selection.read();
        if sel.selected.len() == 1 {
            let p = sel.selected.iter().next().unwrap();
            (can_indent(&r, p), can_outdent(p))
        } else {
            (false, false)
        }
    };
    let add_target: Option<(Vec<usize>, String)> = {
        let r = root.read();
        let sel = selection.read();
        if sel.selected.len() == 1 {
            let p = sel.selected.iter().next().unwrap().clone();
            match get_node(&r, &p) {
                Some(node) if is_container(node) => {
                    Some((p, node_short_label(node)))
                }
                _ => Some((vec![], "Form root".to_string())),
            }
        } else {
            Some((vec![], "Form root".to_string()))
        }
    };
    let selection_count = selection.read().count();
    let node_count = children_ref(&root.read()).map(|c| c.len()).unwrap_or(0);
    let smart_loading = matches!(*smart_state.read(), SmartState::Loading);

    // ── Action handler ─────────────────────────────────────────────────────
    let handle_action = move |action: AemEditorAction| {
        let mutates = describe_action(&action).is_some();
        if mutates {
            // Snapshot for undo.
            undo_stack.write().push(root.read().clone());
            redo_stack.write().clear();
        }

        match action {
            AemEditorAction::ToggleSelection(p) => selection.write().toggle(p),
            AemEditorAction::SelectSingle(p) => selection.write().select_single(p),
            AemEditorAction::ClearSelection => selection.write().clear(),
            AemEditorAction::SelectAll => {
                let paths = collect_paths(&root.read());
                selection.write().selected = paths;
            }
            AemEditorAction::StartEditing(p) => selection.write().start_editing(p),
            AemEditorAction::StartEditingMetadata(p) => selection.write().start_editing_metadata(p),
            AemEditorAction::StopEditing => selection.write().stop_editing(),
            AemEditorAction::DeleteSelected => {
                let sel = selection.read().selected.clone();
                delete_nodes(&mut root.write(), &sel);
                selection.write().clear();
            }
            AemEditorAction::DuplicateSelected => {
                let mut paths: Vec<_> = selection.read().selected.iter().cloned().collect();
                paths.sort();
                // Duplicate the single (or first) selected node.
                if let Some(p) = paths.first() {
                    duplicate_node(&mut root.write(), p);
                }
                selection.write().clear();
            }
            AemEditorAction::MoveUp | AemEditorAction::MoveDown => {
                let up = matches!(action, AemEditorAction::MoveUp);
                let path = selection.read().selected.iter().next().cloned();
                if let Some(p) = path
                    && let Some(np) = move_node(&mut root.write(), &p, up)
                {
                    selection.write().select_single(np);
                }
            }
            AemEditorAction::Indent => {
                let path = selection.read().selected.iter().next().cloned();
                if let Some(p) = path
                    && let Some(np) = indent_node(&mut root.write(), &p)
                {
                    selection.write().select_single(np);
                }
            }
            AemEditorAction::Outdent => {
                let path = selection.read().selected.iter().next().cloned();
                if let Some(p) = path
                    && let Some(np) = outdent_node(&mut root.write(), &p)
                {
                    selection.write().select_single(np);
                }
            }
            AemEditorAction::AddNode { parent, kind } => {
                if let Some(np) = add_node(&mut root.write(), &parent, kind) {
                    selection.write().select_single(np);
                }
            }
            AemEditorAction::ConvertSelected(target) => {
                let path = selection.read().selected.iter().next().cloned();
                if let Some(p) = path {
                    let converted = get_node(&root.read(), &p)
                        .and_then(|n| convert_node(n, target));
                    if let Some(new_node) = converted
                        && let Some(slot) = get_node_mut(&mut root.write(), &p)
                    {
                        *slot = new_node;
                    }
                }
            }
            AemEditorAction::UpdateText { path, content } => {
                if let Some(node) = get_node_mut(&mut root.write(), &path) {
                    set_editable_text(node, &content);
                }
            }
            AemEditorAction::UpdateTranslation {
                path,
                language,
                text,
            } => {
                let uuid = get_node(&root.read(), &path).and_then(node_uuid);
                if let Some(uuid) = uuid {
                    let mut tr = node_translations.write();
                    tr.entry(uuid).or_default().insert(language, text);
                }
            }
            AemEditorAction::UpdateMetadata { path, metadata } => {
                if let Some(node) = get_node_mut(&mut root.write(), &path) {
                    apply_metadata(node, &metadata);
                }
            }
            AemEditorAction::SmartAemEdit => {
                let current = root.read().clone();
                let images = plain_images.clone();
                let pdfs = source_pdfs.clone();
                let api_key = api_key.read().clone();
                let model = model.read().clone();
                let profile = profile.read().clone();
                let started = std::time::Instant::now();
                smart_state.set(SmartState::Loading);
                rejected_ids.write().clear();
                feedback_text.set(String::new());
                spawn(async move {
                    match smart_edit::run_smart_aem_edit(&current, &images, &pdfs, &api_key, &model, profile.as_deref()).await
                    {
                        Ok(result) => smart_state.set(SmartState::Preview {
                            result,
                            elapsed_ms: started.elapsed().as_millis(),
                        }),
                        Err(message) => smart_state.set(SmartState::Error { message }),
                    }
                });
            }
            AemEditorAction::UploadToAem => {
                let Some(conn) = connection.read().clone() else {
                    status_msg.set(Some((false, "No AEM connection configured.".into())));
                    return;
                };
                let cfg = aem_config.read().clone();
                let tree = root.read().clone();
                let node_tr = node_translations.read().clone();
                status_msg.set(Some((true, "Uploading to AEM…".into())));
                spawn(async move {
                    let zip = build_zip(&tree, &cfg, &node_tr);
                    match crate::aem_client::upload_and_install_package(
                        &conn, zip, &cfg.form_code,
                    )
                    .await
                    {
                        Ok(()) => status_msg.set(Some((true, "Uploaded and installed in AEM.".into()))),
                        Err(e) => status_msg.set(Some((false, e))),
                    }
                });
            }
        }
    };

    let on_apply = props.on_apply;
    let on_cancel = props.on_cancel;
    let root_title = root_title(&root.read());

    rsx! {
        div { class: "aem-editor",
            // Header
            div { class: "editor-header",
                h2 { "Edit AEM Structure — {root_title}" }
                div { class: "editor-header-actions",
                    button {
                        class: "editor-btn",
                        disabled: undo_stack.read().is_empty(),
                        onclick: move |_| {
                            if let Some(prev) = undo_stack.write().pop() {
                                redo_stack.write().push(root.read().clone());
                                root.set(prev);
                                selection.write().clear();
                            }
                        },
                        "Undo"
                    }
                    button {
                        class: "editor-btn",
                        disabled: redo_stack.read().is_empty(),
                        onclick: move |_| {
                            if let Some(next) = redo_stack.write().pop() {
                                undo_stack.write().push(root.read().clone());
                                root.set(next);
                                selection.write().clear();
                            }
                        },
                        "Redo"
                    }
                    button {
                        class: "editor-btn editor-btn-secondary",
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        class: "editor-btn editor-btn-primary",
                        onclick: move |_| {
                            let zip = build_zip(&root.read(), &aem_config.read(), &node_translations.read());
                            on_apply.call(zip);
                        },
                        "Apply"
                    }
                }
            }

            // Toolbar
            AemToolbar {
                selection_count,
                node_count,
                can_move_up,
                can_move_down,
                can_indent: can_indent_flag,
                can_outdent: can_outdent_flag,
                add_target,
                has_images,
                is_smart_edit_loading: smart_loading,
                has_connection,
                on_action: handle_action.clone(),
            }

            // Status line
            if let Some((ok, msg)) = status_msg.read().clone() {
                div { class: if ok { "aem-status aem-status-ok" } else { "aem-status aem-status-err" }, "{msg}" }
            }

            // Smart edit review panel
            {render_smart_panel(smart_state, rejected_ids, feedback_text, root, undo_stack, redo_stack, status_msg, selection, aem_config, connection, node_translations, api_key, model, profile, plain_images_signal(&props.plain_images), props.source_pdfs.clone())}

            // Node tree
            div { class: "aem-editor-tree",
                if let Some(children) = children_ref(&root.read()) {
                    for (i, child) in children.iter().enumerate() {
                        AemNodeItem {
                            key: "{i}",
                            node: AemNodeWrapper(child.clone()),
                            path: vec![i],
                            selection: selection.read().clone(),
                            depth: 0,
                            on_action: handle_action.clone(),
                        }
                    }
                }
            }
        }
    }
}

/// Helper to clone the plain images into an owned map (kept out of the rsx body).
fn plain_images_signal(images: &HashMap<String, String>) -> HashMap<String, String> {
    images.clone()
}

fn move_flags(root: &AemNode, path: &[usize]) -> (bool, bool) {
    if let Some((last, parent_path)) = path.split_last()
        && let Some(parent) = get_node(root, parent_path)
        && let Some(children) = children_ref(parent)
    {
        return (*last > 0, *last + 1 < children.len());
    }
    (false, false)
}

fn node_short_label(node: &AemNode) -> String {
    match node {
        AemNode::Root { title, .. } => title.clone(),
        AemNode::Panel { title, name, .. } | AemNode::Repeatable { title, name, .. } => {
            if title.trim().is_empty() {
                name.clone()
            } else {
                title.clone()
            }
        }
        _ => "node".to_string(),
    }
}

fn root_title(node: &AemNode) -> String {
    match node {
        AemNode::Root { title, .. } => title.clone(),
        _ => String::new(),
    }
}

/// Render the Smart AEM Edit review panel (Preview / Error / nothing).
#[allow(clippy::too_many_arguments)]
fn render_smart_panel(
    mut smart_state: Signal<SmartState>,
    mut rejected_ids: Signal<std::collections::HashSet<usize>>,
    mut feedback_text: Signal<String>,
    mut root: Signal<AemNode>,
    mut undo_stack: Signal<Vec<AemNode>>,
    mut redo_stack: Signal<Vec<AemNode>>,
    mut status_msg: Signal<Option<(bool, String)>>,
    mut selection: Signal<AemSelectionState>,
    aem_config: Signal<AemConfig>,
    connection: Signal<Option<AemConnection>>,
    node_translations: Signal<NodeTranslations>,
    api_key: Signal<String>,
    model: Signal<String>,
    profile: Signal<Option<String>>,
    plain_images: HashMap<String, String>,
    source_pdfs: Vec<(String, Vec<u8>)>,
) -> Element {
    match smart_state.read().clone() {
        SmartState::Idle | SmartState::Loading => rsx! {},
        SmartState::Error { message } => rsx! {
            div { class: "smart-edit-inline-panel",
                h3 { "Smart AEM Edit" }
                p { class: "smart-edit-hint smart-edit-warning", "Error: {message}" }
                div { class: "smart-edit-actions",
                    button {
                        class: "editor-btn editor-btn-secondary",
                        onclick: move |_| smart_state.set(SmartState::Idle),
                        "Dismiss"
                    }
                }
            }
        },
        SmartState::Preview { result, elapsed_ms } => {
            let result_for_apply = result.clone();
            let changes = result.changes.clone();
            let images = plain_images.clone();
            let pdfs = source_pdfs.clone();
            rsx! {
                div { class: "smart-edit-inline-panel",
                    h3 { "Smart AEM Edit Review" }
                    p { class: "smart-edit-hint", "Completed in {elapsed_ms}ms" }

                    if changes.is_empty() {
                        p { class: "smart-edit-hint smart-edit-warning",
                            "No structured change list was returned. Review and accept or dismiss."
                        }
                    } else {
                        div { class: "smart-edit-change-list",
                            for change in changes.clone() {
                                {
                                    let id = change.id;
                                    let is_rejected = rejected_ids.read().contains(&id);
                                    rsx! {
                                        label { class: if is_rejected { "smart-edit-change-item smart-edit-change-rejected" } else { "smart-edit-change-item" },
                                            input {
                                                r#type: "checkbox",
                                                checked: !is_rejected,
                                                onchange: move |evt| {
                                                    if evt.checked() { rejected_ids.write().remove(&id); }
                                                    else { rejected_ids.write().insert(id); }
                                                },
                                            }
                                            span { "{change.description}" }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "smart-edit-feedback",
                        textarea {
                            class: "smart-edit-feedback-input",
                            rows: "2",
                            placeholder: "Optional feedback, then retry…",
                            value: "{feedback_text}",
                            oninput: move |evt| feedback_text.set(evt.value()),
                        }
                    }

                    div { class: "smart-edit-actions",
                        button {
                            class: "editor-btn editor-btn-secondary",
                            onclick: move |_| {
                                feedback_text.set(String::new());
                                rejected_ids.write().clear();
                                smart_state.set(SmartState::Idle);
                            },
                            "Dismiss"
                        }

                        // Retry with feedback
                        if !rejected_ids.read().is_empty() || !feedback_text.read().trim().is_empty() {
                            {
                                let rejected: Vec<_> = changes.iter().filter(|c| rejected_ids.read().contains(&c.id)).cloned().collect();
                                let accepted: Vec<_> = changes.iter().filter(|c| !rejected_ids.read().contains(&c.id)).cloned().collect();
                                let images = images.clone();
                                let pdfs = pdfs.clone();
                                rsx! {
                                    button {
                                        class: "editor-btn editor-btn-secondary",
                                        onclick: move |_| {
                                            let current = root.read().clone();
                                            let images = images.clone();
                                            let pdfs = pdfs.clone();
                                            let accepted = accepted.clone();
                                            let rejected = rejected.clone();
                                            let user_feedback = feedback_text.read().clone();
                                            let api_key = api_key.read().clone();
                                            let model = model.read().clone();
                                            let profile = profile.read().clone();
                                            let started = std::time::Instant::now();
                                            smart_state.set(SmartState::Loading);
                                            rejected_ids.write().clear();
                                            feedback_text.set(String::new());
                                            spawn(async move {
                                                match smart_edit::run_smart_aem_edit_with_feedback(
                                                    &current, &images, &pdfs, &accepted, &rejected, &user_feedback, &api_key, &model, profile.as_deref(),
                                                ).await {
                                                    Ok(result) => smart_state.set(SmartState::Preview { result, elapsed_ms: started.elapsed().as_millis() }),
                                                    Err(message) => smart_state.set(SmartState::Error { message }),
                                                }
                                            });
                                        },
                                        "Retry with Feedback"
                                    }
                                }
                            }
                        }

                        // Apply changes (only when nothing rejected)
                        if rejected_ids.read().is_empty() {
                            button {
                                class: "editor-btn editor-btn-primary",
                                onclick: move |_| {
                                    undo_stack.write().push(root.read().clone());
                                    redo_stack.write().clear();
                                    root.set(result_for_apply.root.clone());
                                    selection.write().clear();
                                    feedback_text.set(String::new());
                                    smart_state.set(SmartState::Idle);

                                    // Auto-upload the applied tree for live review, if connected.
                                    if let Some(conn) = connection.read().clone() {
                                        let cfg = aem_config.read().clone();
                                        let tree = root.read().clone();
                                        let node_tr = node_translations.read().clone();
                                        status_msg.set(Some((true, "Uploading applied changes to AEM…".into())));
                                        spawn(async move {
                                            let zip = build_zip(&tree, &cfg, &node_tr);
                                            match crate::aem_client::upload_and_install_package(&conn, zip, &cfg.form_code).await {
                                                Ok(()) => status_msg.set(Some((true, "Uploaded and installed in AEM.".into()))),
                                                Err(e) => status_msg.set(Some((false, e))),
                                            }
                                        });
                                    }
                                },
                                "Apply Changes"
                            }
                        }
                    }
                }
            }
        }
    }
}
