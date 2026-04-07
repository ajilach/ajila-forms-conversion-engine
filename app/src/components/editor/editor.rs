//! Main structured document editor component.
//!
//! This is the top-level component that orchestrates the editor UI.

use dioxus::prelude::*;
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

use blueprint::document::ListStyleType;
use blueprint::{
    DocumentEnvelope, FieldId, FieldNode, FieldType, GroupNode, HeadingLevel, HeadingNode,
    InlineText, ListNode, ParagraphNode, StructuredNode,
};

use super::node_renderer::{FieldLabelsWrapper, NodeRenderer, NodesWrapper};
use super::state::{
    ConvertTarget, EditorAction, FieldInputKind, NewNodeType, NodeMetadata, PathSegment,
    SelectionState, available_conversions, can_merge_selected, delete_nodes,
    get_container_child_info, get_container_children_count, get_list_item_text_mut,
    get_node_at_path_mut, is_container_child_path, is_list_item_path, is_table_row_path,
    move_container_child_down, move_container_child_up, move_list_item_down, move_list_item_up,
    move_table_row_down, move_table_row_up,
};
use super::toolbar::EditorToolbar;
use crate::markdown::{markdown_to_inline_text, markdown_to_inline_text_multilingual};

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
                            collect_from_nodes(std::slice::from_ref(&elem.node), langs);
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
                            collect_field_labels(std::slice::from_ref(&elem.node), labels);
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
        if sel.selected.len() == 1 {
            let path = sel.selected.iter().next().unwrap();

            // Check for list item movement
            if is_list_item_path(path) {
                if let Some(PathSegment::ListItem(idx)) = path.last() {
                    let parent_path: Vec<_> = path[..path.len() - 1].to_vec();
                    if let Some(StructuredNode::List(l)) =
                        super::state::get_node_at_path(&env.content, &parent_path)
                    {
                        let can_up = *idx > 0;
                        let can_down = *idx + 1 < l.items.len();
                        (can_up, can_down)
                    } else {
                        (false, false)
                    }
                } else {
                    (false, false)
                }
            }
            // Check for table row movement
            else if is_table_row_path(path) {
                if let Some(PathSegment::TableRow(idx)) = path.last() {
                    let parent_path: Vec<_> = path[..path.len() - 1].to_vec();
                    if let Some(StructuredNode::Table(t)) =
                        super::state::get_node_at_path(&env.content, &parent_path)
                    {
                        let can_up = *idx > 0;
                        let can_down = *idx + 1 < t.rows.len();
                        (can_up, can_down)
                    } else {
                        (false, false)
                    }
                } else {
                    (false, false)
                }
            }
            // Check for root-level node movement
            else if path.len() == 1 {
                if let Some(PathSegment::Child(idx)) = path.first() {
                    let can_up = *idx > 0;
                    let can_down = *idx + 1 < env.content.len();
                    (can_up, can_down)
                } else {
                    (false, false)
                }
            }
            // Check for container child movement (Group, GridLayout)
            else if is_container_child_path(path) {
                if let Some((parent_path, child_idx)) = get_container_child_info(path) {
                    if let Some(children_count) =
                        get_container_children_count(&env.content, &parent_path)
                    {
                        let can_up = child_idx > 0;
                        let can_down = child_idx + 1 < children_count;
                        (can_up, can_down)
                    } else {
                        (false, false)
                    }
                } else {
                    (false, false)
                }
            } else {
                (false, false)
            }
        } else if !sel.selected.is_empty() {
            // Multiple selection: only support root-level
            let root_indices: Vec<usize> = sel
                .selected
                .iter()
                .filter_map(|p| {
                    if p.len() == 1 {
                        p.first().and_then(|s| s.as_child_index())
                    } else {
                        None
                    }
                })
                .collect();

            if root_indices.is_empty() || root_indices.len() != sel.selected.len() {
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
                    // For now, only support merging at root level (single Child segment)
                    if paths
                        .iter()
                        .all(|p| p.len() == 1 && matches!(p.first(), Some(PathSegment::Child(_))))
                    {
                        let indices: Vec<usize> = paths
                            .iter()
                            .filter_map(|p| p.first().and_then(|s| s.as_child_index()))
                            .collect();
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
                let paths: Vec<_> = sel.selected.iter().cloned().collect();
                drop(sel);

                if paths.len() == 1 {
                    let path = &paths[0];

                    // Handle list item movement
                    if is_list_item_path(path) {
                        let mut env = envelope.write();
                        if let Some(new_path) = move_list_item_up(&mut env.content, path) {
                            drop(env);
                            let mut new_selection = selection.write();
                            new_selection.selected.clear();
                            new_selection.selected.insert(new_path);
                        }
                    }
                    // Handle table row movement
                    else if is_table_row_path(path) {
                        let mut env = envelope.write();
                        if let Some(new_path) = move_table_row_up(&mut env.content, path) {
                            drop(env);
                            let mut new_selection = selection.write();
                            new_selection.selected.clear();
                            new_selection.selected.insert(new_path);
                        }
                    }
                    // Handle root-level node movement
                    else if path.len() == 1 {
                        if let Some(PathSegment::Child(idx)) = path.first() {
                            if *idx > 0 {
                                let mut env = envelope.write();
                                env.content.swap(*idx, idx - 1);
                                drop(env);
                                let mut new_selection = selection.write();
                                new_selection.selected.clear();
                                new_selection
                                    .selected
                                    .insert(vec![PathSegment::Child(idx - 1)]);
                            }
                        }
                    }
                    // Handle container child movement (Group, GridLayout)
                    else if is_container_child_path(path) {
                        let mut env = envelope.write();
                        if let Some(new_path) = move_container_child_up(&mut env.content, path) {
                            drop(env);
                            let mut new_selection = selection.write();
                            new_selection.selected.clear();
                            new_selection.selected.insert(new_path);
                        }
                    }
                } else if !paths.is_empty() {
                    // Multiple selection: only support root-level
                    let mut root_indices: Vec<usize> = paths
                        .iter()
                        .filter_map(|p| {
                            if p.len() == 1 {
                                p.first().and_then(|s| s.as_child_index())
                            } else {
                                None
                            }
                        })
                        .collect();

                    if root_indices.len() == paths.len() {
                        root_indices.sort();
                        if root_indices[0] > 0 {
                            let mut env = envelope.write();
                            for &idx in &root_indices {
                                if idx > 0 {
                                    env.content.swap(idx, idx - 1);
                                }
                            }
                            drop(env);
                            let mut new_selection = selection.write();
                            new_selection.selected.clear();
                            for idx in root_indices {
                                new_selection
                                    .selected
                                    .insert(vec![PathSegment::Child(idx - 1)]);
                            }
                        }
                    }
                }
            }
            EditorAction::MoveDown => {
                let sel = selection.read();
                let paths: Vec<_> = sel.selected.iter().cloned().collect();
                drop(sel);

                let env_len = envelope.read().content.len();

                if paths.len() == 1 {
                    let path = &paths[0];

                    // Handle list item movement
                    if is_list_item_path(path) {
                        let mut env = envelope.write();
                        if let Some(new_path) = move_list_item_down(&mut env.content, path) {
                            drop(env);
                            let mut new_selection = selection.write();
                            new_selection.selected.clear();
                            new_selection.selected.insert(new_path);
                        }
                    }
                    // Handle table row movement
                    else if is_table_row_path(path) {
                        let mut env = envelope.write();
                        if let Some(new_path) = move_table_row_down(&mut env.content, path) {
                            drop(env);
                            let mut new_selection = selection.write();
                            new_selection.selected.clear();
                            new_selection.selected.insert(new_path);
                        }
                    }
                    // Handle root-level node movement
                    else if path.len() == 1 {
                        if let Some(PathSegment::Child(idx)) = path.first() {
                            if *idx + 1 < env_len {
                                let mut env = envelope.write();
                                env.content.swap(*idx, idx + 1);
                                drop(env);
                                let mut new_selection = selection.write();
                                new_selection.selected.clear();
                                new_selection
                                    .selected
                                    .insert(vec![PathSegment::Child(idx + 1)]);
                            }
                        }
                    }
                    // Handle container child movement (Group, GridLayout)
                    else if is_container_child_path(path) {
                        let mut env = envelope.write();
                        if let Some(new_path) = move_container_child_down(&mut env.content, path) {
                            drop(env);
                            let mut new_selection = selection.write();
                            new_selection.selected.clear();
                            new_selection.selected.insert(new_path);
                        }
                    }
                } else if !paths.is_empty() {
                    // Multiple selection: only support root-level
                    let mut root_indices: Vec<usize> = paths
                        .iter()
                        .filter_map(|p| {
                            if p.len() == 1 {
                                p.first().and_then(|s| s.as_child_index())
                            } else {
                                None
                            }
                        })
                        .collect();

                    if root_indices.len() == paths.len() {
                        root_indices.sort();
                        root_indices.reverse();
                        if root_indices[0] + 1 < env_len {
                            let mut env = envelope.write();
                            for &idx in &root_indices {
                                if idx + 1 < env_len {
                                    env.content.swap(idx, idx + 1);
                                }
                            }
                            drop(env);
                            let mut new_selection = selection.write();
                            new_selection.selected.clear();
                            for idx in root_indices {
                                new_selection
                                    .selected
                                    .insert(vec![PathSegment::Child(idx + 1)]);
                            }
                        }
                    }
                }
            }
            EditorAction::UpdateText {
                path,
                content,
                language,
            } => {
                let mut env = envelope.write();

                // Check if this is a list item path
                if is_list_item_path(&path) {
                    if let Some(text) = get_list_item_text_mut(&mut env.content, &path) {
                        update_inline_text(text, &content, language.as_deref());
                    }
                } else if let Some(node) = get_node_at_path_mut(&mut env.content, &path) {
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
                        _ => {}
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
                        NodeMetadata::FieldInputType(kind) => {
                            if let StructuredNode::Field(f) = node {
                                // Convert to new field type, preserving label and name
                                f.input_type = match kind {
                                    FieldInputKind::Text => FieldType::Text {
                                        regex: None,
                                        max_length: None,
                                        min_length: None,
                                    },
                                    FieldInputKind::Number => FieldType::Number {
                                        min: None,
                                        max: None,
                                        step: None,
                                    },
                                    FieldInputKind::Date => FieldType::Date,
                                    FieldInputKind::Email => FieldType::Email,
                                    FieldInputKind::Tel => FieldType::Tel,
                                    FieldInputKind::Checkbox => FieldType::Bool,
                                    FieldInputKind::Dropdown => {
                                        FieldType::Select { options: vec![] }
                                    }
                                    FieldInputKind::Radio => FieldType::Radio { options: vec![] },
                                };
                            }
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

                // Only support root-level conversions for now (single Child segment)
                if paths
                    .iter()
                    .all(|p| p.len() == 1 && matches!(p.first(), Some(PathSegment::Child(_))))
                {
                    let indices: Vec<usize> = paths
                        .iter()
                        .filter_map(|p| p.first().and_then(|s| s.as_child_index()))
                        .collect();
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

    let on_apply = props.on_apply;
    let on_cancel = props.on_cancel;

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
                            let envelope = envelope;
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
/// Content is parsed as markdown to preserve bold/italic formatting.
fn update_inline_text(text: &mut InlineText, content: &str, language: Option<&str>) {
    if let Some(lang) = language {
        // Parse markdown and merge with existing translations
        // This preserves formatting structure while keeping other languages' content
        *text = markdown_to_inline_text_multilingual(content, lang, text);
    } else {
        // Parse content as markdown to preserve bold/italic formatting
        *text = markdown_to_inline_text(content);
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
                    StructuredNode::Field(f) => {
                        // Field -> Paragraph: label becomes content
                        StructuredNode::Paragraph(ParagraphNode {
                            content: f
                                .label
                                .clone()
                                .unwrap_or_else(|| InlineText::plain(&f.name.to_string())),
                            som_path: f.som_path.clone(),
                            source_name: None,
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
                    StructuredNode::Field(f) => {
                        // Field -> Heading: label becomes content
                        StructuredNode::Heading(HeadingNode {
                            level: HeadingLevel::from_u8(level),
                            content: f
                                .label
                                .clone()
                                .unwrap_or_else(|| InlineText::plain(&f.name.to_string())),
                            som_path: f.som_path.clone(),
                            source_name: None,
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
        ConvertTarget::Field => {
            // Converting to field: text content becomes label
            nodes
                .iter()
                .map(|n| match n {
                    StructuredNode::Paragraph(p) => {
                        // Paragraph -> Field: content becomes label
                        let name = format!(
                            "field_{}",
                            Uuid::new_v4().to_string().replace('-', "")[..8].to_string()
                        );
                        StructuredNode::Field(FieldNode {
                            name: FieldId::from(name.as_str()),
                            som_path: p.som_path.clone(),
                            label: Some(p.content.clone()),
                            input_type: FieldType::Text {
                                regex: None,
                                max_length: None,
                                min_length: None,
                            },
                            value: None,
                            placeholder: None,
                        })
                    }
                    StructuredNode::Heading(h) => {
                        // Heading -> Field: content becomes label
                        let name = format!(
                            "field_{}",
                            Uuid::new_v4().to_string().replace('-', "")[..8].to_string()
                        );
                        StructuredNode::Field(FieldNode {
                            name: FieldId::from(name.as_str()),
                            som_path: h.som_path.clone(),
                            label: Some(h.content.clone()),
                            input_type: FieldType::Text {
                                regex: None,
                                max_length: None,
                                min_length: None,
                            },
                            value: None,
                            placeholder: None,
                        })
                    }
                    _ => (*n).clone(), // Keep unchanged
                })
                .collect()
        }
    }
}
