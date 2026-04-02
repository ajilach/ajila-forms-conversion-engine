//! Main structured document editor component.
//!
//! This is the top-level component that orchestrates the editor UI.

use dioxus::prelude::*;
use std::collections::BTreeSet;

use blueprint::{DocumentEnvelope, StructuredNode, InlineText, InlineNode, HeadingLevel, HeadingNode, ParagraphNode, ListNode, GroupNode};
use blueprint::document::ListStyleType;

use super::node_renderer::{NodeRenderer, NodesWrapper};
use super::state::{
    can_merge_selected, delete_nodes, get_node_at_path_mut, EditorAction, NewNodeType,
    SelectionState,
};
use super::toolbar::EditorToolbar;

/// Wrapper for DocumentEnvelope that implements PartialEq (always eq for memoization skip).
#[derive(Clone)]
pub struct EnvelopeWrapper(pub DocumentEnvelope);

impl PartialEq for EnvelopeWrapper {
    fn eq(&self, _other: &Self) -> bool {
        // Always return false to force re-render when envelope changes
        false
    }
}

/// Properties for the structured editor.
#[derive(Clone, PartialEq, Props)]
pub struct StructuredEditorProps {
    /// The document envelope to edit.
    pub envelope: EnvelopeWrapper,
    /// Callback when editing is complete (with the modified envelope).
    pub on_apply: EventHandler<DocumentEnvelope>,
    /// Callback when editing is cancelled.
    pub on_cancel: EventHandler<()>,
}

/// Main structured document editor component.
#[component]
pub fn StructuredEditor(props: StructuredEditorProps) -> Element {
    // Working copy of the envelope
    let mut envelope = use_signal(|| props.envelope.0.clone());

    // Selection state
    let mut selection = use_signal(SelectionState::new);

    // Collect all languages from the document
    let languages: Vec<String> = {
        let env = envelope.read();
        let mut langs = BTreeSet::new();

        // Get language from context
        let ctx_lang = env.context.language();
        if !ctx_lang.is_empty() {
            // Context language might be comma-separated for merged docs
            for lang in ctx_lang.split(',') {
                let trimmed = lang.trim();
                if !trimmed.is_empty() {
                    langs.insert(trimmed.to_string());
                }
            }
        }

        // Collect languages from content
        fn collect_from_nodes(nodes: &[StructuredNode], langs: &mut BTreeSet<String>) {
            for node in nodes {
                match node {
                    StructuredNode::Paragraph(p) => p.content.collect_languages(langs),
                    StructuredNode::Heading(h) => h.content.collect_languages(langs),
                    StructuredNode::Field(f) => {
                        if let Some(label) = &f.label {
                            label.collect_languages(langs);
                        }
                    }
                    StructuredNode::List(l) => {
                        for item in &l.items {
                            item.collect_languages(langs);
                        }
                    }
                    StructuredNode::Group(g) => collect_from_nodes(&g.children, langs),
                    StructuredNode::Table(t) => {
                        if let Some(header) = &t.header {
                            collect_from_nodes(&header.cells, langs);
                        }
                        for row in &t.rows {
                            collect_from_nodes(&row.cells, langs);
                        }
                    }
                    _ => {}
                }
            }
        }

        collect_from_nodes(&env.content, &mut langs);
        langs.into_iter().collect()
    };

    // Check if current selection can be merged
    let can_merge = {
        let env = envelope.read();
        let sel = selection.read();
        can_merge_selected(&env.content, &sel.selected).is_ok()
    };

    // Check if selected node can be moved up/down
    let (can_move_up, can_move_down) = {
        let env = envelope.read();
        let sel = selection.read();
        if sel.selected.len() == 1 {
            let path = sel.selected.iter().next().unwrap();
            // Only support root-level moves for now
            if path.len() == 1 {
                let idx = path[0];
                let can_up = idx > 0;
                let can_down = idx + 1 < env.content.len();
                (can_up, can_down)
            } else {
                (false, false)
            }
        } else {
            (false, false)
        }
    };

    // Handle editor actions
    let handle_action = move |action: EditorAction| {
        match action {
            EditorAction::ToggleSelection(path) => {
                selection.write().toggle(path);
            }
            EditorAction::SelectSingle(path) => {
                selection.write().select_single(path);
            }
            EditorAction::ClearSelection => {
                selection.write().clear();
            }
            EditorAction::StartEditing(path) => {
                selection.write().start_editing(path);
            }
            EditorAction::StopEditing => {
                selection.write().stop_editing();
            }
            EditorAction::DeleteSelected => {
                let paths = selection.read().selected.clone();
                envelope.write().content = {
                    let mut content = envelope.read().content.clone();
                    delete_nodes(&mut content, &paths);
                    content
                };
                selection.write().clear();
            }
            EditorAction::MergeSelected => {
                // Get selected paths sorted by position
                let mut paths: Vec<_> = selection.read().selected.iter().cloned().collect();
                paths.sort();

                if paths.len() >= 2 {
                    // For now, only support merging at root level
                    if paths.iter().all(|p| p.len() == 1) {
                        let indices: Vec<usize> = paths.iter().map(|p| p[0]).collect();
                        let mut env = envelope.write();

                        // Collect nodes to merge
                        let nodes: Vec<StructuredNode> = indices
                            .iter()
                            .filter_map(|&i| env.content.get(i).cloned())
                            .collect();

                        // Try to merge
                        if let Ok(merged) = blueprint::merge_nodes(nodes) {
                            // Remove old nodes (in reverse order to maintain indices)
                            for &idx in indices.iter().rev() {
                                if idx < env.content.len() {
                                    env.content.remove(idx);
                                }
                            }
                            // Insert merged node at first position
                            let insert_idx = indices[0].min(env.content.len());
                            env.content.insert(insert_idx, merged);
                        }
                    }

                    selection.write().clear();
                }
            }
            EditorAction::MoveUp => {
                let sel = selection.read();
                if sel.selected.len() == 1 {
                    let path = sel.selected.iter().next().unwrap().clone();
                    drop(sel);
                    
                    if path.len() == 1 && path[0] > 0 {
                        // Root level: swap with previous
                        let idx = path[0];
                        let mut env = envelope.write();
                        env.content.swap(idx, idx - 1);
                        drop(env);
                        // Update selection to follow the moved node
                        selection.write().select_single(vec![idx - 1]);
                    }
                }
            }
            EditorAction::MoveDown => {
                let sel = selection.read();
                if sel.selected.len() == 1 {
                    let path = sel.selected.iter().next().unwrap().clone();
                    drop(sel);
                    
                    let env_len = envelope.read().content.len();
                    if path.len() == 1 && path[0] + 1 < env_len {
                        // Root level: swap with next
                        let idx = path[0];
                        let mut env = envelope.write();
                        env.content.swap(idx, idx + 1);
                        drop(env);
                        // Update selection to follow the moved node
                        selection.write().select_single(vec![idx + 1]);
                    }
                }
            }
            EditorAction::UpdateText { path, content, language } => {
                let mut env = envelope.write();
                if let Some(node) = get_node_at_path_mut(&mut env.content, &path) {
                    match node {
                        StructuredNode::Paragraph(p) => {
                            update_inline_text(&mut p.content, &content, language.as_deref());
                        }
                        StructuredNode::Heading(h) => {
                            update_inline_text(&mut h.content, &content, language.as_deref());
                        }
                        _ => {}
                    }
                }
            }
            EditorAction::AddNode { parent, index, node_type } => {
                let new_node = match node_type {
                    NewNodeType::Paragraph => StructuredNode::Paragraph(ParagraphNode {
                        content: InlineText::plain("New paragraph"),
                        som_path: None,
                        source_name: None,
                    }),
                    NewNodeType::Heading(level) => StructuredNode::Heading(HeadingNode {
                        level: HeadingLevel::from_u8(level),
                        content: InlineText::plain("New heading"),
                        som_path: None,
                        source_name: None,
                    }),
                    NewNodeType::List => StructuredNode::List(ListNode {
                        list_style: ListStyleType::Disc,
                        items: vec![InlineText::plain("New item")],
                    }),
                    NewNodeType::Group => StructuredNode::Group(GroupNode {
                        children: vec![],
                    }),
                };

                let mut env = envelope.write();
                if parent.is_empty() {
                    // Add to root
                    let insert_idx = index.min(env.content.len());
                    env.content.insert(insert_idx, new_node);
                } else {
                    // TODO: Add to nested parent
                }
            }
        }
    };

    let on_apply = props.on_apply.clone();
    let on_cancel = props.on_cancel.clone();

    rsx! {
        div { class: "structured-editor",
            // Header
            div { class: "editor-header",
                h2 { "Edit Structure" }
                div { class: "editor-header-actions",
                    button {
                        class: "editor-btn editor-btn-secondary",
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        class: "editor-btn editor-btn-primary",
                        onclick: {
                            let envelope = envelope.clone();
                            move |_| on_apply.call(envelope.read().clone())
                        },
                        "Apply Changes"
                    }
                }
            }

            // Toolbar
            EditorToolbar {
                selection: selection.read().clone(),
                can_merge: can_merge,
                can_move_up: can_move_up,
                can_move_down: can_move_down,
                on_action: handle_action,
            }

            // Document tree
            div { class: "editor-content",
                NodeRenderer {
                    nodes: NodesWrapper(envelope.read().content.clone()),
                    selection: selection.read().clone(),
                    languages: languages.clone(),
                    on_action: handle_action,
                }
            }

            // Status bar
            div { class: "editor-status",
                span {
                    "{envelope.read().content.len()} nodes"
                }
                if !languages.is_empty() {
                    span { class: "editor-status-sep", " • " }
                    span {
                        "Languages: {languages.join(\", \")}"
                    }
                }
            }
        }
    }
}

/// Update inline text content, optionally for a specific language.
fn update_inline_text(text: &mut InlineText, content: &str, language: Option<&str>) {
    if let Some(lang) = language {
        // Update for specific language
        // Find and update TranslatedText nodes, or convert Text to TranslatedText
        let mut new_nodes = Vec::new();
        let mut found = false;

        for node in text.0.iter() {
            match node {
                InlineNode::TranslatedText(map) => {
                    let mut new_map = map.clone();
                    new_map.insert(lang.to_string(), content.to_string());
                    new_nodes.push(InlineNode::TranslatedText(new_map));
                    found = true;
                }
                other => new_nodes.push(other.clone()),
            }
        }

        if !found && !text.0.is_empty() {
            // Convert first text node to translated
            new_nodes.clear();
            for (i, node) in text.0.iter().enumerate() {
                if i == 0 {
                    let mut map = std::collections::HashMap::new();
                    if let InlineNode::Text(existing) = node {
                        map.insert("default".to_string(), existing.clone());
                    }
                    map.insert(lang.to_string(), content.to_string());
                    new_nodes.push(InlineNode::TranslatedText(map));
                } else {
                    new_nodes.push(node.clone());
                }
            }
        }

        text.0 = new_nodes;
    } else {
        // Replace all content with plain text
        text.0 = vec![InlineNode::Text(content.to_string())];
    }
}
