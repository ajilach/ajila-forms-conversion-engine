//! The tool executor: one match over the catalog's tool names.
//!
//! Each arm is the tool's own work; the shared plumbing — the `AI:` snapshot
//! label, the "no tree yet" guard, the Ok/Err mapping — lives on the three
//! edit helpers in the parent module.

use super::*;

impl ConversionAgent {
    pub async fn execute(&mut self, name: &str, input: &serde_json::Value) -> ToolReply {
        if let Some(refusal) = self.target_refusal(name) {
            return ToolReply::Error(refusal);
        }

        match name {
            // §1 extraction
            "get_source_info" => match self.extractor(input) {
                Ok(ex) => {
                    let langs: Vec<&str> = ex.states.iter().map(|s| s.context.language()).collect();
                    // Report the cross-language merge outcome: when it fails the
                    // engine's merged tree is empty, and any output derived from
                    // it would silently be empty too.
                    let merge = match &ex.merge_error {
                        Some(e) => format!("FAILED - {e}"),
                        None => "ok".to_string(),
                    };
                    ToolReply::Text(format!(
                        "states: {}, languages: {:?}, xfa_pdfs: {}, merge: {merge}",
                        ex.states.len(),
                        dedup(langs),
                        ex.xfa.len()
                    ))
                }
                Err(e) => ToolReply::Error(e),
            },
            "list_states" => match self.extractor(input) {
                Ok(ex) => {
                    let list: Vec<_> = ex
                        .states
                        .iter()
                        .map(|s| serde_json::json!({"label": s.label, "pdf": s.pdf_name, "selections": s.selections}))
                        .collect();
                    ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
                }
                Err(e) => ToolReply::Error(e),
            },
            "get_xfa" => match self.extractor(input) {
                Ok(ex) if ex.xfa.is_empty() => {
                    ToolReply::Error("No XFA present in the source.".into())
                }
                Ok(ex) => ToolReply::Text(
                    ex.xfa
                        .iter()
                        .map(|(n, x)| format!("BEGIN XFA ({n})\n{x}\nEND XFA ({n})"))
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                ),
                Err(e) => ToolReply::Error(e),
            },
            "search_xfa" => {
                let query = input["query"].as_str().unwrap_or_default().to_string();
                let regex = input["regex"].as_bool().unwrap_or(false);
                match self.extractor(input) {
                    Ok(ex) => {
                        let mut out = String::new();
                        for (n, x) in &ex.xfa {
                            for line in x.lines().filter(|l| line_matches(l, &query, regex)) {
                                out.push_str(&format!("{n}: {}\n", line.trim()));
                                if out.len() > 4000 {
                                    break;
                                }
                            }
                        }
                        if out.is_empty() {
                            ToolReply::Text("No matches.".into())
                        } else {
                            ToolReply::Text(out)
                        }
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_plain_state_image" | "get_annotated_state_image" => {
                let label = input["state_label"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let annotated = name == "get_annotated_state_image";
                let scale = self.render_scale;
                match self.extractor(input) {
                    Ok(ex) => match ex.find(&label) {
                        Some(rec) => {
                            // Render one image per page so no single image exceeds
                            // the vision API's size limit on tall multi-page forms.
                            let pages = if annotated {
                                rec.state.render_annotated_pages(scale)
                            } else {
                                rec.state.render_plain_pages(scale)
                            };
                            match pages.map_err(|e| e.to_string()).and_then(|imgs| {
                                imgs.iter()
                                    .map(|i| {
                                        crate::image_encode::encode_rgba_to_jpeg(i, 82)
                                            .map(|jpeg| base64_encode(&jpeg))
                                            .map_err(|e| e.to_string())
                                    })
                                    .collect::<Result<Vec<String>, String>>()
                            }) {
                                Ok(images) => ToolReply::Image {
                                    media_type: "image/jpeg",
                                    images,
                                },
                                Err(e) => ToolReply::Error(format!("Render failed: {e}")),
                            }
                        }
                        None => ToolReply::Error(format!("Unknown state_label: {label:?}")),
                    },
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_flattened_structure_for_state" => {
                let label = input["state_label"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                match self.extractor(input) {
                    Ok(ex) => match ex.state_structured(&label) {
                        Ok(content) => ToolReply::Text(
                            serde_json::to_string_pretty(&content).unwrap_or_default(),
                        ),
                        Err(e) => ToolReply::Error(e),
                    },
                    Err(e) => ToolReply::Error(e),
                }
            }
            // §2a structured tree (Redacto target)
            "seed_structured_from_state" => {
                let label = input["state_label"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let seeded = match self.extractor(input) {
                    Ok(ex) => ex.state_structured(&label),
                    Err(e) => Err(e),
                };
                match seeded {
                    Ok(nodes) => {
                        let count = nodes.len();
                        self.structured = nodes;
                        self.structured_edited(&format!("AI: seed structured from {label}"));
                        ToolReply::Text(format!(
                            "OK — working structured tree seeded from '{label}' \
                             ({count} top-level nodes). Use get_structured_outline to \
                             review it, then add the other languages."
                        ))
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "set_structured" => {
                let v = input.get("nodes").cloned().unwrap_or_else(|| input.clone());
                match serde_json::from_value::<Vec<StructuredNode>>(v) {
                    Ok(nodes) => {
                        let count = nodes.len();
                        self.structured = nodes;
                        self.structured_edited("AI: set structured tree");
                        ToolReply::Text(format!(
                            "OK — working structured tree set ({count} top-level nodes)."
                        ))
                    }
                    Err(e) => ToolReply::Error(format!("Invalid StructuredNode JSON: {e}")),
                }
            }
            "get_structured_outline" => {
                if self.structured.is_empty() {
                    return ToolReply::Error(NO_STRUCTURED_TREE.into());
                }
                ToolReply::Text(crate::structured_edit::outline(&self.structured))
            }
            "get_structured_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                match crate::structured_edit::resolve_mut(&mut self.structured, &path) {
                    Ok(node) => {
                        ToolReply::Text(serde_json::to_string_pretty(node).unwrap_or_default())
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "set_structured_field" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let field = input["field"].as_str().unwrap_or_default().to_string();
                let value = input
                    .get("value")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let result =
                    crate::structured_edit::set_field(&mut self.structured, &path, &field, value);
                self.edit_structured(format_args!("set {field} on {path}"), result)
            }
            "set_structured_fields" => {
                let edits: Vec<(String, String, serde_json::Value)> = input["edits"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .map(|e| {
                                (
                                    e["path"].as_str().unwrap_or_default().to_string(),
                                    e["field"].as_str().unwrap_or_default().to_string(),
                                    e.get("value").cloned().unwrap_or(serde_json::Value::Null),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let count = edits.len();
                let result = crate::structured_edit::set_fields(&mut self.structured, &edits);
                self.edit_structured(format_args!("set {count} field(s)"), result)
            }
            "replace_structured_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let node = input
                    .get("node")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let result =
                    crate::structured_edit::replace_node(&mut self.structured, &path, node);
                self.edit_structured(format_args!("replace {path}"), result)
            }
            "insert_structured_node" => {
                let parent = input["parent_path"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let node = input
                    .get("node")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let pos = match crate::structured_edit::parse_insert_pos(
                    input.get("position").unwrap_or(&serde_json::Value::Null),
                ) {
                    Ok(p) => p,
                    Err(e) => return ToolReply::Error(e),
                };
                let result =
                    crate::structured_edit::insert_node(&mut self.structured, &parent, node, pos);
                self.edit_structured(format_args!("insert into {parent}"), result)
            }
            "remove_structured_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let result = crate::structured_edit::remove_node(&mut self.structured, &path);
                self.edit_structured(format_args!("remove {path}"), result)
            }
            "build_redacto_dump" => {
                if self.structured.is_empty() {
                    return ToolReply::Error(NO_STRUCTURED_TREE.into());
                }
                match self.build_redacto() {
                    Ok((dump, config)) => {
                        let validation = blueprint::validate_dump(&dump, &config);
                        ToolReply::Text(
                            serde_json::to_string_pretty(&serde_json::json!({
                                "document_id": config.document_id,
                                "title": config.title,
                                "languages": config.languages,
                                "header": config.header,
                                "assets": validation.counts.assets,
                                "asset_versions": validation.counts.asset_versions,
                                "document_versions": validation.counts.document_versions,
                                "rows": validation.counts.rows,
                                "asset_containers": validation.counts.asset_containers,
                                "styled_panels": validation.counts.styled_panels,
                                "problems": validation.problems,
                                "warnings": validation.warnings,
                            }))
                            .unwrap_or_default(),
                        )
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "review_redacto_output" => {
                if self.structured.is_empty() {
                    return ToolReply::Error(NO_STRUCTURED_TREE.into());
                }
                let (dump, config) = match self.build_redacto() {
                    Ok(pair) => pair,
                    Err(e) => return ToolReply::Error(e),
                };
                let source = self.source_envelope().content;
                let report = blueprint::review_redacto(&source, &dump, &config.master_language);
                ToolReply::Text(serde_json::to_string_pretty(&report).unwrap_or_default())
            }
            // §2 multilingual AEM tree (AemNodeTranslated)
            "set_aem_translated" => {
                let v = input.get("root").cloned().unwrap_or_else(|| input.clone());
                match serde_json::from_value::<AemNodeTranslated>(v) {
                    Ok(node) => {
                        if let Some(aem) = self.target.aem_mut() {
                            aem.tree = Some(node);
                        }
                        self.aem_translated_edited("AI: set AEM (translated) tree");
                        ToolReply::Text("OK — working AEM tree set (package invalidated).".into())
                    }
                    Err(e) => ToolReply::Error(format!("Invalid AemNodeTranslated JSON: {e}")),
                }
            }
            "get_aem_translated" => self.read_aem(|root| {
                ToolReply::Text(serde_json::to_string_pretty(root).unwrap_or_default())
            }),
            "get_aem_translated_outline" => {
                self.read_aem(|root| ToolReply::Text(crate::aem_translated_edit::outline(root)))
            }
            "get_aem_translated_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                self.read_aem(
                    |root| match crate::aem_translated_edit::resolve_mut(root, &path) {
                        Ok(node) => {
                            ToolReply::Text(serde_json::to_string_pretty(node).unwrap_or_default())
                        }
                        Err(e) => ToolReply::Error(e),
                    },
                )
            }
            "set_aem_translated_field" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let field = input["field"].as_str().unwrap_or_default().to_string();
                if field.is_empty() {
                    return ToolReply::Error("`field` must not be empty.".into());
                }
                let value = input
                    .get("value")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                self.edit_aem(format_args!("set {field} on {path}"), |root| {
                    crate::aem_translated_edit::set_field(root, &path, &field, value)
                })
            }
            "replace_aem_translated_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let node = input
                    .get("node")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                self.edit_aem(format_args!("replace {path}"), |root| {
                    crate::aem_translated_edit::replace_node(root, &path, node)
                })
            }
            "insert_aem_translated_node" => {
                let parent = input["parent_path"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let node = input
                    .get("node")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let pos = match crate::aem_translated_edit::parse_insert_pos(&input["position"]) {
                    Ok(p) => p,
                    Err(e) => return ToolReply::Error(e),
                };
                self.edit_aem(format_args!("insert into {parent}"), |root| {
                    crate::aem_translated_edit::insert_node(root, &parent, node, pos)
                })
            }
            "remove_aem_translated_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                self.edit_aem(format_args!("remove {path}"), |root| {
                    crate::aem_translated_edit::remove_node(root, &path)
                })
            }

            // §5 output
            "build_aem_package" => {
                let cfg = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                let (aem, translations) = match self.lower_aem_translated() {
                    Ok(pair) => pair,
                    Err(e) => return ToolReply::Error(e),
                };
                // Re-emit each loaded node's fidelity passthrough (raw attrs +
                // unmodeled children) so a template's load→edit→save round-trip
                // preserves what the typed model doesn't represent. Empty for
                // from-XFA trees, so their output is unchanged.
                let passthrough = self
                    .aem_tree()
                    .map(|t| t.passthrough_map())
                    .unwrap_or_default();
                let pkg = blueprint::to_aem_package_from_node_with_passthrough(
                    &aem,
                    &cfg,
                    translations,
                    &passthrough,
                );
                let size = pkg.len();
                if let Some(aem) = self.target.aem_mut() {
                    aem.package = Some(pkg);
                }
                ToolReply::Text(format!("Built package ({size} bytes)."))
            }
            "get_package_info" => match self.target.aem().and_then(|s| s.package.as_ref()) {
                Some(pkg) => {
                    let files = crate::references::unzip_package(pkg).unwrap_or_default();
                    let paths: Vec<&String> = files.iter().map(|(p, _)| p).collect();
                    ToolReply::Text(format!(
                        "size: {} bytes\nfiles:\n{}",
                        pkg.len(),
                        serde_json::to_string_pretty(&paths).unwrap_or_default()
                    ))
                }
                None => ToolReply::Error(NO_PACKAGE.into()),
            },
            "read_package_file" => {
                let path = input["path"].as_str().unwrap_or_default();
                match self.target.aem().and_then(|s| s.package.as_ref()) {
                    Some(pkg) => match crate::references::unzip_package(pkg) {
                        Ok(files) => match files.iter().find(|(p, _)| p == path) {
                            Some((_, c)) => ToolReply::Text(c.clone()),
                            None => ToolReply::Error(format!("No such file: {path:?}")),
                        },
                        Err(e) => ToolReply::Error(e),
                    },
                    None => ToolReply::Error(NO_PACKAGE.into()),
                }
            }
            "validate_aem_package" => {
                let Some(pkg) = self.target.aem().and_then(|s| s.package.clone()) else {
                    return ToolReply::Error(NO_PACKAGE.into());
                };
                match validate_package_bytes(&pkg) {
                    Ok(msg) => ToolReply::Text(msg),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "review_output" => {
                let (aem, _) = match self.lower_aem_translated() {
                    Ok(pair) => pair,
                    Err(e) => return ToolReply::Error(e),
                };
                let merged = match self.extractor(&serde_json::Value::Null) {
                    Ok(ex) => ex.merged.content.clone(),
                    Err(e) => return ToolReply::Error(e),
                };
                let config = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                let master = config.master_language.clone();
                let report = blueprint::review_output(&merged, &aem, &config, &master);
                ToolReply::Text(serde_json::to_string_pretty(&report).unwrap_or_default())
            }
            "generate_xsd" => {
                let p = match self.profile.clone() {
                    Some(p) if blueprint::has_xsd_config(&p) => p,
                    _ => return ToolReply::Error("This profile has no XSD config.".into()),
                };
                let mut cfg = match blueprint::load_xsd_config(&p) {
                    Ok(cfg) => cfg,
                    Err(e) => return ToolReply::Error(e),
                };
                // The AEM tree is the source of truth on an AEM run, so derive
                // the schema straight from it: that is the same tree the package
                // ships, and its bindRefs are the schema's element paths. Fall
                // back to the structured content only when no tree exists yet.
                let fragments = self
                    .config()
                    .map(|c| {
                        cfg.form_code = Some(c.form_code.clone());
                        c.fragments.clone()
                    })
                    .unwrap_or_default();

                if self.aem_translated().is_some() {
                    let (aem, _) = match self.lower_aem_translated_lenient() {
                        Ok(pair) => pair,
                        Err(e) => return ToolReply::Error(e),
                    };
                    return ToolReply::Text(blueprint::generate_xsd_string_from_aem(
                        &aem, &cfg, &fragments,
                    ));
                }

                let content = match self.derived_output_content() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                let aem_config = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                ToolReply::Text(blueprint::to_xsd(&content, &aem_config, &cfg))
            }
            "generate_html" => {
                let p = match self.profile.clone() {
                    Some(p) if blueprint::has_html_config(&p) => p,
                    _ => return ToolReply::Error("This profile has no HTML config.".into()),
                };
                let content = match self.derived_output_content() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                match blueprint::load_html_custom_styles(&p) {
                    Ok(styles) => {
                        let cfg = blueprint::HtmlConfig {
                            custom_styles: Some(styles),
                            ..blueprint::HtmlConfig::default()
                        };
                        ToolReply::Text(blueprint::to_html(&content, &cfg))
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }

            // §6 deploy + verify (network)
            "upload_to_aem" => {
                let Some(conn) = self.conn.clone() else {
                    return ToolReply::Error("No AEM connection configured.".into());
                };
                let Some(pkg) = self.target.aem().and_then(|s| s.package.clone()) else {
                    return ToolReply::Error(NO_PACKAGE.into());
                };
                let cfg = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                match crate::aem_client::upload_and_install_package(&conn, pkg, &cfg.form_code)
                    .await
                {
                    Ok(()) => {
                        if let Some(aem) = self.target.aem_mut() {
                            aem.uploaded = true;
                            aem.form_path = Some(form_jcr_path(&cfg));
                        }
                        ToolReply::Text("Uploaded and installed on AEM.".into())
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "fetch_aem_form_html" => {
                let (Some(conn), Ok(cfg)) = (self.conn.clone(), self.config()) else {
                    return ToolReply::Error("No AEM connection / profile configured.".into());
                };
                let path = form_jcr_path(&cfg);
                match crate::aem_client::fetch_form_html(&conn, &path).await {
                    Ok(html) => ToolReply::Text(truncate(&html, 8000)),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "fetch_aem_dor_pdf" => {
                let (Some(conn), Ok(cfg)) = (self.conn.clone(), self.config()) else {
                    return ToolReply::Error("No AEM connection / profile configured.".into());
                };
                let path = form_jcr_path(&cfg);
                match crate::aem_client::fetch_dor_pdf(&conn, &path).await {
                    Ok(pdf) => match render_pdf_pages(&pdf) {
                        Ok(images) => ToolReply::Image {
                            media_type: "image/jpeg",
                            images,
                        },
                        Err(e) => ToolReply::Error(e),
                    },
                    Err(e) => ToolReply::Error(e),
                }
            }

            // §7 references
            "list_reference_forms" => {
                let profile = self.profile.clone().unwrap_or_default();
                let list: Vec<_> = crate::references::list_references(&profile)
                    .into_iter()
                    .map(|r| serde_json::json!({"ref_id": r.ref_id, "label": r.label, "description": r.description, "pdf_count": r.pdf_count, "files": r.files}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
            }
            "search_references" => {
                let profile = self.profile.clone().unwrap_or_default();
                let query = input["query"].as_str().unwrap_or_default().to_string();
                if query.trim().is_empty() {
                    return ToolReply::Error(
                        "search_references requires a non-empty query — pass a description of the \
                         input form/section, not an empty string."
                            .into(),
                    );
                }
                let top_k = input["top_k"].as_u64().unwrap_or(3).max(1) as usize;
                let matcher = match self.matcher() {
                    Ok(m) => m,
                    Err(e) => return ToolReply::Error(e),
                };
                let hits: Vec<_> =
                    crate::references::search_references(&profile, &query, matcher, top_k)
                        .into_iter()
                        .map(|h| serde_json::json!({"ref_id": h.ref_id, "label": h.label, "where": h.location, "matched": h.matched, "score": h.score, "snippet": h.snippet}))
                        .collect();
                ToolReply::Text(serde_json::to_string_pretty(&hits).unwrap_or_default())
            }
            "grep_references" => {
                let profile = self.profile.clone().unwrap_or_default();
                let query = input["query"].as_str().unwrap_or_default();
                let regex = input["regex"].as_bool().unwrap_or(false);
                let hits: Vec<_> = crate::references::grep_references(&profile, query, regex)
                    .into_iter()
                    .map(|h| serde_json::json!({"ref_id": h.ref_id, "label": h.label, "where": h.location, "snippet": h.snippet}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&hits).unwrap_or_default())
            }
            "read_reference_file" => {
                let ref_id = input["ref_id"].as_str().unwrap_or_default();
                let path = input["path"].as_str().unwrap_or_default();
                let offset = input["offset"].as_u64().unwrap_or(0) as usize;
                let limit = input["limit"].as_u64().unwrap_or(0) as usize;
                match crate::references::read_reference_file(ref_id, path, offset, limit) {
                    Ok(t) => ToolReply::Text(t),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_reference_package" => {
                let ref_id = input["ref_id"].as_str().unwrap_or_default();
                let files = crate::references::get_reference_package_files(ref_id);
                let paths: Vec<&String> = files.iter().map(|(p, _)| p).collect();
                ToolReply::Text(serde_json::to_string_pretty(&paths).unwrap_or_default())
            }
            "list_reference_docs" => {
                let profile = self.profile.clone().unwrap_or_default();
                let list: Vec<_> = crate::references::list_docs(&profile)
                    .into_iter()
                    .map(|d| serde_json::json!({"doc_id": d.doc_id, "label": d.label}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
            }
            "read_reference_doc" => {
                let doc_id = input["doc_id"].as_str().unwrap_or_default();
                let offset = input["offset"].as_u64().unwrap_or(0) as usize;
                let limit = input["limit"].as_u64().unwrap_or(0) as usize;
                match crate::references::read_doc(doc_id, offset, limit) {
                    Ok(t) => ToolReply::Text(t),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "grep_reference_docs" => {
                let profile = self.profile.clone().unwrap_or_default();
                let query = input["query"].as_str().unwrap_or_default();
                let regex = input["regex"].as_bool().unwrap_or(false);
                let hits: Vec<_> = crate::references::grep_docs(&profile, query, regex)
                    .into_iter()
                    .map(|(doc_id, label, snippet)| serde_json::json!({"doc_id": doc_id, "label": label, "snippet": snippet}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&hits).unwrap_or_default())
            }

            // §8 control
            "get_schema" => {
                // Unknown/absent `kind` keeps returning the AEM schema, which is
                // what every caller predating the structured target expects.
                let schema = match input["kind"].as_str() {
                    Some("structured") => blueprint::structured_schema(),
                    _ => blueprint::aem_translated_schema(),
                };
                ToolReply::Text(serde_json::to_string_pretty(&schema).unwrap_or_default())
            }
            "get_profile_info" => match self.config() {
                Ok(c) => ToolReply::Text(format!(
                    "form_code: {}\nlanguages: {:?}\nmaster_language: {}\nform_path: {}\nform_dir: {}\nbind_to_xsd: {}\nuse_fragments: {}",
                    c.form_code,
                    c.languages,
                    c.master_language,
                    c.form_path,
                    c.form_dir,
                    c.bind_to_xsd,
                    c.use_fragments
                )),
                Err(e) => ToolReply::Error(e),
            },
            "submit_review" => {
                let approved = input["approved"].as_bool().unwrap_or(false);
                let report = input["report"].as_str().unwrap_or_default().to_string();
                self.review = Some(ReviewResult { approved, report });
                ToolReply::Text(if approved {
                    "Review recorded: approved.".into()
                } else {
                    "Review recorded: changes requested — returning to the author.".into()
                })
            }

            other => ToolReply::Error(format!("Unknown tool: {other}")),
        }
    }
}
