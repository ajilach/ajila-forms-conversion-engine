//! XFA Event Types and Script Definitions
//!
//! This module defines the event and script types per XFA 3.3 specification.
//!
//! ## XFA 3.3 Spec References:
//! - Chapter 10: Events (page 378-408)
//! - Chapter 11: Scripting (page 410-416)

use std::str::FromStr;

/// Script content type as per XFA spec
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptContentType {
    JavaScript,
    FormCalc,
}

impl ScriptContentType {
    pub fn from_content_type(s: &str) -> Option<Self> {
        match s {
            "application/x-javascript" => Some(ScriptContentType::JavaScript),
            "application/x-formcalc" => Some(ScriptContentType::FormCalc),
            _ => None,
        }
    }
}

/// XFA Event activity types
/// Per XFA spec section 10, "Events"
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EventActivity {
    Ready,
    Initialize,
    Enter,
    Exit,
    Change,
    Click,
    Calculate,
    Validate,
    PreSubmit,
    PostSubmit,
    DocReady,
    IndexChange,
    Other(String),
}

impl FromStr for EventActivity {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "ready" => EventActivity::Ready,
            "initialize" => EventActivity::Initialize,
            "enter" => EventActivity::Enter,
            "exit" => EventActivity::Exit,
            "change" => EventActivity::Change,
            "click" => EventActivity::Click,
            "calculate" => EventActivity::Calculate,
            "validate" => EventActivity::Validate,
            "preSubmit" => EventActivity::PreSubmit,
            "postSubmit" => EventActivity::PostSubmit,
            "docReady" => EventActivity::DocReady,
            "indexChange" => EventActivity::IndexChange,
            _ => EventActivity::Other(s.to_string()),
        })
    }
}

/// XFA Event reference target
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventRef {
    Form,
    Layout,
    Data,
    Current,
    Named(String),
}

impl FromStr for EventRef {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "$form" | "xfa.form" => EventRef::Form,
            "$layout" | "xfa.layout" => EventRef::Layout,
            "$data" | "xfa.data" => EventRef::Data,
            "$" => EventRef::Current,
            _ => EventRef::Named(s.to_string()),
        })
    }
}

/// Where the script should be executed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunAt {
    #[default]
    Client,
    Server,
    Both,
}

impl FromStr for RunAt {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "server" => RunAt::Server,
            "both" => RunAt::Both,
            _ => RunAt::Client,
        })
    }
}

/// Represents a script attached to an event
#[derive(Debug, Clone)]
pub struct XfaScript {
    pub source: String,
    pub content_type: ScriptContentType,
    pub activity: EventActivity,
    pub event_ref: EventRef,
    pub name: Option<String>,
    pub run_at: RunAt,
}

// =============================================================================
// Parsing functions
// =============================================================================

use crate::xfa::{XfaNode, XfaNodeKind};

/// Parse all event scripts from node children
pub fn parse_events_from_node(children: &[XfaNode]) -> Vec<XfaScript> {
    let mut scripts = Vec::new();

    for child in children {
        if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
            if tag_name == "event" {
                if let Some(script) = parse_event_element(child) {
                    scripts.push(script);
                }
            }
        }
    }

    scripts
}

fn parse_event_element(event_node: &XfaNode) -> Option<XfaScript> {
    let activity = event_node
        .attributes
        .get("activity")
        .and_then(|s| s.parse().ok())
        .unwrap_or(EventActivity::Other("unknown".to_string()));

    let event_ref = event_node
        .attributes
        .get("ref")
        .and_then(|s| s.parse().ok())
        .unwrap_or(EventRef::Current);

    let name = event_node.attributes.get("name").cloned();

    for child in &event_node.children {
        if let XfaNodeKind::Element {
            tag_name,
            text_content,
        } = &child.kind
        {
            if tag_name == "script" {
                let content_type = child
                    .attributes
                    .get("contentType")
                    .and_then(|s| ScriptContentType::from_content_type(s))
                    .unwrap_or(ScriptContentType::FormCalc);

                let run_at = child
                    .attributes
                    .get("runAt")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();

                let source = text_content.clone().unwrap_or_default();

                if !source.trim().is_empty() {
                    return Some(XfaScript {
                        source,
                        content_type,
                        activity,
                        event_ref,
                        name,
                        run_at,
                    });
                }
            }
        }
    }

    None
}

/// Parse variables from node children (for script objects)
pub fn parse_variables_from_node(
    children: &[XfaNode],
) -> std::collections::HashMap<String, std::collections::HashMap<String, String>> {
    let mut variables = std::collections::HashMap::new();

    for child in children {
        if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
            if tag_name == "variables" {
                for var_child in &child.children {
                    if let XfaNodeKind::Element {
                        tag_name: var_tag, ..
                    } = &var_child.kind
                    {
                        if var_tag == "script" {
                            if let Some(name) = var_child.attributes.get("name") {
                                variables.insert(name.clone(), std::collections::HashMap::new());
                            }
                        }
                    }
                }
            }
        }
    }

    variables
}
