//! State and tree-manipulation helpers for the AEM node editor.
//!
//! The `AemNode` tree is uniform — only `Root`, `Panel`, and `Repeatable`
//! carry `children` — so a node is addressed by a simple path of child
//! indices ([`AemPath`]). All field-level data (options, alignment, mandatory…)
//! is edited through the metadata editor rather than the tree path.

use std::collections::HashSet;

use blueprint::{AemNode, AemOption, ConditionRule};
use uuid::Uuid;

/// Path to a node in the tree: a sequence of child indices from the root.
/// The empty path refers to the root itself.
pub type AemPath = Vec<usize>;

// ── Selection ───────────────────────────────────────────────────────────────

/// Tracks the current selection and inline-editing focus.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AemSelectionState {
    pub selected: HashSet<AemPath>,
    pub editing: Option<AemPath>,
    pub editing_metadata: Option<AemPath>,
}

impl AemSelectionState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn toggle(&mut self, path: AemPath) {
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
    }
    pub fn select_single(&mut self, path: AemPath) {
        self.selected.clear();
        self.selected.insert(path);
    }
    pub fn clear(&mut self) {
        self.selected.clear();
        self.editing = None;
        self.editing_metadata = None;
    }
    pub fn is_selected(&self, path: &AemPath) -> bool {
        self.selected.contains(path)
    }
    pub fn count(&self) -> usize {
        self.selected.len()
    }
    pub fn start_editing(&mut self, path: AemPath) {
        self.editing = Some(path);
        self.editing_metadata = None;
    }
    pub fn start_editing_metadata(&mut self, path: AemPath) {
        self.editing_metadata = Some(path);
        self.editing = None;
    }
    pub fn stop_editing(&mut self) {
        self.editing = None;
        self.editing_metadata = None;
    }
    pub fn is_editing(&self, path: &AemPath) -> bool {
        self.editing.as_ref() == Some(path)
    }
    pub fn is_editing_metadata(&self, path: &AemPath) -> bool {
        self.editing_metadata.as_ref() == Some(path)
    }
}

// ── Actions ───────────────────────────────────────────────────────────────

/// A new node kind that can be inserted via the Add menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NewAemNodeType {
    Panel,
    Repeatable,
    TextField,
    NumberField,
    DatePicker,
    Dropdown,
    RadioButton,
    Checkbox,
    TextDraw,
    TitleDraw,
}

/// A conversion target for the Convert menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AemConvertTarget {
    TextField,
    NumberField,
    DatePicker,
    Dropdown,
    RadioButton,
    Checkbox,
    TextDraw,
    TitleDraw,
}

/// An editable metadata mutation applied to a single node.
#[derive(Clone, Debug, PartialEq)]
pub enum AemMetadata {
    Name(String),
    Mandatory(bool),
    Visible(bool),
    DorExclude(bool),
    IsPage(bool),
    Colspan(u32),
    MaxChars(Option<usize>),
    HeadingLevel(u8),
    Occurrences { min: u32, max: u32 },
    Alignment(blueprint::OptionAlignment),
    Options(Vec<AemOption>),
    Conditions(Vec<ConditionRule>),
    FragRef(String),
}

/// Editor actions dispatched by the toolbar and node renderer.
#[derive(Clone, Debug)]
pub enum AemEditorAction {
    ToggleSelection(AemPath),
    SelectSingle(AemPath),
    ClearSelection,
    SelectAll,
    StartEditing(AemPath),
    StartEditingMetadata(AemPath),
    StopEditing,
    DeleteSelected,
    DuplicateSelected,
    MoveUp,
    MoveDown,
    Indent,
    Outdent,
    AddNode {
        parent: AemPath,
        kind: NewAemNodeType,
    },
    ConvertSelected(AemConvertTarget),
    UpdateText {
        path: AemPath,
        content: String,
    },
    UpdateTranslation {
        path: AemPath,
        language: String,
        text: String,
    },
    UpdateMetadata {
        path: AemPath,
        metadata: AemMetadata,
    },
    SmartAemEdit,
    UploadToAem,
}

/// Returns a history label for content-mutating actions, or `None` for
/// selection/editing-only actions.
pub fn describe_action(action: &AemEditorAction) -> Option<&'static str> {
    match action {
        AemEditorAction::DeleteSelected => Some("Delete"),
        AemEditorAction::DuplicateSelected => Some("Duplicate"),
        AemEditorAction::MoveUp => Some("Move up"),
        AemEditorAction::MoveDown => Some("Move down"),
        AemEditorAction::Indent => Some("Indent"),
        AemEditorAction::Outdent => Some("Outdent"),
        AemEditorAction::AddNode { .. } => Some("Add node"),
        AemEditorAction::ConvertSelected(_) => Some("Convert"),
        AemEditorAction::UpdateText { .. } => Some("Edit text"),
        AemEditorAction::UpdateMetadata { .. } => Some("Edit properties"),
        _ => None,
    }
}

// ── Tree navigation ─────────────────────────────────────────────────────────

/// Immutable reference to a node's children, if it is a container.
pub fn children_ref(node: &AemNode) -> Option<&Vec<AemNode>> {
    match node {
        AemNode::Root { children, .. }
        | AemNode::Panel { children, .. }
        | AemNode::Repeatable { children, .. } => Some(children),
        _ => None,
    }
}

/// Mutable reference to a node's children, if it is a container.
pub fn children_mut(node: &mut AemNode) -> Option<&mut Vec<AemNode>> {
    match node {
        AemNode::Root { children, .. }
        | AemNode::Panel { children, .. }
        | AemNode::Repeatable { children, .. } => Some(children),
        _ => None,
    }
}

/// Whether a node can contain children.
pub fn is_container(node: &AemNode) -> bool {
    matches!(
        node,
        AemNode::Root { .. } | AemNode::Panel { .. } | AemNode::Repeatable { .. }
    )
}

pub fn get_node<'a>(root: &'a AemNode, path: &[usize]) -> Option<&'a AemNode> {
    let mut cur = root;
    for &i in path {
        cur = children_ref(cur)?.get(i)?;
    }
    Some(cur)
}

pub fn get_node_mut<'a>(root: &'a mut AemNode, path: &[usize]) -> Option<&'a mut AemNode> {
    let mut cur = root;
    for &i in path {
        cur = children_mut(cur)?.get_mut(i)?;
    }
    Some(cur)
}

/// Return the parent's children vector and the child index for `path`.
fn parent_children_mut<'a>(
    root: &'a mut AemNode,
    path: &[usize],
) -> Option<(&'a mut Vec<AemNode>, usize)> {
    let (last, parent_path) = path.split_last()?;
    let parent = get_node_mut(root, parent_path)?;
    let children = children_mut(parent)?;
    Some((children, *last))
}

// ── Mutations ───────────────────────────────────────────────────────────────

/// Move a node up or down among its siblings. Returns its new path.
pub fn move_node(root: &mut AemNode, path: &[usize], up: bool) -> Option<AemPath> {
    let (children, idx) = parent_children_mut(root, path)?;
    let new_idx = if up {
        idx.checked_sub(1)?
    } else if idx + 1 < children.len() {
        idx + 1
    } else {
        return None;
    };
    children.swap(idx, new_idx);
    let mut np = path.to_vec();
    *np.last_mut()? = new_idx;
    Some(np)
}

/// Whether the node at `path` can be moved into a container above it.
pub fn can_indent(root: &AemNode, path: &[usize]) -> bool {
    let Some((last, parent_path)) = path.split_last() else {
        return false;
    };
    if *last == 0 {
        return false;
    }
    let Some(parent) = get_node(root, parent_path) else {
        return false;
    };
    let Some(children) = children_ref(parent) else {
        return false;
    };
    children.get(*last - 1).is_some_and(is_container)
}

/// Whether the node at `path` is nested deep enough to move out of its parent.
pub fn can_outdent(path: &[usize]) -> bool {
    path.len() >= 2
}

/// Move a node into the container immediately above it (as that container's
/// last child). Returns the moved node's new path.
pub fn indent_node(root: &mut AemNode, path: &[usize]) -> Option<AemPath> {
    if !can_indent(root, path) {
        return None;
    }
    let (children, idx) = parent_children_mut(root, path)?;
    let node = children.remove(idx);
    let target = &mut children[idx - 1];
    let target_children = children_mut(target)?;
    let new_child_idx = target_children.len();
    target_children.push(node);

    let mut np = path.to_vec();
    let last = np.len() - 1;
    np[last] = idx - 1;
    np.push(new_child_idx);
    Some(np)
}

/// Move a node out of its parent, inserting it just after the parent in the
/// grandparent's children. Returns the moved node's new path.
pub fn outdent_node(root: &mut AemNode, path: &[usize]) -> Option<AemPath> {
    if !can_outdent(path) {
        return None;
    }
    // Remove from parent.
    let (parent_children, idx) = parent_children_mut(root, path)?;
    let node = parent_children.remove(idx);

    // Insert into grandparent after the parent.
    let parent_path = &path[..path.len() - 1];
    let parent_idx = *parent_path.last().unwrap();
    let (grandparent_children, _) = parent_children_mut(root, parent_path)?;
    let insert_at = (parent_idx + 1).min(grandparent_children.len());
    grandparent_children.insert(insert_at, node);

    let mut np = parent_path.to_vec();
    *np.last_mut().unwrap() = insert_at;
    Some(np)
}

/// Delete the nodes at the given paths (root path is ignored).
pub fn delete_nodes(root: &mut AemNode, paths: &HashSet<AemPath>) {
    // Remove deepest / highest-index first so earlier indices stay valid.
    let mut sorted: Vec<AemPath> = paths.iter().filter(|p| !p.is_empty()).cloned().collect();
    sorted.sort();
    for path in sorted.into_iter().rev() {
        if let Some((children, idx)) = parent_children_mut(root, &path)
            && idx < children.len()
        {
            children.remove(idx);
        }
    }
}

/// Recursively assign fresh UUIDs to a node and its descendants.
pub fn regenerate_uuids(node: &mut AemNode) {
    set_uuid(node, Uuid::new_v4());
    if let Some(children) = children_mut(node) {
        for child in children.iter_mut() {
            regenerate_uuids(child);
        }
    }
}

fn set_uuid(node: &mut AemNode, new: Uuid) {
    use AemNode::*;
    match node {
        Root { .. } => {}
        Panel { uuid, .. }
        | TextField { uuid, .. }
        | NumberField { uuid, .. }
        | DatePicker { uuid, .. }
        | Dropdown { uuid, .. }
        | Checkbox { uuid, .. }
        | RadioButton { uuid, .. }
        | TextDraw { uuid, .. }
        | TitleDraw { uuid, .. }
        | Repeatable { uuid, .. }
        | Fragment { uuid, .. }
        | Preface { uuid, .. }
        | Appendix { uuid, .. }
        | FootnotePlaceholder { uuid, .. }
        | Custom { uuid, .. } => *uuid = new,
    }
}

/// Duplicate a single node in place (inserted right after it, with fresh
/// UUIDs). Returns the new node's path.
pub fn duplicate_node(root: &mut AemNode, path: &[usize]) -> Option<AemPath> {
    let (children, idx) = parent_children_mut(root, path)?;
    let mut clone = children.get(idx)?.clone();
    regenerate_uuids(&mut clone);
    children.insert(idx + 1, clone);
    let mut np = path.to_vec();
    *np.last_mut()? = idx + 1;
    Some(np)
}

/// Insert a new node of the given kind as the last child of `parent`.
pub fn add_node(root: &mut AemNode, parent: &[usize], kind: NewAemNodeType) -> Option<AemPath> {
    let parent_node = get_node_mut(root, parent)?;
    let children = children_mut(parent_node)?;
    let new_idx = children.len();
    children.push(new_node(kind));
    let mut np = parent.to_vec();
    np.push(new_idx);
    Some(np)
}

fn new_node(kind: NewAemNodeType) -> AemNode {
    let uuid = Uuid::new_v4();
    match kind {
        NewAemNodeType::Panel => AemNode::Panel {
            uuid,
            name: String::new(),
            title: "New Panel".into(),
            children: vec![],
            is_page: false,
            dor_exclude: false,
            visible: true,
            is_conditional: false,
            dor_num_cols: None,
            colspan: 12,
            dor_colspan: None,
            bind_ref: None,
        },
        NewAemNodeType::Repeatable => AemNode::Repeatable {
            uuid,
            name: String::new(),
            title: "New Repeatable".into(),
            children: vec![],
            min_occur: 1,
            max_occur: 1,
            bind_ref: None,
        },
        NewAemNodeType::TextField => AemNode::TextField {
            uuid,
            name: String::new(),
            label: "New Field".into(),
            mandatory: false,
            visible: true,
            max_chars: None,
            colspan: 12,
            dor_colspan: None,
            bind_ref: None,
        },
        NewAemNodeType::NumberField => AemNode::NumberField {
            uuid,
            name: String::new(),
            label: "New Number".into(),
            mandatory: false,
            visible: true,
            colspan: 12,
            dor_colspan: None,
            bind_ref: None,
        },
        NewAemNodeType::DatePicker => AemNode::DatePicker {
            uuid,
            name: String::new(),
            label: "New Date".into(),
            mandatory: false,
            visible: true,
            colspan: 12,
            dor_colspan: None,
            bind_ref: None,
        },
        NewAemNodeType::Dropdown => AemNode::Dropdown {
            uuid,
            name: String::new(),
            label: "New Dropdown".into(),
            options: vec![],
            mandatory: false,
            visible: true,
            colspan: 12,
            dor_colspan: None,
            field_id: None,
            conditions: vec![],
            bind_ref: None,
        },
        NewAemNodeType::RadioButton => AemNode::RadioButton {
            uuid,
            name: String::new(),
            label: "New Radio".into(),
            options: vec![],
            alignment: blueprint::OptionAlignment::Vertical,
            mandatory: false,
            visible: true,
            colspan: 12,
            dor_colspan: None,
            field_id: None,
            conditions: vec![],
            bind_ref: None,
        },
        NewAemNodeType::Checkbox => AemNode::Checkbox {
            uuid,
            name: String::new(),
            label: "New Checkbox".into(),
            options: vec![],
            alignment: blueprint::OptionAlignment::Vertical,
            visible: true,
            colspan: 12,
            dor_colspan: None,
            field_id: None,
            conditions: vec![],
            bind_ref: None,
        },
        NewAemNodeType::TextDraw => AemNode::TextDraw {
            uuid,
            name: String::new(),
            content: "New text".into(),
            dor_exclude: false,
            colspan: 12,
            dor_colspan: None,
        },
        NewAemNodeType::TitleDraw => AemNode::TitleDraw {
            uuid,
            name: String::new(),
            content: "New heading".into(),
            heading_level: 3,
            colspan: 12,
            dor_colspan: None,
        },
    }
}

// ── Editable text + metadata ──────────────────────────────────────────────

/// The single primary string a node exposes for inline editing, if any.
pub fn editable_text(node: &AemNode) -> Option<String> {
    use AemNode::*;
    match node {
        Root { title, .. } | Panel { title, .. } | Repeatable { title, .. } => Some(title.clone()),
        TextField { label, .. }
        | NumberField { label, .. }
        | DatePicker { label, .. }
        | Dropdown { label, .. }
        | Checkbox { label, .. }
        | RadioButton { label, .. }
        | Custom { label, .. } => Some(label.clone()),
        TextDraw { content, .. } | TitleDraw { content, .. } => Some(content.clone()),
        _ => None,
    }
}

pub fn set_editable_text(node: &mut AemNode, text: &str) {
    use AemNode::*;
    match node {
        Root { title, .. } | Panel { title, .. } | Repeatable { title, .. } => {
            *title = text.to_string();
        }
        TextField { label, .. }
        | NumberField { label, .. }
        | DatePicker { label, .. }
        | Dropdown { label, .. }
        | Checkbox { label, .. }
        | RadioButton { label, .. }
        | Custom { label, .. } => *label = text.to_string(),
        TextDraw { content, .. } | TitleDraw { content, .. } => *content = text.to_string(),
        _ => {}
    }
}

/// Apply a metadata mutation to a node (no-op for fields it does not carry).
pub fn apply_metadata(node: &mut AemNode, meta: &AemMetadata) {
    use AemNode::*;
    match meta {
        AemMetadata::Name(v) => set_name(node, v),
        AemMetadata::Mandatory(v) => {
            if let TextField { mandatory, .. }
            | NumberField { mandatory, .. }
            | DatePicker { mandatory, .. }
            | Dropdown { mandatory, .. }
            | RadioButton { mandatory, .. }
            | Custom { mandatory, .. } = node
            {
                *mandatory = *v;
            }
        }
        AemMetadata::Visible(v) => {
            if let Panel { visible, .. }
            | TextField { visible, .. }
            | NumberField { visible, .. }
            | DatePicker { visible, .. }
            | Dropdown { visible, .. }
            | Checkbox { visible, .. }
            | RadioButton { visible, .. }
            | Custom { visible, .. } = node
            {
                *visible = *v;
            }
        }
        AemMetadata::DorExclude(v) => {
            if let Panel { dor_exclude, .. } | TextDraw { dor_exclude, .. } = node {
                *dor_exclude = *v;
            }
        }
        AemMetadata::IsPage(v) => {
            if let Panel { is_page, .. } = node {
                *is_page = *v;
            }
        }
        AemMetadata::Colspan(v) => set_colspan(node, *v),
        AemMetadata::MaxChars(v) => {
            if let TextField { max_chars, .. } = node {
                *max_chars = *v;
            }
        }
        AemMetadata::HeadingLevel(v) => {
            if let TitleDraw { heading_level, .. } = node {
                *heading_level = *v;
            }
        }
        AemMetadata::Occurrences { min, max } => {
            if let Repeatable {
                min_occur,
                max_occur,
                ..
            } = node
            {
                *min_occur = *min;
                *max_occur = *max;
            }
        }
        AemMetadata::Alignment(v) => {
            if let Checkbox { alignment, .. } | RadioButton { alignment, .. } = node {
                *alignment = *v;
            }
        }
        AemMetadata::Options(v) => {
            if let Dropdown { options, .. }
            | Checkbox { options, .. }
            | RadioButton { options, .. }
            | Custom { options, .. } = node
            {
                *options = v.clone();
            }
        }
        AemMetadata::Conditions(v) => {
            if let Dropdown { conditions, .. }
            | Checkbox { conditions, .. }
            | RadioButton { conditions, .. } = node
            {
                *conditions = v.clone();
            }
        }
        AemMetadata::FragRef(v) => {
            if let Fragment { frag_ref, .. } = node {
                *frag_ref = v.clone();
            }
        }
    }
}

/// Visibility condition rules carried by a field, if any.
pub fn node_conditions(node: &AemNode) -> Option<Vec<ConditionRule>> {
    use AemNode::*;
    match node {
        Dropdown { conditions, .. }
        | Checkbox { conditions, .. }
        | RadioButton { conditions, .. } => Some(conditions.clone()),
        _ => None,
    }
}

fn set_name(node: &mut AemNode, v: &str) {
    use AemNode::*;
    match node {
        Root { .. } => {}
        Panel { name, .. }
        | TextField { name, .. }
        | NumberField { name, .. }
        | DatePicker { name, .. }
        | Dropdown { name, .. }
        | Checkbox { name, .. }
        | RadioButton { name, .. }
        | TextDraw { name, .. }
        | TitleDraw { name, .. }
        | Repeatable { name, .. }
        | Fragment { name, .. }
        | Preface { name, .. }
        | Appendix { name, .. }
        | FootnotePlaceholder { name, .. }
        | Custom { name, .. } => *name = v.to_string(),
    }
}

fn set_colspan(node: &mut AemNode, v: u32) {
    use AemNode::*;
    match node {
        Panel { colspan, .. }
        | TextField { colspan, .. }
        | NumberField { colspan, .. }
        | DatePicker { colspan, .. }
        | Dropdown { colspan, .. }
        | Checkbox { colspan, .. }
        | RadioButton { colspan, .. }
        | TextDraw { colspan, .. }
        | TitleDraw { colspan, .. }
        | FootnotePlaceholder { colspan, .. }
        | Custom { colspan, .. } => *colspan = v,
        _ => {}
    }
}

// ── Conversion ──────────────────────────────────────────────────────────────

/// Convert a node to a different field / draw variant, preserving as much of
/// its identity (uuid, name, label, layout, options) as possible.
pub fn convert_node(node: &AemNode, target: AemConvertTarget) -> Option<AemNode> {
    let uuid = node_uuid(node)?;
    let name = node_name(node).unwrap_or_default();
    let label = editable_text(node).unwrap_or_default();
    let colspan = node_colspan(node).unwrap_or(12);
    let options = node_options(node).unwrap_or_default();
    let mandatory = node_mandatory(node).unwrap_or(false);

    Some(match target {
        AemConvertTarget::TextField => AemNode::TextField {
            uuid,
            name,
            label,
            mandatory,
            visible: true,
            max_chars: None,
            colspan,
            dor_colspan: None,
            bind_ref: None,
        },
        AemConvertTarget::NumberField => AemNode::NumberField {
            uuid,
            name,
            label,
            mandatory,
            visible: true,
            colspan,
            dor_colspan: None,
            bind_ref: None,
        },
        AemConvertTarget::DatePicker => AemNode::DatePicker {
            uuid,
            name,
            label,
            mandatory,
            visible: true,
            colspan,
            dor_colspan: None,
            bind_ref: None,
        },
        AemConvertTarget::Dropdown => AemNode::Dropdown {
            uuid,
            name,
            label,
            options,
            mandatory,
            visible: true,
            colspan,
            dor_colspan: None,
            field_id: None,
            conditions: vec![],
            bind_ref: None,
        },
        AemConvertTarget::RadioButton => AemNode::RadioButton {
            uuid,
            name,
            label,
            options,
            alignment: blueprint::OptionAlignment::Vertical,
            mandatory,
            visible: true,
            colspan,
            dor_colspan: None,
            field_id: None,
            conditions: vec![],
            bind_ref: None,
        },
        AemConvertTarget::Checkbox => AemNode::Checkbox {
            uuid,
            name,
            label,
            options,
            alignment: blueprint::OptionAlignment::Vertical,
            visible: true,
            colspan,
            dor_colspan: None,
            field_id: None,
            conditions: vec![],
            bind_ref: None,
        },
        AemConvertTarget::TextDraw => AemNode::TextDraw {
            uuid,
            name,
            content: label,
            dor_exclude: false,
            colspan,
            dor_colspan: None,
        },
        AemConvertTarget::TitleDraw => AemNode::TitleDraw {
            uuid,
            name,
            content: label,
            heading_level: 3,
            colspan,
            dor_colspan: None,
        },
    })
}

pub fn node_uuid(node: &AemNode) -> Option<Uuid> {
    use AemNode::*;
    match node {
        Root { .. } => None,
        Panel { uuid, .. }
        | TextField { uuid, .. }
        | NumberField { uuid, .. }
        | DatePicker { uuid, .. }
        | Dropdown { uuid, .. }
        | Checkbox { uuid, .. }
        | RadioButton { uuid, .. }
        | TextDraw { uuid, .. }
        | TitleDraw { uuid, .. }
        | Repeatable { uuid, .. }
        | Fragment { uuid, .. }
        | Preface { uuid, .. }
        | Appendix { uuid, .. }
        | FootnotePlaceholder { uuid, .. }
        | Custom { uuid, .. } => Some(*uuid),
    }
}

pub fn node_name(node: &AemNode) -> Option<String> {
    use AemNode::*;
    match node {
        Root { .. } => None,
        Panel { name, .. }
        | TextField { name, .. }
        | NumberField { name, .. }
        | DatePicker { name, .. }
        | Dropdown { name, .. }
        | Checkbox { name, .. }
        | RadioButton { name, .. }
        | TextDraw { name, .. }
        | TitleDraw { name, .. }
        | Repeatable { name, .. }
        | Fragment { name, .. }
        | Preface { name, .. }
        | Appendix { name, .. }
        | FootnotePlaceholder { name, .. }
        | Custom { name, .. } => Some(name.clone()),
    }
}

fn node_colspan(node: &AemNode) -> Option<u32> {
    use AemNode::*;
    match node {
        Panel { colspan, .. }
        | TextField { colspan, .. }
        | NumberField { colspan, .. }
        | DatePicker { colspan, .. }
        | Dropdown { colspan, .. }
        | Checkbox { colspan, .. }
        | RadioButton { colspan, .. }
        | TextDraw { colspan, .. }
        | TitleDraw { colspan, .. }
        | FootnotePlaceholder { colspan, .. }
        | Custom { colspan, .. } => Some(*colspan),
        _ => None,
    }
}

pub fn node_options(node: &AemNode) -> Option<Vec<AemOption>> {
    use AemNode::*;
    match node {
        Dropdown { options, .. }
        | Checkbox { options, .. }
        | RadioButton { options, .. }
        | Custom { options, .. } => Some(options.clone()),
        _ => None,
    }
}

fn node_mandatory(node: &AemNode) -> Option<bool> {
    use AemNode::*;
    match node {
        TextField { mandatory, .. }
        | NumberField { mandatory, .. }
        | DatePicker { mandatory, .. }
        | Dropdown { mandatory, .. }
        | RadioButton { mandatory, .. }
        | Custom { mandatory, .. } => Some(*mandatory),
        _ => None,
    }
}

/// Walk the tree, invoking `visit(uuid, master_label)` for every node that has
/// both a uuid and editable text. Used to seed and emit per-language labels.
pub fn for_each_labeled<F: FnMut(Uuid, &str)>(root: &AemNode, mut visit: F) {
    fn walk<F: FnMut(Uuid, &str)>(node: &AemNode, visit: &mut F) {
        if let (Some(uuid), Some(text)) = (node_uuid(node), editable_text(node)) {
            visit(uuid, &text);
        }
        if let Some(children) = children_ref(node) {
            for c in children {
                walk(c, visit);
            }
        }
    }
    walk(root, &mut visit);
}

/// Collect every selectable path in the tree (all nodes except the root).
pub fn collect_paths(root: &AemNode) -> HashSet<AemPath> {
    let mut out = HashSet::new();
    fn walk(node: &AemNode, prefix: &[usize], out: &mut HashSet<AemPath>) {
        if let Some(children) = children_ref(node) {
            for (i, child) in children.iter().enumerate() {
                let mut path = prefix.to_vec();
                path.push(i);
                out.insert(path.clone());
                walk(child, &path, out);
            }
        }
    }
    walk(root, &[], &mut out);
    out
}
