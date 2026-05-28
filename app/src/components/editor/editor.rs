//! Main structured document editor component.
//!
//! This is the top-level component that orchestrates the editor UI.

use dioxus::prelude::*;
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

use blueprint::document::ListStyleType;
use blueprint::structured::TableRow as StructTableRow;
use blueprint::structured::{GridLayoutElement, NameValue};
use blueprint::{
    DocumentEnvelope, FieldId, FieldNode, FieldType, GroupNode, HeadingLevel, HeadingNode,
    ListItem, ListNode, ParagraphNode, StructuredNode, TranslatedText,
};

use super::node_renderer::{FieldLabelsWrapper, NodeRenderer, NodesWrapper};
use super::smart_edit;
use super::state::{
    ConvertTarget, EditorAction, FieldInputKind, NewNodeType, NodeMetadata, PathSegment,
    SelectionState, available_conversions, can_indent, can_merge_selected, can_outdent,
    collect_selectable_paths, compute_add_options, delete_nodes, get_container_child_info,
    get_container_children_count, get_list_at_path, get_list_at_path_mut, get_list_item_text_mut,
    get_node_at_path, get_node_at_path_mut, get_shared_parent_path, get_table_column_count,
    indent_node, is_container_child_path, is_list_item_path, is_table_row_path,
    move_container_child_down, move_container_child_up, move_list_item_down, move_list_item_up,
    move_table_row_down, move_table_row_up, outdent_node, search_nodes,
};
use super::toolbar::EditorToolbar;
use crate::markdown::{markdown_to_inline_text, markdown_to_inline_text_multilingual};
use crate::platform::show_html_preview;

#[derive(Clone, Debug)]
enum SmartEditState {
    Idle,
    Loading,
    Preview {
        selected_indices: Vec<usize>,
        elapsed_ms: u128,
        result: smart_edit::SmartEditResult,
    },
    Error {
        selected_indices: Vec<usize>,
        elapsed_ms: u128,
        message: String,
    },
}

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
    /// Plain rendered page images (label → base64 PNG) for Smart Edit.
    pub plain_images: HashMap<String, String>,
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

    // Smart edit inline state
    let mut smart_edit_state = use_signal(|| SmartEditState::Idle);
    let mut smart_edit_session_name = use_signal(|| None::<String>);
    let smart_edit_images = props.plain_images.clone();
    let smart_edit_images_for_action = smart_edit_images.clone();
    let has_images = !smart_edit_images.is_empty();

    // Which change IDs the user has rejected in the current Preview round.
    // Reset to empty whenever a new smart-edit run starts.
    let mut rejected_ids = use_signal(std::collections::HashSet::<usize>::new);

    // Search state
    let mut search_query = use_signal(String::new);
    let mut search_index = use_signal(|| 0usize);

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

    // Compute context-aware add options
    let add_options = {
        let env = envelope.read();
        let sel = selection.read();
        compute_add_options(&env.content, &sel)
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
                    if let Some(l) = get_list_at_path(&env.content, &parent_path) {
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

    // Check if selected node can be indented/outdented
    let (can_indent_node, can_outdent_node) = {
        let env = envelope.read();
        let sel = selection.read();
        if sel.selected.len() == 1 {
            let path = sel.selected.iter().next().unwrap();
            (
                can_indent(&env.content, path),
                can_outdent(&env.content, path),
            )
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
                    // All selected paths must be siblings (same parent, all ending with Child)
                    if let Some(parent_path) = get_shared_parent_path(&paths) {
                        let mut indices: Vec<usize> = paths
                            .iter()
                            .filter_map(|p| p.last().and_then(|s| s.as_child_index()))
                            .collect();
                        indices.sort();

                        let mut env = envelope.write();

                        if parent_path.is_empty() {
                            // Root-level merge
                            let nodes: Vec<StructuredNode> = indices
                                .iter()
                                .filter_map(|&i| env.content.get(i).cloned())
                                .collect();

                            if let Ok(merged) = blueprint::merge_nodes(nodes) {
                                for &idx in indices.iter().rev() {
                                    if idx < env.content.len() {
                                        env.content.remove(idx);
                                    }
                                }
                                let insert_idx = indices[0].min(env.content.len());
                                env.content.insert(insert_idx, merged);
                            }
                        } else if let Some(parent) =
                            get_node_at_path_mut(&mut env.content, &parent_path)
                        {
                            match parent {
                                StructuredNode::Group(g) => {
                                    let nodes: Vec<StructuredNode> = indices
                                        .iter()
                                        .filter_map(|&i| g.children.get(i).cloned())
                                        .collect();

                                    if let Ok(merged) = blueprint::merge_nodes(nodes) {
                                        for &idx in indices.iter().rev() {
                                            if idx < g.children.len() {
                                                g.children.remove(idx);
                                            }
                                        }
                                        let insert_idx = indices[0].min(g.children.len());
                                        g.children.insert(insert_idx, merged);
                                    }
                                }
                                StructuredNode::GridLayout(g) => {
                                    let nodes: Vec<StructuredNode> = indices
                                        .iter()
                                        .filter_map(|&i| g.elements.get(i).map(|e| e.node.clone()))
                                        .collect();

                                    if let Ok(merged) = blueprint::merge_nodes(nodes) {
                                        let merged_span =
                                            g.elements.get(indices[0]).map(|e| e.span).unwrap_or(1);
                                        for &idx in indices.iter().rev() {
                                            if idx < g.elements.len() {
                                                g.elements.remove(idx);
                                            }
                                        }
                                        let insert_idx = indices[0].min(g.elements.len());
                                        g.elements.insert(
                                            insert_idx,
                                            GridLayoutElement {
                                                span: merged_span,
                                                node: merged,
                                            },
                                        );
                                    }
                                }
                                _ => {}
                            }
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
                        if let Some(PathSegment::Child(idx)) = path.first()
                            && *idx > 0
                        {
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
                        if let Some(PathSegment::Child(idx)) = path.first()
                            && *idx + 1 < env_len
                        {
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
            EditorAction::Indent => {
                let sel = selection.read();
                let paths: Vec<_> = sel.selected.iter().cloned().collect();
                drop(sel);

                if paths.len() == 1 {
                    let path = &paths[0];

                    if path.len() == 1 {
                        // Root-level: handle directly since we need Vec access
                        if let Some(PathSegment::Child(idx)) = path.first() {
                            let idx = *idx;
                            if idx > 0 {
                                let mut env = envelope.write();
                                if is_children_container_node(&env.content[idx - 1]) {
                                    let node = env.content.remove(idx);
                                    let new_child_idx = match &mut env.content[idx - 1] {
                                        StructuredNode::Group(g) => {
                                            g.children.push(node);
                                            g.children.len() - 1
                                        }
                                        StructuredNode::GridLayout(g) => {
                                            g.elements.push(GridLayoutElement { span: 1, node });
                                            g.elements.len() - 1
                                        }
                                        _ => unreachable!(),
                                    };
                                    drop(env);
                                    let mut new_selection = selection.write();
                                    new_selection.selected.clear();
                                    new_selection.selected.insert(vec![
                                        PathSegment::Child(idx - 1),
                                        PathSegment::Child(new_child_idx),
                                    ]);
                                }
                            }
                        }
                    } else if is_container_child_path(path) {
                        let mut env = envelope.write();
                        if let Some(new_path) = indent_node(&mut env.content, path) {
                            drop(env);
                            let mut new_selection = selection.write();
                            new_selection.selected.clear();
                            new_selection.selected.insert(new_path);
                        }
                    }
                }
            }
            EditorAction::Outdent => {
                let sel = selection.read();
                let paths: Vec<_> = sel.selected.iter().cloned().collect();
                drop(sel);

                if paths.len() == 1 {
                    let path = &paths[0];

                    if path.len() >= 2 && is_container_child_path(path) {
                        let (parent_path, child_idx) = get_container_child_info(path).unwrap();

                        if parent_path.len() == 1 {
                            // Parent is at root level: extract from parent and insert after it in root
                            if let Some(PathSegment::Child(parent_root_idx)) = parent_path.first() {
                                let parent_root_idx = *parent_root_idx;
                                let mut env = envelope.write();
                                let parent_node = &mut env.content[parent_root_idx];
                                let extracted = match parent_node {
                                    StructuredNode::Group(g) => {
                                        if child_idx < g.children.len() {
                                            Some(g.children.remove(child_idx))
                                        } else {
                                            None
                                        }
                                    }
                                    StructuredNode::GridLayout(g) => {
                                        if child_idx < g.elements.len() {
                                            Some(g.elements.remove(child_idx).node)
                                        } else {
                                            None
                                        }
                                    }
                                    _ => None,
                                };
                                if let Some(node) = extracted {
                                    let insert_idx = parent_root_idx + 1;
                                    env.content.insert(insert_idx, node);
                                    drop(env);
                                    let mut new_selection = selection.write();
                                    new_selection.selected.clear();
                                    new_selection
                                        .selected
                                        .insert(vec![PathSegment::Child(insert_idx)]);
                                }
                            }
                        } else {
                            // Deeper nesting: use the helper
                            let mut env = envelope.write();
                            if let Some(new_path) = outdent_node(&mut env.content, path) {
                                drop(env);
                                let mut new_selection = selection.write();
                                new_selection.selected.clear();
                                new_selection.selected.insert(new_path);
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
                                // Convert to new field type, preserving options if switching between Radio/Dropdown
                                let existing_options = match &f.input_type {
                                    FieldType::Radio { options }
                                    | FieldType::Select { options } => options.clone(),
                                    _ => vec![],
                                };
                                f.input_type = field_type_from_input_kind(kind, existing_options);
                            }
                        }
                        NodeMetadata::FieldOptions(options) => {
                            if let StructuredNode::Field(f) = node {
                                match &mut f.input_type {
                                    FieldType::Radio { options: opts }
                                    | FieldType::Select { options: opts } => {
                                        *opts = options;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        NodeMetadata::FieldRequired(required) => {
                            if let StructuredNode::Field(f) = node {
                                f.required = required;
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
                let mut env = envelope.write();
                let new_selection: Option<Vec<PathSegment>>;

                match node_type {
                    NewNodeType::ListItem => {
                        // Insert a new list item into the parent list
                        new_selection =
                            if let Some(l) = get_list_at_path_mut(&mut env.content, &parent) {
                                let insert_idx = index.min(l.items.len());
                                l.items.insert(
                                    insert_idx,
                                    ListItem::simple(TranslatedText::plain("New item")),
                                );
                                let mut path = parent.clone();
                                path.push(PathSegment::ListItem(insert_idx));
                                Some(path)
                            } else {
                                None
                            };
                    }
                    NewNodeType::TableRow => {
                        // Insert a new table row into the parent table
                        new_selection = if let Some(StructuredNode::Table(t)) =
                            get_node_at_path_mut(&mut env.content, &parent)
                        {
                            let col_count = get_table_column_count(t);
                            let cells: Vec<StructuredNode> = (0..col_count)
                                .map(|_| {
                                    StructuredNode::Paragraph(ParagraphNode {
                                        content: TranslatedText::plain(""),
                                        som_path: None,
                                        source_name: None,
                                    })
                                })
                                .collect();
                            let insert_idx = index.min(t.rows.len());
                            t.rows.insert(insert_idx, StructTableRow { cells });
                            let mut path = parent.clone();
                            path.push(PathSegment::TableRow(insert_idx));
                            Some(path)
                        } else {
                            None
                        };
                    }
                    NewNodeType::TableCell => {
                        // Insert a new column into the table (all rows + header)
                        // parent path ends at TableRow(r) or TableHeader
                        new_selection = if parent.len() >= 2 {
                            let table_path: Vec<_> = parent
                                .iter()
                                .take_while(|s| matches!(s, PathSegment::Child(_)))
                                .cloned()
                                .collect();
                            let row_segment = parent.last().cloned();
                            if let Some(StructuredNode::Table(t)) =
                                get_node_at_path_mut(&mut env.content, &table_path)
                            {
                                let insert_idx = index;
                                let make_cell = || {
                                    StructuredNode::Paragraph(ParagraphNode {
                                        content: TranslatedText::plain(""),
                                        som_path: None,
                                        source_name: None,
                                    })
                                };
                                // Insert into header if present
                                if let Some(header) = &mut t.header {
                                    let idx = insert_idx.min(header.cells.len());
                                    header.cells.insert(idx, make_cell());
                                }
                                // Insert into all rows
                                for row in &mut t.rows {
                                    let idx = insert_idx.min(row.cells.len());
                                    row.cells.insert(idx, make_cell());
                                }
                                // Select the newly inserted cell in the originating row
                                let mut path = parent.clone();
                                let cell_idx = match &row_segment {
                                    Some(PathSegment::TableRow(row_idx)) => t
                                        .rows
                                        .get(*row_idx)
                                        .map(|r| insert_idx.min(r.cells.len().saturating_sub(1)))
                                        .unwrap_or(insert_idx),
                                    Some(PathSegment::TableHeader) => t
                                        .header
                                        .as_ref()
                                        .map(|h| insert_idx.min(h.cells.len().saturating_sub(1)))
                                        .unwrap_or(insert_idx),
                                    _ => insert_idx,
                                };
                                path.push(PathSegment::TableCell(cell_idx));
                                Some(path)
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                    }
                    _ => {
                        // Standard node types: Paragraph, Heading, List, Group
                        let new_node = match node_type {
                            NewNodeType::Paragraph => StructuredNode::Paragraph(ParagraphNode {
                                content: TranslatedText::plain("New paragraph"),
                                som_path: None,
                                source_name: None,
                            }),
                            NewNodeType::Heading(level) => StructuredNode::Heading(HeadingNode {
                                level: HeadingLevel::from_u8(level),
                                content: TranslatedText::plain("New heading"),
                                som_path: None,
                                source_name: None,
                            }),
                            NewNodeType::List => StructuredNode::List(ListNode {
                                list_style: ListStyleType::Disc,
                                items: vec![ListItem::simple(TranslatedText::plain("New item"))],
                            }),
                            NewNodeType::Group => {
                                StructuredNode::Group(GroupNode { children: vec![] })
                            }
                            _ => unreachable!(),
                        };

                        if parent.is_empty() {
                            // Add to root
                            let insert_idx = index.min(env.content.len());
                            env.content.insert(insert_idx, new_node);
                            new_selection = Some(vec![PathSegment::Child(insert_idx)]);
                        } else if let Some(parent_node) =
                            get_node_at_path_mut(&mut env.content, &parent)
                        {
                            // Add to nested parent (Group or GridLayout)
                            match parent_node {
                                StructuredNode::Group(g) => {
                                    let insert_idx = index.min(g.children.len());
                                    g.children.insert(insert_idx, new_node);
                                    let mut path = parent.clone();
                                    path.push(PathSegment::Child(insert_idx));
                                    new_selection = Some(path);
                                }
                                StructuredNode::GridLayout(g) => {
                                    let insert_idx = index.min(g.elements.len());
                                    g.elements.insert(
                                        insert_idx,
                                        GridLayoutElement {
                                            span: 1,
                                            node: new_node,
                                        },
                                    );
                                    let mut path = parent.clone();
                                    path.push(PathSegment::Child(insert_idx));
                                    new_selection = Some(path);
                                }
                                _ => {
                                    new_selection = None;
                                }
                            }
                        } else {
                            new_selection = None;
                        }
                    }
                }

                drop(env);
                // Select the newly added element
                if let Some(new_path) = new_selection {
                    let mut sel = selection.write();
                    sel.selected.clear();
                    sel.selected.insert(new_path);
                }
            }
            EditorAction::ConvertSelected(target) => {
                // Get selected paths sorted by position
                let mut paths: Vec<_> = selection.read().selected.iter().cloned().collect();
                paths.sort();

                // All selected paths must be siblings (same parent, all ending with Child)
                if let Some(parent_path) = get_shared_parent_path(&paths) {
                    let mut indices: Vec<usize> = paths
                        .iter()
                        .filter_map(|p| p.last().and_then(|s| s.as_child_index()))
                        .collect();
                    indices.sort();

                    if parent_path.is_empty() {
                        // Root-level conversion
                        let env_read = envelope.read();
                        let nodes: Vec<&StructuredNode> = indices
                            .iter()
                            .filter_map(|&i| env_read.content.get(i))
                            .collect();

                        if !nodes.is_empty() {
                            let converted_nodes = convert_nodes(&nodes, target);
                            drop(env_read);

                            if !converted_nodes.is_empty() {
                                let mut env = envelope.write();
                                for &idx in indices.iter().rev() {
                                    if idx < env.content.len() {
                                        env.content.remove(idx);
                                    }
                                }
                                let insert_idx = indices[0].min(env.content.len());
                                for (i, node) in converted_nodes.into_iter().enumerate() {
                                    env.content.insert(insert_idx + i, node);
                                }
                            }
                        }
                    } else {
                        // Non-root conversion: collect clones first, then mutate
                        let nodes_cloned: Vec<StructuredNode> = {
                            let env_read = envelope.read();
                            let parent = get_node_at_path(&env_read.content, &parent_path);
                            match parent {
                                Some(StructuredNode::Group(g)) => indices
                                    .iter()
                                    .filter_map(|&i| g.children.get(i).cloned())
                                    .collect(),
                                Some(StructuredNode::GridLayout(g)) => indices
                                    .iter()
                                    .filter_map(|&i| g.elements.get(i).map(|e| e.node.clone()))
                                    .collect(),
                                _ => vec![],
                            }
                        };

                        if !nodes_cloned.is_empty() {
                            let node_refs: Vec<&StructuredNode> = nodes_cloned.iter().collect();
                            let converted_nodes = convert_nodes(&node_refs, target);

                            if !converted_nodes.is_empty() {
                                let mut env = envelope.write();
                                if let Some(parent) =
                                    get_node_at_path_mut(&mut env.content, &parent_path)
                                {
                                    match parent {
                                        StructuredNode::Group(g) => {
                                            for &idx in indices.iter().rev() {
                                                if idx < g.children.len() {
                                                    g.children.remove(idx);
                                                }
                                            }
                                            let insert_idx = indices[0].min(g.children.len());
                                            for (i, node) in converted_nodes.into_iter().enumerate()
                                            {
                                                g.children.insert(insert_idx + i, node);
                                            }
                                        }
                                        StructuredNode::GridLayout(g) => {
                                            let first_span = g
                                                .elements
                                                .get(indices[0])
                                                .map(|e| e.span)
                                                .unwrap_or(1);
                                            for &idx in indices.iter().rev() {
                                                if idx < g.elements.len() {
                                                    g.elements.remove(idx);
                                                }
                                            }
                                            let insert_idx = indices[0].min(g.elements.len());
                                            for (i, node) in converted_nodes.into_iter().enumerate()
                                            {
                                                let span = if i == 0 { first_span } else { 1 };
                                                g.elements.insert(
                                                    insert_idx + i,
                                                    GridLayoutElement { span, node },
                                                );
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }

                selection.write().clear();
            }
            EditorAction::SmartEdit => {
                let selected_indices: Vec<usize> = Vec::new();
                let session_name = format!("smart-edit-{}", Uuid::new_v4());
                smart_edit_session_name.set(Some(session_name.clone()));
                let content = envelope.read().content.clone();
                let plain_images = smart_edit_images_for_action.clone();
                let started_at = std::time::Instant::now();

                smart_edit_state.set(SmartEditState::Loading);
                rejected_ids.write().clear();

                spawn(async move {
                    match smart_edit::run_smart_edit(
                        &content,
                        &selected_indices,
                        &plain_images,
                        &session_name,
                        false,
                    )
                    .await
                    {
                        Ok(result) => {
                            let elapsed_ms = started_at.elapsed().as_millis();
                            smart_edit_state.set(SmartEditState::Preview {
                                selected_indices,
                                elapsed_ms,
                                result,
                            });
                        }
                        Err(message) => {
                            let elapsed_ms = started_at.elapsed().as_millis();
                            smart_edit_state.set(SmartEditState::Error {
                                selected_indices,
                                elapsed_ms,
                                message,
                            });
                        }
                    }
                });
            }
            EditorAction::SelectAll => {
                let all_paths = {
                    let env = envelope.read();
                    collect_selectable_paths(&env.content)
                };
                let mut sel = selection.write();
                sel.selected = all_paths;
            }
        }
    };

    let on_apply = props.on_apply;
    let on_cancel = props.on_cancel;

    // Compute search results once per render for both the search bar and the highlight set
    let search_results = search_nodes(&envelope.read().content, &search_query.read());
    let search_result_count = search_results.len();
    let search_current_idx = if search_result_count == 0 {
        0
    } else {
        *search_index.read() % search_result_count
    };
    let search_highlight: std::collections::HashSet<super::state::NodePath> =
        search_results.iter().cloned().collect();

    rsx! {
        div { class: "structured-editor",
            // Header
            div { class: "editor-header",
                h2 { "Edit Structure" }
                div { class: "editor-header-actions",
                    button {
                        class: "editor-btn editor-btn-secondary",
                        title: "Preview the current (unapplied) changes as HTML",
                        onclick: {
                            let envelope = envelope;
                            move |_| {
                                let html = blueprint::to_html(
                                    &envelope.read().content,
                                    &blueprint::HtmlConfig::default(),
                                );
                                show_html_preview(html, "editor-preview.html");
                            }
                        },
                        "Preview HTML"
                    }
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
                can_indent: can_indent_node,
                can_outdent: can_outdent_node,
                available_conversions: conversions.clone(),
                add_options: add_options.clone(),
                has_images,
                is_smart_edit_loading: matches!(*smart_edit_state.read(), SmartEditState::Loading),
                node_count: envelope.read().content.len(),
                on_action: handle_action.clone(),
            }

            // Smart Edit inline review panel
            {
                match smart_edit_state.read().clone() {
                    SmartEditState::Idle => rsx! {},
                    SmartEditState::Loading => rsx! {},
                    SmartEditState::Preview { selected_indices, elapsed_ms, result } => {
                        let session_name_for_retry = smart_edit_session_name.read().clone();
                        let selected_indices_for_apply = selected_indices.clone();
                        let selected_indices_for_retry = selected_indices.clone();
                        let content_for_retry = envelope.read().content.clone();
                        let plain_images_for_retry = smart_edit_images.clone();
                        let nodes_for_apply = result.nodes.clone();
                        let original_nodes_for_preview: Vec<StructuredNode> = if selected_indices
                            .is_empty()
                        {
                            envelope.read().content.clone()
                        } else {
                            selected_indices
                                .iter()
                                .filter_map(|&i| envelope.read().content.get(i).cloned())
                                .collect()
                        };
                        let modified_nodes_for_preview = result.nodes.clone();
                        rsx! {
                            div { class: "smart-edit-inline-panel",
                                h3 { "Smart Edit Review" }
                                p { class: "smart-edit-hint", "Completed in {elapsed_ms}ms · {result.nodes.len()} node(s)" }

                                if result.changes.is_empty() {
                                    p { class: "smart-edit-hint smart-edit-warning",
                                        "Copilot did not provide a change list. Accept or dismiss the suggestion."
                                    }
                                } else {
                                    p { class: "smart-edit-hint smart-edit-success",
                                        "Review the proposed changes below. Uncheck any changes you want to reject."
                                    }
                                    div { class: "smart-edit-change-list",
                                        for change in result.changes.clone() {
                                            {
                                                let id = change.id;
                                                let is_rejected = rejected_ids.read().contains(&id);
                                                rsx! {
                                                    label { class: if is_rejected { "smart-edit-change-item smart-edit-change-rejected" } else { "smart-edit-change-item" },
                                                        input {
                                                            r#type: "checkbox",
                                                            checked: !is_rejected,
                                                            onchange: move |evt| {
                                                                if evt.checked() {
                                                                    rejected_ids.write().remove(&id);
                                                                } else {
                                                                    rejected_ids.write().insert(id);
                                                                }
                                                            },
                                                        }
                                                        span { "{change.description}" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                div { class: "smart-edit-actions",
                                    button {
                                        class: "editor-btn editor-btn-secondary",
                                        onclick: {
                                            let original_nodes = original_nodes_for_preview.clone();
                                            move |_| {
                                                let html = blueprint::to_html(
                                                    &original_nodes,
                                                    &blueprint::HtmlConfig::default(),
                                                );
                                                show_html_preview(html, "smart-edit-original-preview.html");
                                            }
                                        },
                                        "Preview Original"
                                    }
                                    button {
                                        class: "editor-btn editor-btn-secondary",
                                        onclick: {
                                            let modified_nodes = modified_nodes_for_preview.clone();
                                            move |_| {
                                                let html = blueprint::to_html(
                                                    &modified_nodes,
                                                    &blueprint::HtmlConfig::default(),
                                                );
                                                show_html_preview(html, "smart-edit-modified-preview.html");
                                            }
                                        },
                                        "Preview Modified"
                                    }
                                    button {
                                        class: "editor-btn editor-btn-secondary",
                                        onclick: move |_| {
                                            smart_edit_state.set(SmartEditState::Idle);
                                            smart_edit_session_name.set(None);
                                        },
                                        "Dismiss"
                                    }

                                    // Show "Retry with Feedback" when some changes are rejected.
                                    if !rejected_ids.read().is_empty() {
                                        {
                                            let rejected: Vec<smart_edit::ChangeItem> = result
                                                .changes
                                                .iter()
                                                .filter(|c| rejected_ids.read().contains(&c.id))
                                                .cloned()
                                                .collect();
                                            let content = content_for_retry.clone();
                                            let plain_images = plain_images_for_retry.clone();
                                            let selected_indices = selected_indices_for_retry.clone();
                                            let session_name = session_name_for_retry
                                                .clone()
                                                .unwrap_or_else(|| format!("smart-edit-{}", Uuid::new_v4()));

                                            rsx! {
                                                button {
                                                    class: "editor-btn editor-btn-secondary",
                                                    onclick: move |_| {
                                                        let content = content.clone();
                                                        let plain_images = plain_images.clone();
                                                        let selected_indices = selected_indices.clone();
                                                        let rejected = rejected.clone();
                                                        let session_name = session_name.clone();
                                                        let started_at = std::time::Instant::now();
                                                        smart_edit_state.set(SmartEditState::Loading);
                                                        rejected_ids.write().clear();
                                                        spawn(async move {
                                                            match smart_edit::run_smart_edit_with_feedback(
                                                                    &content,
                                                                    &selected_indices,
                                                                    &plain_images,
                                                                    &rejected,
                                                                    &session_name,
                                                                )
                                                                .await
                                                            {
                                                                Ok(result) => {
                                                                    let elapsed_ms = started_at.elapsed().as_millis();
                                                                    smart_edit_state
                                                                        .set(SmartEditState::Preview {
                                                                            selected_indices,
                                                                            elapsed_ms,
                                                                            result,
                                                                        });
                                                                }
                                                                Err(message) => {
                                                                    let elapsed_ms = started_at.elapsed().as_millis();
                                                                    smart_edit_state
                                                                        .set(SmartEditState::Error {
                                                                            selected_indices,
                                                                            elapsed_ms,
                                                                            message,
                                                                        });
                                                                }
                                                            }
                                                        });
                                                    },
                                                    "Retry with Feedback"
                                                }
                                            }
                                        }
                                    }

                                    // "Apply Changes" is only shown when all changes are accepted
                                    // (nothing in rejected_ids), or when there are no changes listed.
                                    if rejected_ids.read().is_empty() {
                                        button {
                                            class: "editor-btn editor-btn-primary",
                                            onclick: move |_| {
                                                let mut indices = selected_indices_for_apply.clone();
                                                indices.sort();

                                                let mut env = envelope.write();
                                                for &idx in indices.iter().rev() {
                                                    if idx < env.content.len() {
                                                        env.content.remove(idx);
                                                    }
                                                }
                                                let insert_at =
                                                    indices.first().copied().unwrap_or(0).min(env.content.len());
                                                for (i, node) in nodes_for_apply.clone().into_iter().enumerate() {
                                                    env.content.insert(insert_at + i, node);
                                                }
                                                drop(env);

                                                selection.write().clear();
                                                smart_edit_state.set(SmartEditState::Idle);
                                                smart_edit_session_name.set(None);
                                            },
                                            "Apply Changes"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    SmartEditState::Error { selected_indices, elapsed_ms, message } => {
                        let session_name_for_retry = smart_edit_session_name.read().clone();
                        let selected_indices_for_retry = selected_indices.clone();
                        let content_for_retry = envelope.read().content.clone();
                        let plain_images_for_retry = smart_edit_images.clone();
                        rsx! {
                            div { class: "smart-edit-inline-panel",
                                p { class: "smart-edit-hint smart-edit-error", "Smart Edit failed: {message}" }
                                p { class: "smart-edit-hint", "Failed after {elapsed_ms}ms" }
                                div { class: "smart-edit-actions",
                                    button {
                                        class: "editor-btn editor-btn-secondary",
                                        onclick: move |_| {
                                            smart_edit_state.set(SmartEditState::Idle);
                                            smart_edit_session_name.set(None);
                                        },
                                        "Dismiss"
                                    }
                                    button {
                                        class: "editor-btn editor-btn-secondary",
                                        onclick: move |_| {
                                            let content = content_for_retry.clone();
                                            let plain_images = plain_images_for_retry.clone();
                                            let selected_indices = selected_indices_for_retry.clone();
                                            let session_name = session_name_for_retry
                                                .clone()
                                                .unwrap_or_else(|| format!("smart-edit-{}", Uuid::new_v4()));
                                            let session_name = session_name.clone();
                                            let started_at = std::time::Instant::now();
                                            smart_edit_state.set(SmartEditState::Loading);
                                            rejected_ids.write().clear();
                                            spawn(async move {
                                                match smart_edit::run_smart_edit(
                                                        &content,
                                                        &selected_indices,
                                                        &plain_images,
                                                        &session_name,
                                                        true,
                                                    )
                                                    .await
                                                {
                                                    Ok(result) => {
                                                        let elapsed_ms = started_at.elapsed().as_millis();
                                                        smart_edit_state
                                                            .set(SmartEditState::Preview {
                                                                selected_indices,
                                                                elapsed_ms,
                                                                result,
                                                            });
                                                    }
                                                    Err(message) => {
                                                        let elapsed_ms = started_at.elapsed().as_millis();
                                                        smart_edit_state
                                                            .set(SmartEditState::Error {
                                                                selected_indices,
                                                                elapsed_ms,
                                                                message,
                                                            });
                                                    }
                                                }
                                            });
                                        },
                                        "Try Again"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Search bar
            div { class: "editor-search",
                span { class: "editor-search-icon", "🔍" }
                input {
                    class: "editor-search-input",
                    r#type: "text",
                    placeholder: "Search text, labels… (all languages)",
                    value: "{search_query.read()}",
                    oninput: move |evt| {
                        search_query.set(evt.value());
                        search_index.set(0);
                    },
                }
                if search_result_count > 0 {
                    span { class: "editor-search-count",
                        "{search_current_idx + 1} / {search_result_count}"
                    }
                    button {
                        class: "editor-search-nav",
                        title: "Previous match",
                        onclick: {
                            let results = search_results.clone();
                            move |_| {
                                let idx = *search_index.read() % results.len();
                                let new_idx = if idx == 0 { results.len() - 1 } else { idx - 1 };
                                search_index.set(new_idx);
                                if let Some(path) = results.get(new_idx) {
                                    selection.write().select_single(path.clone());
                                }
                            }
                        },
                        "▲"
                    }
                    button {
                        class: "editor-search-nav",
                        title: "Next match",
                        onclick: {
                            let results = search_results.clone();
                            move |_| {
                                let new_idx = (*search_index.read() + 1) % results.len();
                                search_index.set(new_idx);
                                if let Some(path) = results.get(new_idx) {
                                    selection.write().select_single(path.clone());
                                }
                            }
                        },
                        "▼"
                    }
                } else if !search_query.read().is_empty() {
                    span { class: "editor-search-count editor-search-no-match", "No matches" }
                }
                if !search_query.read().is_empty() {
                    button {
                        class: "editor-search-clear",
                        title: "Clear search",
                        onclick: move |_| {
                            search_query.set(String::new());
                            search_index.set(0);
                        },
                        "✕"
                    }
                }
            }

            // Document tree
            div { class: "editor-content",
                NodeRenderer {
                    nodes: NodesWrapper(envelope.read().content.clone()),
                    selection: selection.read().clone(),
                    languages: languages.clone(),
                    field_labels: field_labels.clone(),
                    highlight: search_highlight,
                    on_action: handle_action.clone(),
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
fn update_inline_text(text: &mut TranslatedText, content: &str, language: Option<&str>) {
    if let Some(lang) = language {
        // Parse markdown and merge with existing translations
        // This preserves formatting structure while keeping other languages' content
        *text = markdown_to_inline_text_multilingual(content, lang, text);
    } else {
        // No language specified: parse and replace as single-language text
        let parsed = markdown_to_inline_text(content);
        *text = TranslatedText::single("default", parsed);
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
                                .unwrap_or_else(|| TranslatedText::plain(f.name.to_string())),
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
                                    content: item.content.clone(),
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
                                .unwrap_or_else(|| TranslatedText::plain(f.name.to_string())),
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
            let items: Vec<ListItem> = nodes
                .iter()
                .filter_map(|n| match n {
                    StructuredNode::Paragraph(p) => Some(ListItem::simple(p.content.clone())),
                    StructuredNode::Heading(h) => Some(ListItem::simple(h.content.clone())),
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
                            &Uuid::new_v4().to_string().replace('-', "")[..8]
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
                            required: false,
                        })
                    }
                    StructuredNode::Heading(h) => {
                        // Heading -> Field: content becomes label
                        let name = format!(
                            "field_{}",
                            &Uuid::new_v4().to_string().replace('-', "")[..8]
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
                            required: false,
                        })
                    }
                    _ => (*n).clone(), // Keep unchanged
                })
                .collect()
        }
    }
}

fn field_type_from_input_kind(kind: FieldInputKind, existing_options: Vec<NameValue>) -> FieldType {
    match kind {
        FieldInputKind::Text => FieldType::Text {
            regex: None,
            max_length: None,
            min_length: None,
        },
        FieldInputKind::Textarea => FieldType::Textarea { max_length: None },
        FieldInputKind::Number => FieldType::Number {
            min: None,
            max: None,
            step: None,
        },
        FieldInputKind::Date => FieldType::Date,
        FieldInputKind::Email => FieldType::Email,
        FieldInputKind::Tel => FieldType::Tel,
        FieldInputKind::Checkbox => FieldType::Bool,
        FieldInputKind::Dropdown => FieldType::Select {
            options: existing_options,
        },
        FieldInputKind::Radio => FieldType::Radio {
            options: existing_options,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueprint::structured::{InputValue, TranslatableString};

    #[test]
    fn textarea_kind_converts_to_textarea_field_type() {
        let converted = field_type_from_input_kind(FieldInputKind::Textarea, vec![]);
        assert!(matches!(
            converted,
            FieldType::Textarea { max_length: None }
        ));
    }

    #[test]
    fn dropdown_and_radio_keep_existing_options() {
        let existing_options = vec![NameValue {
            name: TranslatableString::Plain("Option".to_string()),
            value: InputValue::Text("value".to_string()),
        }];

        let dropdown =
            field_type_from_input_kind(FieldInputKind::Dropdown, existing_options.clone());
        let radio = field_type_from_input_kind(FieldInputKind::Radio, existing_options.clone());

        assert!(matches!(
            dropdown,
            FieldType::Select { options } if options == existing_options
        ));
        assert!(matches!(
            radio,
            FieldType::Radio { options } if options == existing_options
        ));
    }
}

/// Check if a node is a container that can accept children (Group or GridLayout).
fn is_children_container_node(node: &StructuredNode) -> bool {
    matches!(
        node,
        StructuredNode::Group(_) | StructuredNode::GridLayout(_)
    )
}
