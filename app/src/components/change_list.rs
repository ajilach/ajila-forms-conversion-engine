//! Reusable change-list component shared by Smart Edit and Smart AEM Edit.

use std::collections::HashSet;

use dioxus::prelude::*;

/// A reviewable list of proposed changes with per-item accept/reject checkboxes.
///
/// `changes` are `(id, description)` pairs. An item is *accepted* while checked;
/// unchecking it inserts its `id` into `rejected_ids` (and re-checking removes
/// it). The rejected set is owned by the caller so it can read which changes
/// were rejected when applying or retrying. Both editors render an identical
/// list, so the markup lives here.
#[component]
pub fn ChangeList(changes: Vec<(usize, String)>, rejected_ids: Signal<HashSet<usize>>) -> Element {
    rsx! {
        div { class: "smart-edit-change-list",
            for (id , description) in changes {
                label {
                    key: "{id}",
                    class: if rejected_ids.read().contains(&id) { "smart-edit-change-item smart-edit-change-rejected" } else { "smart-edit-change-item" },
                    input {
                        r#type: "checkbox",
                        checked: !rejected_ids.read().contains(&id),
                        onchange: move |evt| {
                            if evt.checked() {
                                rejected_ids.write().remove(&id);
                            } else {
                                rejected_ids.write().insert(id);
                            }
                        },
                    }
                    span { "{description}" }
                }
            }
        }
    }
}
