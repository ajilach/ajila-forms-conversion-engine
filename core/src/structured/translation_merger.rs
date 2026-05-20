//! Translation merger for combining documents in different languages.
//!
//! This module merges multiple `DocumentEnvelope`s (one per language) into a single
//! multilingual representation. Text content is stored per-language using
//! `TranslatedText` and `TranslatableString::Translated`.
//!
//! # Algorithm
//!
//! 1. Take the first language as the base tree structure.
//! 2. For each subsequent language, align its node list against the base using
//!    LCS (longest common subsequence) on `node_matches_for_similarity`.
//! 3. For matched nodes, recursively merge text content by combining translations.
//! 4. Unmatched nodes are kept with only their source language populated.

use crate::context::Context;
#[cfg(feature = "semantic-matching")]
use crate::structured::merge_engine::merge_node_lists_semantic;
use crate::structured::merge_engine::{
    fill_missing_translation_placeholders, lcs_table_with, merge_node_lists,
    node_matches_for_similarity,
};
use crate::structured::{DocumentEnvelope, SemanticCtx, StructuredNode};

/// Threshold for minimum structural similarity (0.0 to 1.0).
/// Documents must have at least this much structural overlap to be considered
/// translations of the same form.
const MIN_STRUCTURAL_SIMILARITY: f64 = 0.5;

/// Error type for translation merging failures.
#[derive(Debug, Clone)]
pub enum MergeError {
    /// Documents are too structurally different to be translations of the same form.
    InsufficientStructuralSimilarity { similarity: f64, threshold: f64 },
    /// Multiple documents have the same language code.
    DuplicateLanguage { language: String },
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::InsufficientStructuralSimilarity {
                similarity,
                threshold,
            } => {
                write!(
                    f,
                    "Documents are too different to be translations (similarity: {:.1}%, required: {:.1}%)",
                    similarity * 100.0,
                    threshold * 100.0
                )
            }
            MergeError::DuplicateLanguage { language } => {
                write!(
                    f,
                    "Cannot merge documents with duplicate language code: '{}'",
                    language
                )
            }
        }
    }
}

impl std::error::Error for MergeError {}

/// Merge multiple `DocumentEnvelope`s from different languages into one multilingual envelope.
///
/// Each envelope is expected to come from the same document in a different language.
/// The context from the first envelope is used as the base, with language set to
/// a comma-separated list of all languages.
///
/// Returns an error if the documents are too structurally different to be translations.
pub fn merge_translations(
    envelopes: Vec<DocumentEnvelope>,
    semantic: Option<&SemanticCtx>,
) -> Result<DocumentEnvelope, MergeError> {
    if envelopes.is_empty() {
        return Ok(DocumentEnvelope {
            context: Context::with_language("und"),
            content: Vec::new(),
            state_count: 1,
        });
    }

    if envelopes.len() == 1 {
        return Ok(envelopes.into_iter().next().unwrap());
    }

    // Collect all languages and check for duplicates
    let languages: Vec<String> = envelopes
        .iter()
        .map(|e| e.context.language().to_string())
        .collect();

    let mut seen_languages = std::collections::HashSet::new();
    for lang in &languages {
        if !seen_languages.insert(lang.clone()) {
            return Err(MergeError::DuplicateLanguage {
                language: lang.clone(),
            });
        }
    }

    // Log a warning if envelopes have different state counts.
    // Different state counts can occur when one language version's scripts
    // don't differentiate layouts as finely as another's. The merger handles
    // this by matching Conditional nodes by their condition values.
    {
        let first_count = envelopes[0].state_count;
        if envelopes.iter().any(|e| e.state_count != first_count) {
            let details: Vec<String> = envelopes
                .iter()
                .zip(languages.iter())
                .map(|(e, lang)| format!("{}: {}", lang, e.state_count))
                .collect();
            log::warn!(
                "Language versions have different state counts ({}). \
                 Merging by condition values.",
                details.join(", ")
            );
        }
    }

    // Validate structural similarity between all pairs
    for i in 0..envelopes.len() {
        for j in (i + 1)..envelopes.len() {
            let similarity =
                calculate_structural_similarity(&envelopes[i].content, &envelopes[j].content);
            if similarity < MIN_STRUCTURAL_SIMILARITY {
                return Err(MergeError::InsufficientStructuralSimilarity {
                    similarity,
                    threshold: MIN_STRUCTURAL_SIMILARITY,
                });
            }
        }
    }

    // Start with the first envelope as the base, preserving its context
    // (which contains XFA variables, modules, etc.).
    let mut iter = envelopes.into_iter();
    let base = iter.next().unwrap();
    let base_lang = base.context.language().to_string();
    let mut merged_content = base.content;

    // Merge each subsequent language into the base
    for envelope in iter {
        let other_lang = envelope.context.language().to_string();
        #[cfg(feature = "semantic-matching")]
        {
            if let Some(sem) = semantic {
                merged_content = merge_node_lists_semantic(
                    &merged_content,
                    &base_lang,
                    &envelope.content,
                    &other_lang,
                    sem,
                );
            } else {
                merged_content =
                    merge_node_lists(&merged_content, &base_lang, &envelope.content, &other_lang);
            }
        }
        #[cfg(not(feature = "semantic-matching"))]
        {
            let _ = &semantic; // suppress unused warning
            merged_content =
                merge_node_lists(&merged_content, &base_lang, &envelope.content, &other_lang);
        }
    }

    // Best-effort optimistic normalization: mark missing language entries explicitly.
    fill_missing_translation_placeholders(&mut merged_content, &languages, &base_lang);

    // Create merged context — start from the base context to preserve variables
    // and modules, then update the language to the combined list.
    let mut context = base.context;
    context.set_language(languages.join(","));

    Ok(DocumentEnvelope {
        context,
        content: merged_content,
        state_count: base.state_count,
    })
}

/// Calculate structural similarity between two node lists.
///
/// Returns a value between 0.0 (completely different) and 1.0 (identical structure).
/// Uses the LCS length (with relaxed node matching) as a percentage of the average list length.
///
/// The relaxed matching treats container nodes (Conditional, Group, GridLayout, Repeatable)
/// as matching by type/shape rather than requiring identical deep structure. This correctly
/// handles translation pairs where layout details differ slightly between languages.
pub fn calculate_structural_similarity(a: &[StructuredNode], b: &[StructuredNode]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let dp = lcs_table_with(a, b, node_matches_for_similarity);
    let lcs_length = dp[a.len()][b.len()] as f64;
    let avg_length = (a.len() + b.len()) as f64 / 2.0;

    lcs_length / avg_length
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::structured::merge_engine::{
        AlignedNode, merge_table, prepend_orphan_text_to_matched_paragraph,
    };
    use crate::structured::{
        ConditionalNode, FieldCondition, FieldId, FieldNode, FieldType, HeadingLevel, HeadingNode,
        InlineNode, InlineText, InputValue, ListItem, ListNode, NameValue, ParagraphNode,
        TableHeader, TableNode, TableRow, TranslatableString, TranslatedText,
    };

    fn make_envelope(lang: &str, content: Vec<StructuredNode>) -> DocumentEnvelope {
        DocumentEnvelope {
            context: Context::with_language(lang),
            content,
            state_count: 1,
        }
    }

    fn make_envelope_with_variables(
        lang: &str,
        variables: HashMap<String, String>,
        content: Vec<StructuredNode>,
    ) -> DocumentEnvelope {
        DocumentEnvelope {
            context: Context::new(lang.to_string(), variables),
            content,
            state_count: 1,
        }
    }

    #[test]
    fn test_merge_single_language_passthrough() {
        let envelope = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("de", "Hallo Welt"),
                som_path: None,
                source_name: None,
            })],
        );

        let result = merge_translations(vec![envelope], None).unwrap();
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.context.language(), "de");
    }

    #[test]
    fn test_merge_mismatched_state_counts_succeeds_with_warning() {
        // Mismatched state counts should now produce a warning (not an error)
        // and succeed with condition-based merging.
        let mut de = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("de", "Hallo"),
                som_path: None,
                source_name: None,
            })],
        );
        de.state_count = 2;

        let en = make_envelope(
            "en",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("en", "Hello"),
                som_path: None,
                source_name: None,
            })],
        );

        let result = merge_translations(vec![de, en], None);
        assert!(
            result.is_ok(),
            "Mismatched state counts should not be an error"
        );
    }

    #[test]
    fn test_merge_two_languages_identical_structure() {
        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("de", "Titel"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Hallo"),
                    som_path: None,
                    source_name: None,
                }),
            ],
        );
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("en", "Title"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Hello"),
                    som_path: None,
                    source_name: None,
                }),
            ],
        );

        let result = merge_translations(vec![de, en], None).unwrap();
        assert_eq!(result.content.len(), 2);
        assert_eq!(result.context.language(), "de,en");

        // Check heading has translated text
        if let StructuredNode::Heading(h) = &result.content[0] {
            assert_eq!(h.content.get("de").unwrap().as_plain_text(), "Titel");
            assert_eq!(h.content.get("en").unwrap().as_plain_text(), "Title");
        } else {
            panic!("Expected Heading");
        }

        // Check paragraph has translated text
        if let StructuredNode::Paragraph(p) = &result.content[1] {
            assert_eq!(p.content.get("de").unwrap().as_plain_text(), "Hallo");
            assert_eq!(p.content.get("en").unwrap().as_plain_text(), "Hello");
        } else {
            panic!("Expected Paragraph");
        }
    }

    #[test]
    fn test_merge_with_structural_mismatch_lcs() {
        // German has an extra paragraph that English doesn't have
        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Einleitung"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Nur auf Deutsch"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "field1".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("de", "Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Introduction"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "field1".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("en", "Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );

        let result = merge_translations(vec![de, en], None).unwrap();
        // Should have 3 nodes: merged paragraph, DE-only paragraph, merged field
        assert_eq!(result.content.len(), 3);

        // First paragraph should be merged
        assert!(matches!(result.content[0], StructuredNode::Paragraph(_)));
        // Second is DE-only
        assert!(matches!(result.content[1], StructuredNode::Paragraph(_)));
        // Third is the merged field
        assert!(matches!(result.content[2], StructuredNode::Field(_)));
    }

    #[test]
    fn test_merge_three_languages() {
        let de = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("de", "Hallo"),
                som_path: None,
                source_name: None,
            })],
        );
        let en = make_envelope(
            "en",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("en", "Hello"),
                som_path: None,
                source_name: None,
            })],
        );
        let fr = make_envelope(
            "fr",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("fr", "Bonjour"),
                som_path: None,
                source_name: None,
            })],
        );

        let result = merge_translations(vec![de, en, fr], None).unwrap();
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.context.language(), "de,en,fr");

        if let StructuredNode::Paragraph(p) = &result.content[0] {
            assert_eq!(p.content.get("de").unwrap().as_plain_text(), "Hallo");
            assert_eq!(p.content.get("en").unwrap().as_plain_text(), "Hello");
            assert_eq!(p.content.get("fr").unwrap().as_plain_text(), "Bonjour");
        } else {
            panic!("Expected Paragraph");
        }
    }

    #[test]
    fn test_merge_field_labels_and_options() {
        let de = make_envelope(
            "de",
            vec![StructuredNode::Field(FieldNode {
                name: "gender".into(),
                som_path: None,
                label: Some(TranslatedText::plain_with_lang("de", "Geschlecht")),
                input_type: FieldType::Radio {
                    options: vec![
                        NameValue {
                            name: TranslatableString::Plain("Männlich".to_string()),
                            value: crate::structured::InputValue::Text("M".to_string()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("Weiblich".to_string()),
                            value: crate::structured::InputValue::Text("F".to_string()),
                        },
                    ],
                },
                value: None,
                placeholder: Some(TranslatableString::Plain("Bitte wählen".to_string())),
                required: false,
            })],
        );
        let en = make_envelope(
            "en",
            vec![StructuredNode::Field(FieldNode {
                name: "gender".into(),
                som_path: None,
                label: Some(TranslatedText::plain_with_lang("en", "Gender")),
                input_type: FieldType::Radio {
                    options: vec![
                        NameValue {
                            name: TranslatableString::Plain("Male".to_string()),
                            value: crate::structured::InputValue::Text("M".to_string()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("Female".to_string()),
                            value: crate::structured::InputValue::Text("F".to_string()),
                        },
                    ],
                },
                value: None,
                placeholder: Some(TranslatableString::Plain("Please select".to_string())),
                required: false,
            })],
        );

        let result = merge_translations(vec![de, en], None).unwrap();
        assert_eq!(result.content.len(), 1);

        if let StructuredNode::Field(f) = &result.content[0] {
            // Check label is merged
            let label = f.label.as_ref().unwrap();
            assert_eq!(label.get("de").unwrap().as_plain_text(), "Geschlecht");
            assert_eq!(label.get("en").unwrap().as_plain_text(), "Gender");

            // Check placeholder is merged
            if let Some(TranslatableString::Translated(map)) = &f.placeholder {
                assert_eq!(map.get("de").unwrap().as_deref(), Some("Bitte wählen"));
                assert_eq!(map.get("en").unwrap().as_deref(), Some("Please select"));
            } else {
                panic!("Expected translated placeholder");
            }

            // Check radio option names are merged
            if let FieldType::Radio { options } = &f.input_type {
                if let TranslatableString::Translated(map) = &options[0].name {
                    assert_eq!(map.get("de").unwrap().as_deref(), Some("Männlich"));
                    assert_eq!(map.get("en").unwrap().as_deref(), Some("Male"));
                } else {
                    panic!("Expected translated option name");
                }
            } else {
                panic!("Expected Radio field type");
            }
        } else {
            panic!("Expected Field");
        }
    }

    #[test]
    fn test_merge_empty() {
        let result = merge_translations(vec![], None).unwrap();
        assert!(result.content.is_empty());
    }

    #[test]
    fn test_reject_completely_different_documents() {
        // Create two completely different documents
        let doc1 = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("de", "Formular A"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "field1".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("de", "Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );
        let doc2 = make_envelope(
            "en",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Completely different"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Table(TableNode {
                    header: None,
                    rows: vec![],
                    caption: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "different_field".into(),
                    som_path: None,
                    label: None,
                    input_type: FieldType::Bool,
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );

        // Should fail with InsufficientStructuralSimilarity
        let result = merge_translations(vec![doc1, doc2], None);
        assert!(result.is_err());
        if let Err(MergeError::InsufficientStructuralSimilarity {
            similarity,
            threshold,
        }) = result
        {
            assert!(similarity < threshold);
            assert_eq!(threshold, MIN_STRUCTURAL_SIMILARITY);
        } else {
            panic!("Expected InsufficientStructuralSimilarity error");
        }
    }

    #[test]
    fn test_reject_partially_different_documents() {
        // Create documents with some overlap but not enough
        let doc1 = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("de", "Title"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Text 1"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Text 2"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Text 3"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Text 4"),
                    som_path: None,
                    source_name: None,
                }),
            ],
        );
        let doc2 = make_envelope(
            "en",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("en", "Title"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "field1".into(),
                    som_path: None,
                    label: None,
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
                StructuredNode::Field(FieldNode {
                    name: "field2".into(),
                    som_path: None,
                    label: None,
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
                StructuredNode::Field(FieldNode {
                    name: "field3".into(),
                    som_path: None,
                    label: None,
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
                StructuredNode::Field(FieldNode {
                    name: "field4".into(),
                    som_path: None,
                    label: None,
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );

        // Should fail - only 1 out of 5 nodes match (20%)
        let result = merge_translations(vec![doc1, doc2], None);
        assert!(result.is_err());
    }

    #[test]
    fn test_accept_similar_documents() {
        // Create documents with good structural overlap
        let doc1 = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("de", "Formular"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Beschreibung"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "name".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("de", "Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
                StructuredNode::Field(FieldNode {
                    name: "email".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("de", "E-Mail")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );
        let doc2 = make_envelope(
            "en",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("en", "Form"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Description"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "name".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("en", "Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
                StructuredNode::Field(FieldNode {
                    name: "email".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("en", "Email")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );

        // Should succeed - 100% match
        let result = merge_translations(vec![doc1, doc2], None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_reject_duplicate_languages() {
        // Create two documents with the same language code
        let doc1 = make_envelope(
            "en",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("en", "First document"),
                som_path: None,
                source_name: None,
            })],
        );
        let doc2 = make_envelope(
            "en",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("en", "Second document"),
                som_path: None,
                source_name: None,
            })],
        );

        // Should fail with DuplicateLanguage error
        let result = merge_translations(vec![doc1, doc2], None);
        assert!(result.is_err());
        if let Err(MergeError::DuplicateLanguage { language }) = result {
            assert_eq!(language, "en");
        } else {
            panic!("Expected DuplicateLanguage error");
        }
    }

    #[test]
    fn test_reject_duplicate_languages_among_three() {
        // Create three documents where two have the same language
        let doc1 = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("de", "German"),
                som_path: None,
                source_name: None,
            })],
        );
        let doc2 = make_envelope(
            "en",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("en", "English"),
                som_path: None,
                source_name: None,
            })],
        );
        let doc3 = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("de", "Another German"),
                som_path: None,
                source_name: None,
            })],
        );

        // Should fail with DuplicateLanguage error
        let result = merge_translations(vec![doc1, doc2, doc3], None);
        assert!(result.is_err());
        if let Err(MergeError::DuplicateLanguage { language }) = result {
            assert_eq!(language, "de");
        } else {
            panic!("Expected DuplicateLanguage error");
        }
    }

    #[test]
    fn test_merge_preserves_context_variables() {
        let vars: HashMap<String, String> = [
            ("formrange_code".to_string(), "AAAI".to_string()),
            ("formrange_entity".to_string(), "019".to_string()),
        ]
        .into_iter()
        .collect();

        let de = make_envelope_with_variables(
            "de",
            vars.clone(),
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("de", "Hallo"),
                som_path: None,
                source_name: None,
            })],
        );
        let en = make_envelope_with_variables(
            "en",
            vars.clone(),
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("en", "Hello"),
                som_path: None,
                source_name: None,
            })],
        );

        let merged = merge_translations(vec![de, en], None).unwrap();
        assert_eq!(merged.context.language(), "de,en");
        assert_eq!(merged.context.get_variable("formrange_code"), Some("AAAI"));
        assert_eq!(merged.context.get_variable("formrange_entity"), Some("019"));
    }

    #[test]
    fn test_accept_documents_with_differing_conditional_and_gridlayout_structure() {
        // Regression test for: similarity check was rejecting translation pairs where
        // Conditionals had different internal structure or GridLayouts had different
        // element counts but the same column count.
        //
        // Synthetic structure mirroring AACC_019 DE vs EN:
        //   DE: H1, H2, Field(shared), H2, Para, Cond, Cond, Cond, Cond, H2, GridLayout(12, 4 elems)
        //   EN: H1, H2, Field(shared), Para, Cond, Cond, Cond, Cond, H2, GridLayout(12, 2 elems)
        use crate::structured::{
            ConditionalNode, FieldCondition, FieldId, FieldType, GridLayout, GridLayoutElement,
            GroupNode, InputValue,
        };

        let shared_field = FieldNode {
            name: "shared_field".into(),
            som_path: None,
            label: Some(TranslatedText::plain_with_lang("de", "Shared")),
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: None,
            required: false,
        };

        let dummy_condition = FieldCondition {
            field_name: FieldId::from("some_field"),
            value: InputValue::Text("yes".to_string()),
        };

        // DE Conditional wraps a Group with 3 children.
        let de_cond = || {
            StructuredNode::Conditional(ConditionalNode {
                condition: dummy_condition.clone(),
                content: Box::new(StructuredNode::Group(GroupNode {
                    children: vec![
                        StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("de", "Absatz 1"),
                            som_path: None,
                            source_name: None,
                        }),
                        StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("de", "Absatz 2"),
                            som_path: None,
                            source_name: None,
                        }),
                        StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("de", "Absatz 3"),
                            som_path: None,
                            source_name: None,
                        }),
                    ],
                })),
            })
        };

        // EN Conditional wraps a Group with 2 children (structurally different from DE).
        let en_cond = || {
            StructuredNode::Conditional(ConditionalNode {
                condition: dummy_condition.clone(),
                content: Box::new(StructuredNode::Group(GroupNode {
                    children: vec![
                        StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("en", "Paragraph 1"),
                            som_path: None,
                            source_name: None,
                        }),
                        StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("en", "Paragraph 2"),
                            som_path: None,
                            source_name: None,
                        }),
                    ],
                })),
            })
        };

        let de_grid = StructuredNode::GridLayout(GridLayout {
            columns: 12,
            elements: vec![
                GridLayoutElement {
                    span: 3,
                    node: StructuredNode::Empty,
                },
                GridLayoutElement {
                    span: 3,
                    node: StructuredNode::Empty,
                },
                GridLayoutElement {
                    span: 3,
                    node: StructuredNode::Empty,
                },
                GridLayoutElement {
                    span: 3,
                    node: StructuredNode::Empty,
                },
            ],
        });

        // EN has 2 elements instead of 4 — different count, same column count.
        let en_grid = StructuredNode::GridLayout(GridLayout {
            columns: 12,
            elements: vec![
                GridLayoutElement {
                    span: 6,
                    node: StructuredNode::Empty,
                },
                GridLayoutElement {
                    span: 6,
                    node: StructuredNode::Empty,
                },
            ],
        });

        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("de", "Formular"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("de", "Abschnitt"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(shared_field.clone()),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("de", "Hinweis"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Erklärung"),
                    som_path: None,
                    source_name: None,
                }),
                de_cond(),
                de_cond(),
                de_cond(),
                de_cond(),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("de", "Unterschrift"),
                    som_path: None,
                    source_name: None,
                }),
                de_grid,
            ],
        );

        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("en", "Form"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("en", "Section"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(shared_field.clone()),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Instruction"),
                    som_path: None,
                    source_name: None,
                }),
                en_cond(),
                en_cond(),
                en_cond(),
                en_cond(),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("en", "Signature"),
                    som_path: None,
                    source_name: None,
                }),
                en_grid,
            ],
        );

        // Should succeed: relaxed similarity check recognises Conditionals and
        // GridLayouts with the same column count as structurally compatible.
        let result = merge_translations(vec![de, en], None);
        assert!(
            result.is_ok(),
            "Expected merge to succeed for documents with differing Conditional/GridLayout internals, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_merge_table_caption_only_in_other_language() {
        // Base (de) has a table with no caption; other (en) has a caption.
        // The caption from the other language should be preserved, not dropped.
        let base = TableNode {
            header: None,
            rows: vec![TableRow {
                cells: vec![StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Zelle"),
                    som_path: None,
                    source_name: None,
                })],
            }],
            caption: None,
        };
        let other = TableNode {
            header: None,
            rows: vec![TableRow {
                cells: vec![StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Cell"),
                    som_path: None,
                    source_name: None,
                })],
            }],
            caption: Some(TranslatedText::plain_with_lang("en", "My Table")),
        };

        let merged = merge_table(&base, "de", &other, "en");
        assert!(
            merged.caption.is_some(),
            "Caption from 'en' should not be dropped when base has None"
        );
    }

    #[test]
    fn test_merge_table_header_only_in_other_language() {
        // Base (de) has no header; other (en) has a header.
        // The header should be preserved, not dropped.
        let base = TableNode {
            header: None,
            rows: vec![TableRow {
                cells: vec![StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Zelle"),
                    som_path: None,
                    source_name: None,
                })],
            }],
            caption: None,
        };
        let other = TableNode {
            header: Some(TableHeader {
                cells: vec![StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Column"),
                    som_path: None,
                    source_name: None,
                })],
            }),
            rows: vec![TableRow {
                cells: vec![StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Cell"),
                    som_path: None,
                    source_name: None,
                })],
            }],
            caption: None,
        };

        let merged = merge_table(&base, "de", &other, "en");
        assert!(
            merged.header.is_some(),
            "Header from 'en' should not be dropped when base has None"
        );
    }

    // =========================================================================
    // Regression tests for zip-truncation bug (asymmetric collection counts)
    // =========================================================================

    #[test]
    fn test_merge_grid_layout_asymmetric_element_count_preserves_all() {
        // DE has 4 grid elements, EN has 2.  Before the fix, merge_node used .zip()
        // which silently drops the DE elements at index 2 and 3.
        use crate::structured::{GridLayout, GridLayoutElement};
        let de = make_envelope(
            "de",
            vec![StructuredNode::GridLayout(GridLayout {
                columns: 12,
                elements: vec![
                    GridLayoutElement {
                        span: 3,
                        node: StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("de", "A"),
                            som_path: None,
                            source_name: None,
                        }),
                    },
                    GridLayoutElement {
                        span: 3,
                        node: StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("de", "B"),
                            som_path: None,
                            source_name: None,
                        }),
                    },
                    GridLayoutElement {
                        span: 3,
                        node: StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("de", "C"),
                            som_path: None,
                            source_name: None,
                        }),
                    },
                    GridLayoutElement {
                        span: 3,
                        node: StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("de", "D"),
                            som_path: None,
                            source_name: None,
                        }),
                    },
                ],
            })],
        );
        let en = make_envelope(
            "en",
            vec![StructuredNode::GridLayout(GridLayout {
                columns: 12,
                elements: vec![
                    GridLayoutElement {
                        span: 6,
                        node: StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("en", "X"),
                            som_path: None,
                            source_name: None,
                        }),
                    },
                    GridLayoutElement {
                        span: 6,
                        node: StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("en", "Y"),
                            som_path: None,
                            source_name: None,
                        }),
                    },
                ],
            })],
        );

        let result = merge_translations(vec![de, en], None).unwrap();
        assert_eq!(
            result.content.len(),
            1,
            "Asymmetric grids must be merged into a single node, got {}",
            result.content.len()
        );
        if let StructuredNode::GridLayout(g) = &result.content[0] {
            assert_eq!(
                g.elements.len(),
                4,
                "All 4 DE elements must be preserved, got {}",
                g.elements.len()
            );
        } else {
            panic!("Expected GridLayout");
        }
    }

    #[test]
    fn test_merge_list_asymmetric_item_count_preserves_all() {
        // DE has 3 list items, EN has 2.  Before the fix the third DE item was dropped.
        let de = make_envelope(
            "de",
            vec![StructuredNode::List(ListNode {
                list_style: crate::document::ListStyleType::Disc,
                items: vec![
                    ListItem::simple(TranslatedText::plain_with_lang("de", "Eins")),
                    ListItem::simple(TranslatedText::plain_with_lang("de", "Zwei")),
                    ListItem::simple(TranslatedText::plain_with_lang("de", "Drei")),
                ],
            })],
        );
        let en = make_envelope(
            "en",
            vec![StructuredNode::List(ListNode {
                list_style: crate::document::ListStyleType::Disc,
                items: vec![
                    ListItem::simple(TranslatedText::plain_with_lang("en", "One")),
                    ListItem::simple(TranslatedText::plain_with_lang("en", "Two")),
                ],
            })],
        );

        let result = merge_translations(vec![de, en], None).unwrap();
        assert_eq!(
            result.content.len(),
            1,
            "Asymmetric lists must be merged into a single node, got {}",
            result.content.len()
        );
        if let StructuredNode::List(l) = &result.content[0] {
            assert_eq!(
                l.items.len(),
                3,
                "All 3 items must be preserved, got {}",
                l.items.len()
            );
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_unmatched_list_item_with_prefixed_translated_text_keeps_full_content() {
        let de = make_envelope(
            "de",
            vec![StructuredNode::List(ListNode {
                list_style: crate::document::ListStyleType::Disc,
                items: vec![ListItem::simple(TranslatedText::single("de", InlineText(vec![
                    InlineNode::Text("Prefix ".to_string()),
                    InlineNode::Strong(Box::new(InlineNode::Text("Suffix".to_string()))),
                ])))],
            })],
        );

        let en = make_envelope(
            "en",
            vec![StructuredNode::List(ListNode {
                list_style: crate::document::ListStyleType::Disc,
                items: vec![],
            })],
        );

        let result = merge_translations(vec![de, en], None).unwrap();

        let list = match &result.content[0] {
            StructuredNode::List(list) => list,
            _ => panic!("Expected list node"),
        };

        assert_eq!(list.items[0].content.get("de").unwrap().as_plain_text(), "Prefix Suffix");
        assert!(
            list.items[0]
                .content
                .get("en")
                .is_some_and(|t| t.as_plain_text().is_empty()),
            "Unmatched EN translation should exist as an explicit empty placeholder"
        );
    }

    #[test]
    fn test_merge_radio_options_asymmetric_count_preserves_all() {
        // DE has 3 radio options, EN has 2.  Before the fix the third option was dropped.
        let de = make_envelope(
            "de",
            vec![StructuredNode::Field(FieldNode {
                name: "q".into(),
                som_path: None,
                label: None,
                input_type: FieldType::Radio {
                    options: vec![
                        NameValue {
                            name: TranslatableString::Plain("Ja".into()),
                            value: crate::structured::InputValue::Text("Y".into()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("Nein".into()),
                            value: crate::structured::InputValue::Text("N".into()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("Enthaltung".into()),
                            value: crate::structured::InputValue::Text("A".into()),
                        },
                    ],
                },
                value: None,
                placeholder: None,
                required: false,
            })],
        );
        let en = make_envelope(
            "en",
            vec![StructuredNode::Field(FieldNode {
                name: "q".into(),
                som_path: None,
                label: None,
                input_type: FieldType::Radio {
                    options: vec![
                        NameValue {
                            name: TranslatableString::Plain("Yes".into()),
                            value: crate::structured::InputValue::Text("Y".into()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("No".into()),
                            value: crate::structured::InputValue::Text("N".into()),
                        },
                    ],
                },
                value: None,
                placeholder: None,
                required: false,
            })],
        );

        let result = merge_translations(vec![de, en], None).unwrap();
        if let StructuredNode::Field(f) = &result.content[0] {
            if let FieldType::Radio { options } = &f.input_type {
                assert_eq!(
                    options.len(),
                    3,
                    "All 3 options must be preserved, got {}",
                    options.len()
                );
                // The third option should carry DE text and explicit EN placeholder.
                if let TranslatableString::Translated(map) = &options[2].name {
                    assert_eq!(map.get("de").unwrap().as_deref(), Some("Enthaltung"));
                    assert_eq!(map.get("en").and_then(|o| o.as_deref()), None);
                } else {
                    panic!("Expected translated option name for third entry");
                }
            } else {
                panic!("Expected Radio");
            }
        } else {
            panic!("Expected Field");
        }
    }

    #[test]
    fn test_merge_table_row_asymmetric_count_preserves_all() {
        // DE table has 3 rows, EN has 2.  Before the fix the third row was dropped.
        let de = make_envelope(
            "de",
            vec![StructuredNode::Table(TableNode {
                header: None,
                rows: vec![
                    TableRow {
                        cells: vec![StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("de", "R1"),
                            som_path: None,
                            source_name: None,
                        })],
                    },
                    TableRow {
                        cells: vec![StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("de", "R2"),
                            som_path: None,
                            source_name: None,
                        })],
                    },
                    TableRow {
                        cells: vec![StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("de", "R3"),
                            som_path: None,
                            source_name: None,
                        })],
                    },
                ],
                caption: None,
            })],
        );
        let en = make_envelope(
            "en",
            vec![StructuredNode::Table(TableNode {
                header: None,
                rows: vec![
                    TableRow {
                        cells: vec![StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("en", "Row1"),
                            som_path: None,
                            source_name: None,
                        })],
                    },
                    TableRow {
                        cells: vec![StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang("en", "Row2"),
                            som_path: None,
                            source_name: None,
                        })],
                    },
                ],
                caption: None,
            })],
        );

        let result = merge_translations(vec![de, en], None).unwrap();
        assert_eq!(
            result.content.len(),
            1,
            "Asymmetric tables must be merged into a single node, got {}",
            result.content.len()
        );
        if let StructuredNode::Table(t) = &result.content[0] {
            assert_eq!(
                t.rows.len(),
                3,
                "All 3 rows must be preserved, got {}",
                t.rows.len()
            );
        } else {
            panic!("Expected Table");
        }
    }

    #[test]
    fn test_merge_unmatched_nodes_are_tagged_with_source_language() {
        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Gemeinsam"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("de", "Nur Deutsch"),
                    som_path: None,
                    source_name: None,
                }),
            ],
        );
        let en = make_envelope(
            "en",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("en", "Shared"),
                som_path: None,
                source_name: None,
            })],
        );

        let result = merge_translations(vec![de, en], None).unwrap();
        assert_eq!(result.content.len(), 2);

        if let StructuredNode::Heading(heading) = &result.content[1] {
            assert_eq!(heading.content.get("de").unwrap().as_plain_text(), "Nur Deutsch");
            assert!(
                heading
                    .content
                    .get("en")
                    .is_some_and(|t| t.as_plain_text().is_empty()),
                "Unmatched DE-only heading should carry an explicit empty EN placeholder"
            );
        } else {
            panic!("Expected unmatched node to remain a Heading");
        }
    }

    #[test]
    fn test_orphan_paragraph_absorption_does_not_drop_formatted_content() {
        let de = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::single("de", InlineText(vec![InlineNode::Strong(Box::new(InlineNode::Text(
                    "Basis".to_string(),
                )))])),
                som_path: None,
                source_name: None,
            })],
        );
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Intro"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::single("en", InlineText(vec![InlineNode::Strong(Box::new(InlineNode::Text(
                        "Other".to_string(),
                    )))])),
                    som_path: None,
                    source_name: None,
                }),
            ],
        );

        let result = merge_translations(vec![de, en], None).unwrap();
        assert_eq!(
            result.content.len(),
            1,
            "The orphan EN paragraph should be absorbed into the matched paragraph"
        );

        if let StructuredNode::Paragraph(paragraph) = &result.content[0] {
            assert_eq!(paragraph.content.plain_text_in("de"), "Basis");
            assert_eq!(paragraph.content.plain_text_in("en"), "Intro Other");
            assert!(!paragraph.content.0.is_empty());
        } else {
            panic!("Expected merged paragraph");
        }
    }

    #[test]
    fn test_orphan_paragraph_absorption_preserves_start_order_with_formatted_prefix() {
        let de = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::single("de", InlineText(vec![
                    InlineNode::Strong(Box::new(InlineNode::Text("Basis".to_string()))),
                    InlineNode::Text(" Ende".to_string()),
                ])),
                som_path: None,
                source_name: None,
            })],
        );
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Intro "),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::single("en", InlineText(vec![
                        InlineNode::Strong(Box::new(InlineNode::Text("Other".to_string()))),
                        InlineNode::Text(" tail".to_string()),
                    ])),
                    som_path: None,
                    source_name: None,
                }),
            ],
        );

        let result = merge_translations(vec![de, en], None).unwrap();
        assert_eq!(result.content.len(), 1);

        if let StructuredNode::Paragraph(paragraph) = &result.content[0] {
            assert_eq!(paragraph.content.plain_text_in("de"), "Basis Ende");
            assert_eq!(
                paragraph.content.plain_text_in("en"),
                "Intro Other tail",
                "Absorbed orphan text must stay at the beginning of the rendered paragraph"
            );
            assert!(!paragraph.content.0.is_empty());
        } else {
            panic!("Expected merged paragraph");
        }
    }

    #[test]
    fn test_orphan_paragraph_absorption_preserves_multiple_orphan_order() {
        let de = make_envelope(
            "de",
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain_with_lang("de", "Basis"),
                som_path: None,
                source_name: None,
            })],
        );
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "First "),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Second "),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Other"),
                    som_path: None,
                    source_name: None,
                }),
            ],
        );

        let result = merge_translations(vec![de, en], None).unwrap();
        assert_eq!(result.content.len(), 1);

        if let StructuredNode::Paragraph(paragraph) = &result.content[0] {
            assert_eq!(paragraph.content.plain_text_in("en"), "First Second Other");
        } else {
            panic!("Expected merged paragraph");
        }
    }

    #[test]
    fn test_prepend_orphan_seeds_missing_language_keys_on_existing_prefix_node() {
        let mut entry = AlignedNode::Matched(StructuredNode::Paragraph(ParagraphNode {
            content: TranslatedText::single("en", InlineText::plain("Other")),
            som_path: None,
            source_name: None,
        }));

        assert!(prepend_orphan_text_to_matched_paragraph(
            &mut entry, "Intro ", "en", "de", "en",
        ));

        if let AlignedNode::Matched(StructuredNode::Paragraph(paragraph)) = &entry {
            assert_eq!(paragraph.content.plain_text_in("en"), "Intro Other");
            assert_eq!(
                paragraph
                    .content
                    .get("de")
                    .map(|t| t.as_plain_text())
                    .unwrap_or_default(),
                "",
                "Local helper keeps empty key; final normalization fills placeholders"
            );
        } else {
            panic!("Expected matched paragraph entry");
        }
    }

    #[test]
    fn test_unmatched_option_gets_missing_translation_placeholder() {
        let de = make_envelope(
            "de",
            vec![StructuredNode::Field(FieldNode {
                name: crate::structured::FieldId::from("gender"),
                som_path: None,
                label: None,
                input_type: FieldType::Select {
                    options: vec![
                        NameValue {
                            name: TranslatableString::Plain("Ja".into()),
                            value: crate::structured::InputValue::Text("yes".into()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("Nein".into()),
                            value: crate::structured::InputValue::Text("no".into()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("Vielleicht".into()),
                            value: crate::structured::InputValue::Text("maybe".into()),
                        },
                    ],
                },
                value: None,
                placeholder: None,
                required: false,
            })],
        );

        let en = make_envelope(
            "en",
            vec![StructuredNode::Field(FieldNode {
                name: crate::structured::FieldId::from("gender"),
                som_path: None,
                label: None,
                input_type: FieldType::Select {
                    options: vec![
                        NameValue {
                            name: TranslatableString::Plain("Yes".into()),
                            value: crate::structured::InputValue::Text("yes".into()),
                        },
                        NameValue {
                            name: TranslatableString::Plain("No".into()),
                            value: crate::structured::InputValue::Text("no".into()),
                        },
                    ],
                },
                value: None,
                placeholder: None,
                required: false,
            })],
        );

        let result = merge_translations(vec![de, en], None).unwrap();

        let field = match &result.content[0] {
            StructuredNode::Field(field) => field,
            _ => panic!("Expected field node"),
        };

        let options = match &field.input_type {
            FieldType::Select { options } => options,
            _ => panic!("Expected select field"),
        };

        let unmatched = &options[2];
        match &unmatched.name {
            TranslatableString::Translated(map) => {
                assert_eq!(map.get("de").and_then(|o| o.as_deref()), Some("Vielleicht"));
                assert_eq!(map.get("en").and_then(|o| o.as_deref()), None);
            }
            _ => panic!("Expected translated name map"),
        }
    }

    // =========================================================================
    // 3-language multi-way alignment tests
    // =========================================================================

    /// Helper: assert that a paragraph node contains expected translations.
    fn assert_paragraph_translations(node: &StructuredNode, expected: &[(&str, &str)]) {
        if let StructuredNode::Paragraph(p) = node {
            for &(lang, text) in expected {
                assert_eq!(
                    p.content.get(lang).unwrap().as_plain_text(),
                    text,
                    "Expected '{}' for lang '{}'",
                    text,
                    lang
                );
            }
        } else {
            panic!("Expected Paragraph, got {:?}", node);
        }
    }

    #[test]
    fn test_three_language_permutation_invariance_text() {
        // Merging DE, EN, FR in all 6 input orderings must produce the same
        // set of language keys and text values in every paragraph.
        let make = |lang: &str, texts: &[&str]| {
            make_envelope(
                lang,
                texts
                    .iter()
                    .map(|t| {
                        StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang(lang, *t),
                            som_path: None,
                            source_name: None,
                        })
                    })
                    .collect(),
            )
        };

        let de = make("de", &["Titel", "Inhalt"]);
        let en = make("en", &["Title", "Content"]);
        let fr = make("fr", &["Titre", "Contenu"]);

        let permutations: Vec<Vec<DocumentEnvelope>> = vec![
            vec![de.clone(), en.clone(), fr.clone()],
            vec![de.clone(), fr.clone(), en.clone()],
            vec![en.clone(), de.clone(), fr.clone()],
            vec![en.clone(), fr.clone(), de.clone()],
            vec![fr.clone(), de.clone(), en.clone()],
            vec![fr.clone(), en.clone(), de.clone()],
        ];

        for (i, perm) in permutations.into_iter().enumerate() {
            let result = merge_translations(perm, None).unwrap();
            assert_eq!(
                result.content.len(),
                2,
                "permutation {i}: expected 2 nodes, got {}",
                result.content.len()
            );

            assert_paragraph_translations(
                &result.content[0],
                &[("de", "Titel"), ("en", "Title"), ("fr", "Titre")],
            );
            assert_paragraph_translations(
                &result.content[1],
                &[("de", "Inhalt"), ("en", "Content"), ("fr", "Contenu")],
            );
        }
    }

    #[test]
    fn test_three_language_majority_structure_wins() {
        // DE and EN share [H1, Para, Field].  FR has [H1, Field] (no Para).
        // The majority structure (2 of 3) should be preserved.
        let shared_field = FieldNode {
            name: "f1".into(),
            som_path: None,
            label: Some(TranslatedText::plain_with_lang("de", "lbl")),
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: None,
            required: false,
        };

        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("de", "Titel"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Beschreibung"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(shared_field.clone()),
            ],
        );
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("en", "Title"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Description"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(shared_field.clone()),
            ],
        );
        let fr = make_envelope(
            "fr",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H1,
                    content: TranslatedText::plain_with_lang("fr", "Titre"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(shared_field),
            ],
        );

        for envelopes in [
            vec![de.clone(), en.clone(), fr.clone()],
            vec![fr.clone(), de.clone(), en.clone()],
        ] {
            let result = merge_translations(envelopes, None).unwrap();

            let has_para = result
                .content
                .iter()
                .any(|n| matches!(n, StructuredNode::Paragraph(_)));
            assert!(has_para, "Majority paragraph must survive in merged output");

            let has_heading = result
                .content
                .iter()
                .any(|n| matches!(n, StructuredNode::Heading(_)));
            let has_field = result
                .content
                .iter()
                .any(|n| matches!(n, StructuredNode::Field(_)));
            assert!(has_heading, "Heading must be present");
            assert!(has_field, "Field must be present");
        }
    }

    #[test]
    fn test_three_language_union_unique_content() {
        // Each language has a shared paragraph plus a unique heading.
        // The union must contain text from all three languages.
        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("de", "Gemeinsam"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("de", "Nur DE"),
                    som_path: None,
                    source_name: None,
                }),
            ],
        );
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("en", "Shared"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("en", "Only EN"),
                    som_path: None,
                    source_name: None,
                }),
            ],
        );
        let fr = make_envelope(
            "fr",
            vec![
                StructuredNode::Paragraph(ParagraphNode {
                    content: TranslatedText::plain_with_lang("fr", "Commun"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("fr", "Seulement FR"),
                    som_path: None,
                    source_name: None,
                }),
            ],
        );

        let result = merge_translations(vec![de, en, fr], None).unwrap();

        assert_paragraph_translations(
            &result.content[0],
            &[("de", "Gemeinsam"), ("en", "Shared"), ("fr", "Commun")],
        );

        let heading_count = result
            .content
            .iter()
            .filter(|n| matches!(n, StructuredNode::Heading(_)))
            .count();
        assert_eq!(
            heading_count, 1,
            "Exactly one merged heading expected, got {heading_count}"
        );
    }

    #[test]
    fn test_three_language_field_options_all_present() {
        // Radio field with 2 options in DE+EN but 3 in FR.
        // After merge, all 3 options must be present with all language keys.
        let mk_opts = |names: &[&str]| -> Vec<NameValue> {
            names
                .iter()
                .enumerate()
                .map(|(i, n)| NameValue {
                    name: TranslatableString::Plain(n.to_string()),
                    value: InputValue::Text(format!("v{i}")),
                })
                .collect()
        };

        let de = make_envelope(
            "de",
            vec![StructuredNode::Field(FieldNode {
                name: "q".into(),
                som_path: None,
                label: None,
                input_type: FieldType::Radio {
                    options: mk_opts(&["Ja", "Nein"]),
                },
                value: None,
                placeholder: None,
                required: false,
            })],
        );
        let en = make_envelope(
            "en",
            vec![StructuredNode::Field(FieldNode {
                name: "q".into(),
                som_path: None,
                label: None,
                input_type: FieldType::Radio {
                    options: mk_opts(&["Yes", "No"]),
                },
                value: None,
                placeholder: None,
                required: false,
            })],
        );
        let fr = make_envelope(
            "fr",
            vec![StructuredNode::Field(FieldNode {
                name: "q".into(),
                som_path: None,
                label: None,
                input_type: FieldType::Radio {
                    options: mk_opts(&["Oui", "Non", "Abstention"]),
                },
                value: None,
                placeholder: None,
                required: false,
            })],
        );

        let result = merge_translations(vec![de, en, fr], None).unwrap();
        let field = match &result.content[0] {
            StructuredNode::Field(f) => f,
            other => panic!("Expected Field, got {:?}", other),
        };
        let options = match &field.input_type {
            FieldType::Radio { options } => options,
            other => panic!("Expected Radio, got {:?}", other),
        };

        assert!(
            options.len() >= 3,
            "All 3 options must survive, got {}",
            options.len()
        );

        // The third option from FR must carry placeholder for DE + EN.
        if let TranslatableString::Translated(map) = &options[2].name {
            assert_eq!(map.get("fr").and_then(|o| o.as_deref()), Some("Abstention"));
            assert_eq!(map.get("de").and_then(|o| o.as_deref()), None);
            assert_eq!(map.get("en").and_then(|o| o.as_deref()), None);
        } else {
            panic!("Expected Translated name for third option");
        }
    }

    #[test]
    fn test_three_language_conditional_alignment_across_orders() {
        // All three languages share a conditional with the same condition.
        // Regardless of input order, the content must be merged with all
        // three translations present.
        let cond = FieldCondition {
            field_name: FieldId::from("toggle"),
            value: InputValue::Text("on".to_string()),
        };

        let mk = |lang: &str, text: &str| {
            make_envelope(
                lang,
                vec![StructuredNode::Conditional(ConditionalNode {
                    condition: cond.clone(),
                    content: Box::new(StructuredNode::Paragraph(ParagraphNode {
                        content: TranslatedText::plain_with_lang(lang, text),
                        som_path: None,
                        source_name: None,
                    })),
                })],
            )
        };

        let de = mk("de", "Hallo");
        let en = mk("en", "Hello");
        let fr = mk("fr", "Bonjour");

        for envelopes in [
            vec![de.clone(), en.clone(), fr.clone()],
            vec![fr.clone(), en.clone(), de.clone()],
        ] {
            let result = merge_translations(envelopes, None).unwrap();
            assert_eq!(result.content.len(), 1, "Single conditional expected");

            if let StructuredNode::Conditional(c) = &result.content[0] {
                assert_eq!(c.condition, cond);
                if let StructuredNode::Paragraph(p) = c.content.as_ref() {
                    assert_eq!(p.content.get("de").unwrap().as_plain_text(), "Hallo");
                    assert_eq!(p.content.get("en").unwrap().as_plain_text(), "Hello");
                    assert_eq!(p.content.get("fr").unwrap().as_plain_text(), "Bonjour");
                } else {
                    panic!("Expected Paragraph inside Conditional");
                }
            } else {
                panic!("Expected Conditional");
            }
        }
    }

    #[test]
    fn test_three_language_table_merge() {
        // A table with identical structure in 3 languages.  All cell texts
        // must carry all 3 translations after merge.
        let mk_table = |lang: &str, header_text: &str, cell_text: &str| {
            make_envelope(
                lang,
                vec![StructuredNode::Table(TableNode {
                    header: Some(TableHeader {
                        cells: vec![StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang(lang, header_text),
                            som_path: None,
                            source_name: None,
                        })],
                    }),
                    rows: vec![TableRow {
                        cells: vec![StructuredNode::Paragraph(ParagraphNode {
                            content: TranslatedText::plain_with_lang(lang, cell_text),
                            som_path: None,
                            source_name: None,
                        })],
                    }],
                    caption: None,
                })],
            )
        };

        let result = merge_translations(
            vec![
                mk_table("de", "Spalte", "Wert"),
                mk_table("en", "Column", "Value"),
                mk_table("fr", "Colonne", "Valeur"),
            ],
            None,
        )
        .unwrap();

        assert_eq!(result.content.len(), 1);
        if let StructuredNode::Table(t) = &result.content[0] {
            let hcell = &t.header.as_ref().unwrap().cells[0];
            if let StructuredNode::Paragraph(p) = hcell {
                assert_eq!(p.content.0.len(), 3);
                assert_eq!(p.content.get("de").unwrap().as_plain_text(), "Spalte");
                assert_eq!(p.content.get("en").unwrap().as_plain_text(), "Column");
                assert_eq!(p.content.get("fr").unwrap().as_plain_text(), "Colonne");
            }
            let bcell = &t.rows[0].cells[0];
            if let StructuredNode::Paragraph(p) = bcell {
                assert_eq!(p.content.0.len(), 3);
                assert_eq!(p.content.get("de").unwrap().as_plain_text(), "Wert");
            }
        } else {
            panic!("Expected Table");
        }
    }

    /// Regression test: when merging 3 languages iteratively, a compound-word
    /// language (DE, 1 word) merged with EN (2 words) should still match
    /// against SP (3 words).  Previously `stable_inline_text_projection`
    /// picked only the alphabetically-first language key, causing a
    /// word-ratio mismatch and duplicate headings with MISSING TRANSLATION.
    #[test]
    fn test_merge_three_languages_compound_word_heading_no_duplicate() {
        // DE: single compound word; EN: 2 words; SP: 3 words.
        // word_ratio(DE, SP) = 3/1 = 3.0 > 2.5 threshold → would fail
        // word_ratio(EN, SP) = 3/2 = 1.5              → passes
        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("de", "Kundenerklärungen"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "field1".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("de", "Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("en", "Client representations"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "field1".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("en", "Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );
        let sp = make_envelope(
            "es",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("es", "Declaraciones del Cliente"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "field1".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("es", "Nombre")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );

        let result = merge_translations(vec![de, en, sp], None).unwrap();

        // There must be exactly one H2, not two.
        let h2_count = result
            .content
            .iter()
            .filter(|n| matches!(n, StructuredNode::Heading(h) if h.level.as_u8() == HeadingLevel::H2.as_u8()))
            .count();
        assert_eq!(
            h2_count, 1,
            "Expected exactly 1 H2 heading after 3-language merge, got {}",
            h2_count
        );

        // The single H2 must carry all three translations.
        if let StructuredNode::Heading(h) = &result.content[0] {
            assert_eq!(h.content.get("de").unwrap().as_plain_text(), "Kundenerklärungen");
            assert_eq!(h.content.get("en").unwrap().as_plain_text(), "Client representations");
            assert_eq!(h.content.get("es").unwrap().as_plain_text(), "Declaraciones del Cliente");
        } else {
            panic!("Expected Heading as first node");
        }

        // No node should contain MISSING TRANSLATION.
        fn assert_no_missing(nodes: &[StructuredNode]) {
            for node in nodes {
                match node {
                    StructuredNode::Heading(h) => {
                        for (lang, text) in &h.content.0 {
                            assert!(
                                !text.0.is_empty(),
                                "H2 has MISSING TRANSLATION for lang '{}'",
                                lang
                            );
                        }
                    }
                    StructuredNode::Group(g) => assert_no_missing(&g.children),
                    StructuredNode::Conditional(c) => assert_no_missing(&[*c.content.clone()]),
                    _ => {}
                }
            }
        }
        assert_no_missing(&result.content);
    }

    /// Regression test: a direct 2-language merge with a German compound-word
    /// heading (1 word) vs an English multi-word heading (3 words) must produce
    /// exactly one merged heading, not two orphaned headings.
    ///
    /// Previously the `word_ratio > 2.5` gate in `text_shape_compatible` would
    /// reject the pair (ratio = 3/1 = 3.0), leaving both nodes as orphans.
    /// The fix raises the limit to 4.0 whenever one side has a single word.
    #[test]
    fn test_two_language_direct_compound_word_heading() {
        // "Kundenkontoverwaltung" = 1 word (21 chars)
        // "Customer account management" = 3 words (27 chars, no spaces)
        // word_ratio = 3/1 = 3.0  →  previously failed (> 2.5), now passes (≤ 4.0)
        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("de", "Kundenkontoverwaltung"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "f1".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("de", "Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
                StructuredNode::Field(FieldNode {
                    name: "f2".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("de", "Adresse")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("en", "Customer account management"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "f1".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("en", "Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
                StructuredNode::Field(FieldNode {
                    name: "f2".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("en", "Address")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );

        let result = merge_translations(vec![de, en], None).unwrap();

        // Exactly one H2 — not two orphaned headings.
        let h2_count = result
            .content
            .iter()
            .filter(|n| {
                matches!(n, StructuredNode::Heading(h) if h.level.as_u8() == HeadingLevel::H2.as_u8())
            })
            .count();
        assert_eq!(h2_count, 1, "Expected exactly 1 H2 heading, got {h2_count}",);

        // The single H2 must carry both translations.
        if let StructuredNode::Heading(h) = &result.content[0] {
            assert_eq!(h.content.get("de").unwrap().as_plain_text(), "Kundenkontoverwaltung");
            assert_eq!(h.content.get("en").unwrap().as_plain_text(), "Customer account management");
        } else {
            panic!("Expected Heading as first node");
        }
    }

    /// Regression test: neighborhood recovery must pair orphaned headings that
    /// are separated by a matched anchor (crossed structure).
    ///
    /// Scenario: one language has the heading *before* a field, the other has
    /// it *after*.  The LCS produces [LeftOnly(H_de), Matched(F), RightOnly(H_en)].
    /// Previously `consolidate_by_neighborhood` would `break` on the matched
    /// boundary and never pair the two headings.  The fix changes `break` to
    /// `continue` so the search skips past matched entries.
    ///
    /// The heading texts are chosen so that `word_ratio = 5/1 = 5.0` exceeds
    /// even the relaxed compound-word threshold (4.0), guaranteeing the LCS
    /// phase itself cannot match them and the recovery pass is the only path.
    #[test]
    fn test_neighborhood_recovery_past_matched_boundary() {
        // H_de = 1 word (33 chars), H_en = 5 words → word_ratio = 5.0 > 4.0
        // The LCS will match the shared field but leave both headings as orphans.
        // DE document: Heading then Field.
        let de = make_envelope(
            "de",
            vec![
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("de", "Kontoeröffnungsantragsbearbeitung"),
                    som_path: None,
                    source_name: None,
                }),
                StructuredNode::Field(FieldNode {
                    name: "anchor_field".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("de", "Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
            ],
        );
        // EN document: Field then Heading (crossed order).
        let en = make_envelope(
            "en",
            vec![
                StructuredNode::Field(FieldNode {
                    name: "anchor_field".into(),
                    som_path: None,
                    label: Some(TranslatedText::plain_with_lang("en", "Name")),
                    input_type: FieldType::Text {
                        regex: None,
                        max_length: None,
                        min_length: None,
                    },
                    value: None,
                    placeholder: None,
                    required: false,
                }),
                StructuredNode::Heading(HeadingNode {
                    level: HeadingLevel::H2,
                    content: TranslatedText::plain_with_lang("en", "Processing of account opening applications"),
                    som_path: None,
                    source_name: None,
                }),
            ],
        );

        let result = merge_translations(vec![de, en], None).unwrap();

        // Exactly one H2 — the neighborhood recovery must pair them.
        let h2_count = result
            .content
            .iter()
            .filter(|n| {
                matches!(n, StructuredNode::Heading(h) if h.level.as_u8() == HeadingLevel::H2.as_u8())
            })
            .count();
        assert_eq!(
            h2_count, 1,
            "Expected exactly 1 H2 (neighborhood recovery past matched boundary), got {h2_count}",
        );

        // Find the H2 and verify it carries both translations.
        let heading = result
            .content
            .iter()
            .find(|n| matches!(n, StructuredNode::Heading(h) if h.level.as_u8() == HeadingLevel::H2.as_u8()))
            .expect("H2 heading not found");
        if let StructuredNode::Heading(h) = heading {
            assert_eq!(h.content.get("de").unwrap().as_plain_text(), "Kontoeröffnungsantragsbearbeitung");
            assert_eq!(h.content.get("en").unwrap().as_plain_text(), "Processing of account opening applications");
        }
    }
}
