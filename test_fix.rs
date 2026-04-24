#[test]
fn test_aacx_debug_doc_structure() {
    use crate::document::{GroupKind, ListStyleType};
    use crate::document_analysis::run_default_modules;

    let mut form = crate::pdf::XfaForm::load(input_path("AACX_033_IT.pdf")).expect("load form");
    let states = crate::exhaustive::collect_states(&mut form)
        .expect("collect_states");
    let state = &states[0];
    let mut doc = crate::document::Document::from_flattened(&state.flattened);
    run_default_modules(&mut doc);

    let roots: Vec<usize> = doc.groups.iter().enumerate()
        .filter(|(_, g)| matches!(g.source, crate::document::GroupSource::Initial)) 
        .map(|(i, _)| i).collect();
    
    // Actually, I should just find the groups that are not children of any other group.
    let mut is_child = vec![false; doc.groups.len()];
    for g in &doc.groups {
        for &child in &g.children {
            if child < is_child.len() {
                is_child[child] = true;
            }
        }
    }

    for (root_idx, g) in doc.groups.iter().enumerate() {
        if is_child[root_idx] { continue; }
        if let GroupKind::List { list_style } = &g.kind {
            println!(
                "ROOT LIST idx={} style={:?} children={}",
                root_idx, list_style, g.children.len()
            );
            for (ci, &child_idx) in g.children.iter().enumerate() {
                if let Some(cg) = doc.groups.get(child_idx) {
                    match &cg.kind {
                        GroupKind::List { list_style: sub_style } => {
                            println!(
                                "  [{}] idx={} SUBLIST style={:?} children={}",
                                ci, child_idx, sub_style, cg.children.len()
                            );
                            for (si, &sub_child_idx) in cg.children.iter().enumerate() {
                                if let Some(sg) = doc.groups.get(sub_child_idx) {
                                    match &sg.kind {
                                        GroupKind::List { list_style: ss } => println!(
                                            "    [{}.{}] SUB-SUBLIST style={:?} children={}",
                                            ci, si, ss, sg.children.len()
                                        ),
                                        _ => {
                                            println!("    [{}.{}] (leaf)", ci, si);
                                        }
                                    }
                                }
                            }
                        }
                        _ => {
                           println!("  [{}] (leaf)", ci);
                        }
                    }
                }
            }
        }
    }
}
