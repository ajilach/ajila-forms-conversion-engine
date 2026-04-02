//! Main structured document editor component.
//!
//! This is the top-level component that orchestrates the editor UI.

use dioxus::prelude::*;
use std::collections::{BTreeSet, HashMap};

use blueprint::document::ListStyleType;
use blueprint::{
    DocumentEnvelope, FieldId, GroupNode, HeadingLevel, HeadingNode, InlineNode, InlineText,
    ListNode, ParagraphNode, StructuredNode,
};

use super::node_renderer::{FieldLabelsWrapper, NodeRenderer, NodesWrapper};
use super::state::{
    ConvertTarget, EditorAction, NewNodeType, NodeMetadata, SelectionState, available_conversions,
    can_merge_selected, delete_nodes, get_node_at_path_mut,
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
                    StructuredNode::Repeatable(r) => {
                        collect_from_nodes(&[(*r.item).clone()], langs);
                    }
                    StructuredNode::Conditional(c) => {
                        collect_from_nodes(&[(*c.content).clone()], langs);
                    }
                    StructuredNode::GridLayout(g) => {
                        for elem in &g.elements {
                            collect_from_nodes(&[elem.node.clone()], langs);
                        }
                    }
                    _ => {}
                }
            }
        }

        collect_from_nodes(&env.content, &mut langs);
        langs.into_iter().collect()
    };

    // Collect field labels for display in conditionals
    let field_labels = {
        let env = envelope.read();
        let mut labels = HashMap::new();

        fn collect_field_labels(nodes: &[StructuredNode], labels: &mut HashMap<FieldId, String>) {
            for node in nodes {
                match node {
                    StructuredNode::Field(f) => {
                        if let Some(label) = &f.label {
                            let label_text = label.as_plain_text();
                            if !label_text.is_empty() {
                                labels.insert(f.name.clone(), label_text);
                            }
                        }
                    }
                    StructuredNode::Group(g) => collect_field_labels(&g.children, labels),
                    StructuredNode::Table(t) => {
                        if let Some(header) = &t.header {
                            collect_field_labels(&header.cells, labels);
                        }
                        for row in &t.rows {
                            collect_field_labels(&row.cells, labels);
                        }
                    }
                    StructuredNode::Repeatable(r) => {
                        collect_field_labels(&[(*r.item).clone()], labels);
                    }
                    StructuredNode::Conditional(c) => {
                        collect_field_labels(&[(*c.content).clone()], labels);
                    }
                    StructuredNode::GridLayout(g) => {
                        for elem in &g.elements {
                            collect_field_labels(&[elem.node.clone()], labels);
                        }
                    }
                    _ => {}
                }
            }
        }

        collect_field_labels(&env.content, &mut labels);
        FieldLabelsWrapper(labels)
    };

    // Check if current selection can be merged
    let can_merge = {
        let env = envelope.read();
        let sel = selection.read();
        can_merge_selected(&env.content, &sel.selected).is_ok()
    };

    // Get available conversions for current selection
    let conversions = {
        let env = envelope.read();
        let sel = selection.read();
        available_conversions(&env.content, &sel.selected)
    };

    // Check if selected nodes can be moved up/down
    let (can_move_up, can_move_down) = {
        let env = envelope.read();
        let sel = selection.read();
        if !sel.selected.is_empty() {
            // Get all root-level selected indices
            let root_indices: Vec<usize> = sel
                .selected
                .iter()
                .filter(|p| p.len() == 1)
                .map(|p| p[0])
                .collect();

            if root_indices.is_empty() {
                (false, false)
            } else {
                let min_idx = *root_indices.iter().min().unwrap();
                let max_idx = *root_indices.iter().max().unwrap();
                let can_up = min_idx > 0;
                let can_down = max_idx + 1 < env.content.len();
                (can_up, can_down)
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
            EditorAction::StartEditingListItem(path, index) => {
                selection.write().start_editing_list_item(path, index);
            }
            EditorAction::StartEditingMetadata(path) => {
                selection.write().start_editing_metadata(path);
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
                // Get all root-level selected indices, sorted
                let mut root_indices: Vec<usize> = sel
                    .selected
                    .iter()
                    .filter(|p| p.len() == 1)
                    .map(|p| p[0])
                    .collect();
                root_indices.sort();
                drop(sel);

                if !root_indices.is_empty() && root_indices[0] > 0 {
                    let mut env = envelope.write();
                    // Move from top to bottom to avoid index shifting issues
                    for &idx in &root_indices {
                        if idx > 0 {
                            env.content.swap(idx, idx - 1);
                        }
                    }
                    drop(env);
                    // Update selection to follow moved nodes
                    let mut new_selection = selection.write();
                    new_selection.selected.clear();
                    for idx in root_indices {
                        new_selection.selected.insert(vec![idx - 1]);
                    }
                }
            }
            EditorAction::MoveDown => {
                let sel = selection.read();
                // Get all root-level selected indices, sorted in reverse
                let mut root_indices: Vec<usize> = sel
                    .selected
                    .iter()
                    .filter(|p| p.len() == 1)
                    .map(|p| p[0])
                    .collect();
                root_indices.sort();
                root_indices.reverse();
                drop(sel);

                let env_len = envelope.read().content.len();
                if !root_indices.is_empty() && root_indices[0] + 1 < env_len {
                    let mut env = envelope.write();
                    // Move from bottom to top to avoid index shifting issues
                    for &idx in &root_indices {
                        if idx + 1 < env_len {
                            env.content.swap(idx, idx + 1);
                        }
                    }
                    drop(env);
                    // Update selection to follow moved nodes
                    let mut new_selection = selection.write();
                    new_selection.selected.clear();
                    for idx in root_indices {
                        new_selection.selected.insert(vec![idx + 1]);
                    }
                }
            }
            EditorAction::UpdateText {
                path,
                content,
                language,
            } => {
                let mut env = envelope.write();
                if let Some(node) = get_node_at_path_mut(&mut env.content, &path) {
                    match node {
                        StructuredNode::Paragraph(p) => {
                            update_inline_text(&mut p.content, &content, language.as_deref());
                        }
                        StructuredNode::Heading(h) => {
                            update_inline_text(&mut h.content, &content, language.as_deref());
                        }
                        StructuredNode::Field(f) => {
                            if let Some(label) = &mut f.label {
                                update_inline_text(label, &content, language.as_deref());
                            }
                        }
                        // Handle list item updates that come through UpdateText
                        // (from the path encoding used in list item editing)
                        StructuredNode::List(_) => {
                            // This case is handled by UpdateListItem, but we need to check
                            // if the path has an extra element for the item index
                        }
                        _ => {}
                    }
                }
            }
            EditorAction::UpdateListItem {
                path,
                item_index,
                content,
                language,
            } => {
                let mut env = envelope.write();
                if let Some(node) = get_node_at_path_mut(&mut env.content, &path) {
                    if let StructuredNode::List(l) = node {
                        if item_index < l.items.len() {
                            update_inline_text(
                                &mut l.items[item_index],
                                &content,
                                language.as_deref(),
                            );
                        }
                    }
                }
            }
            EditorAction::UpdateMetadata { path, metadata } => {
                let mut env = envelope.write();
                if let Some(node) = get_node_at_path_mut(&mut env.content, &path) {
                    match metadata {
                        NodeMetadata::HeadingLevel(level) => {
                            if let StructuredNode::Heading(h) = node {
                                h.level = HeadingLevel::from_u8(level);
                            }
                        }
                        NodeMetadata::Repeatable { min, max } => {
                            if let StructuredNode::Repeatable(r) = node {
                                r.min_occurrences = min;
                                r.max_occurrences = max;
                            }
                        }
                        NodeMetadata::GridColumns(cols) => {
                            if let StructuredNode::GridLayout(g) = node {
                                g.columns = cols;
                            }
                        }
                        NodeMetadata::GridElementSpan(span) => {
                            // For grid element span, we need parent context
                            // This is more complex and would need different handling
                            let _ = span;
                        }
                    }
                }
            }
            EditorAction::AddNode {
                parent,
                index,
                node_type,
            } => {
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
                    NewNodeType::Group => StructuredNode::Group(GroupNode { children: vec![] }),
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
            EditorAction::ConvertSelected(target) => {
                // Get selected paths sorted by position
                let mut paths: Vec<_> = selection.read().selected.iter().cloned().collect();
                paths.sort();

                // Only support root-level conversions for now
                if paths.iter().all(|p| p.len() == 1) {
                    let indices: Vec<usize> = paths.iter().map(|p| p[0]).collect();
                    let env_read = envelope.read();

                    // Collect nodes to convert
                    let nodes: Vec<&StructuredNode> = indices
                        .iter()
                        .filter_map(|&i| env_read.content.get(i))
                        .collect();

                    if !nodes.is_empty() {
                        let converted_nodes = convert_nodes(&nodes, target);
                        drop(env_read);

                        if !converted_nodes.is_empty() {
                            let mut env = envelope.write();
                            // Remove old nodes (in reverse order to maintain indices)
                            for &idx in indices.iter().rev() {
                                if idx < env.content.len() {
                                    env.content.remove(idx);
                                }
                            }
                            // Insert converted nodes at first position
                            let insert_idx = indices[0].min(env.content.len());
                            for (i, node) in converted_nodes.into_iter().enumerate() {
                                env.content.insert(insert_idx + i, node);
                            }
                        }
                    }
                }

                selection.write().clear();
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
                can_merge,
                can_move_up,
                can_move_down,
                available_conversions: conversions.clone(),
                on_action: handle_action,
            }

            // Document tree
            div { class: "editor-content",
                NodeRenderer {
                    nodes: NodesWrapper(envelope.read().content.clone()),
                    selection: selection.read().clone(),
                    languages: languages.clone(),
                    field_labels: field_labels.clone(),
                    on_action: handle_action,
                }
            }

            // Status bar
            div { class: "editor-status",
                span { "{envelope.read().content.len()} nodes" }
                if !languages.is_empty() {
                    span { class: "editor-status-sep", " • " }
                    span { "Languages: {languages.join(\", \")}" }
                }
            }
        }
    }
}

/// Update inline text content, optionally for a specific language.
fn update_inline_text(text: &mut InlineText, content: &str, language: Option<&str>) {
    if let Some(lang) = language {
        // Update for specific language
        // For multilingual content, we need to merge all existing content into a single
        // TranslatedText node, then update the specific language.

        // First, collect all existing translations
        let mut translations = std::collections::HashMap::new();

        for node in text.0.iter() {
            match node {
                InlineNode::TranslatedText(map) => {
                    for (k, v) in map {
                        translations.insert(k.clone(), v.clone());
                    }
                }
                InlineNode::Text(t) => {
                    // If there's a plain text node, treat it as the default language
                    if !translations.contains_key("default") {
                        translations.insert("default".to_string(), t.clone());
                    }
                }
                _ => {
                    // Skip formatting nodes for now - simplified editing
                }
            }
        }

        // Update the specified language
        translations.insert(lang.to_string(), content.to_string());

        // Create a single TranslatedText node with all translations
        text.0 = vec![InlineNode::TranslatedText(translations)];
    } else {
        // Replace all content with plain text
        text.0 = vec![InlineNode::Text(content.to_string())];
    }
}

/// Convert selected nodes to a target type.
///
/// Returns the resulting nodes (may be fewer or more than input).
fn convert_nodes(nodes: &[&StructuredNode], target: ConvertTarget) -> Vec<StructuredNode> {
    match target {
        ConvertTarget::Paragraph => {
            // Converting single element to paragraph
            nodes
                .iter()
                .map(|n| match n {
                    StructuredNode::Heading(h) => {
                        // Heading -> Paragraph: preserve text content
                        StructuredNode::Paragraph(ParagraphNode {
                            content: h.content.clone(),
                            som_path: h.som_path.clone(),
                            source_name: h.source_name.clone(),
                        })
                    }
                    _ => (*n).clone(), // Keep unchanged
                })
                .collect()
        }
        ConvertTarget::Paragraphs => {
            // Explode list items to multiple paragraphs
            nodes
                .iter()
                .flat_map(|n| match n {
                    StructuredNode::List(l) => {
                        // List -> Multiple paragraphs: each item becomes a paragraph
                        l.items
                            .iter()
                            .map(|item| {
                                StructuredNode::Paragraph(ParagraphNode {
                                    content: item.clone(),
                                    som_path: None,
                                    source_name: None,
                                })
                            })
                            .collect()
                    }
                    _ => vec![(*n).clone()], // Keep unchanged
                })
                .collect()
        }
        ConvertTarget::Heading(level) => {
            // Converting to heading
            nodes
                .iter()
                .map(|n| match n {
                    StructuredNode::Paragraph(p) => {
                        // Paragraph -> Heading
                        StructuredNode::Heading(HeadingNode {
                            level: HeadingLevel::from_u8(level),
                            content: p.content.clone(),
                            som_path: p.som_path.clone(),
                            source_name: p.source_name.clone(),
                        })
                    }
                    _ => (*n).clone(), // Keep unchanged
                })
                .collect()
        }
        ConvertTarget::List => {
            // Converting multiple items to a single list
            let items: Vec<InlineText> = nodes
                .iter()
                .filter_map(|n| match n {
                    StructuredNode::Paragraph(p) => Some(p.content.clone()),
                    StructuredNode::Heading(h) => Some(h.content.clone()),
                    _ => None,
                })
                .collect();

            if items.is_empty() {
                vec![]
            } else {
                vec![StructuredNode::List(ListNode {
                    list_style: ListStyleType::Disc,
                    items,
                })]
            }
        }
    }
}
