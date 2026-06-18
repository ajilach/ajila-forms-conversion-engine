//! Graphical editor for the intermediate AEM node tree.
//!
//! Analogous to the structured editor, but edits an `AemNode` tree directly —
//! the source of truth for the generated package — and adds the Smart AEM Edit
//! flow plus on-demand upload to an AEM instance.

#[allow(clippy::module_inception)]
mod editor;
mod metadata_editor;
mod node_renderer;
pub mod smart_edit;
mod state;
mod text_editor;
mod toolbar;

pub use editor::{AemConfigWrapper, AemConnWrapper, AemEditor, AemRootWrapper};
