//! Blueprint MCP server (stdio).
//!
//! Exposes the form-conversion engine's tools over the Model Context Protocol so
//! an external LLM client (Claude Desktop, Claude Code, Cursor, …) can drive a
//! conversion step by step. The client supplies the reasoning; this server
//! supplies the tools, backed by the headless [`agent::ConversionAgent`].
//!
//! Transport is stdio: the client launches this binary as a subprocess and
//! speaks JSON-RPC over stdin/stdout. Register it in the client's MCP config
//! with `command` pointing at the built binary.
//!
//! ## Session model
//!
//! A single implicit conversion is held in the server. Call `start_conversion`
//! (with a `pdf_path` or `pdf_base64`, and an optional `profile`) to load a
//! source PDF; every other tool then operates on that loaded conversion. Each
//! tree change is versioned into the same edit-history SQLite the desktop app
//! uses, so a conversion driven here can later be reviewed in the app.

use std::sync::Arc;

use agent::{ConversionAgent, ToolReply};
use rmcp::handler::server::ServerHandler;
use rmcp::model::*;
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServiceExt};
use tokio::sync::Mutex;

/// The MCP server: holds a single implicit conversion session.
#[derive(Clone)]
struct Blueprint {
    agent: Arc<Mutex<Option<ConversionAgent>>>,
}

impl Blueprint {
    fn new() -> Self {
        Self {
            agent: Arc::new(Mutex::new(None)),
        }
    }
}

/// Build a `start_conversion` tool spec in the same Anthropic-style JSON shape
/// `ConversionAgent::tools()` emits, so it maps through the same converter.
fn start_conversion_spec() -> serde_json::Value {
    serde_json::json!({
        "name": "start_conversion",
        "description": "Load the source PDF(s) and begin a conversion. PREFER `pdf_path` (an absolute path on the machine running this server) — efficient, no payload. Use `pdf_paths` for a form that spans several PDFs. Only if the file is NOT reachable on the server's filesystem (e.g. it lives in a sandbox/upload), fall back to `pdf_base64` (raw PDF bytes, base64). Optionally pass a `profile` name (selects the AEM config + reference library). Must be called before any other tool; calling it again starts a fresh conversion.",
        "input_schema": {
            "type": "object",
            "properties": {
                "pdf_path": {"type": "string", "description": "Absolute path to the source PDF, on the server's filesystem. Preferred."},
                "pdf_paths": {"type": "array", "items": {"type": "string"}, "description": "Absolute paths to the source PDFs (use instead of pdf_path for a multi-PDF form)."},
                "pdf_base64": {"type": "string", "description": "Base64-encoded PDF bytes. Fallback for when the file is not on the server's filesystem; large, so prefer a path when possible."},
                "pdf_name": {"type": "string", "description": "Display name for a pdf_base64 source (e.g. \"form.pdf\")."},
                "profile": {"type": "string", "description": "Conversion profile name (for AEM config + references)."}
            },
            "required": []
        }
    })
}

/// Spec for the MCP-only `write_package` tool that exports the built AEM package
/// (a binary ZIP) to a local file path — binaries are returned by path, not
/// inlined into the transcript.
fn write_package_spec() -> serde_json::Value {
    serde_json::json!({
        "name": "write_package",
        "description": "Write the built AEM FileVault package (ZIP) to a local file path and return the path. Run build_aem_package first. Use this to retrieve the binary result instead of reading it into the conversation.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Absolute path to write the .zip package to."}
            },
            "required": ["path"]
        }
    })
}

/// The full advertised catalog: the MCP-only bootstrap/export tools plus the
/// engine tools.
///
/// The engine tools are static (independent of conversion state), so they are
/// read from a throwaway agent.
fn tool_catalog() -> Vec<serde_json::Value> {
    let mut specs = vec![start_conversion_spec(), write_package_spec()];
    let probe = ConversionAgent::new(None, Vec::new(), None, String::new());
    specs.extend(probe.tools());
    specs
}

/// Convert one Anthropic-style tool spec (`{name, description, input_schema}`)
/// into an rmcp [`Tool`]. The raw JSON schema is passed straight through as the
/// MCP `inputSchema`, so no schema derive (and no schemars-version coupling) is
/// involved.
fn to_mcp_tool(spec: &serde_json::Value) -> Option<Tool> {
    let name = spec.get("name")?.as_str()?.to_string();
    let description = spec
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or_default()
        .to_string();
    let input_schema = spec
        .get("input_schema")
        .and_then(|s| s.as_object())
        .cloned()
        .unwrap_or_default();
    Some(Tool::new(name, description, Arc::new(input_schema)))
}

/// Map an engine [`ToolReply`] onto an MCP [`CallToolResult`].
fn reply_to_result(reply: ToolReply) -> CallToolResult {
    match reply {
        ToolReply::Text(text) => CallToolResult::success(vec![Content::text(text)]),
        ToolReply::Image { media_type, b64 } => {
            CallToolResult::success(vec![Content::image(b64, media_type.to_string())])
        }
        ToolReply::Error(msg) => CallToolResult::error(vec![Content::text(msg)]),
    }
}

impl Blueprint {
    /// Handle the `start_conversion` bootstrap tool: read the PDF(s) from disk,
    /// create an edit-history session, and install a fresh [`ConversionAgent`].
    async fn start_conversion(&self, args: &serde_json::Value) -> CallToolResult {
        // Collect the requested source paths (single `pdf_path` and/or the
        // `pdf_paths` array), preserving order and de-duplicating.
        let mut paths: Vec<String> = Vec::new();
        if let Some(p) = args.get("pdf_path").and_then(|v| v.as_str()) {
            paths.push(p.to_string());
        }
        if let Some(arr) = args.get("pdf_paths").and_then(|v| v.as_array()) {
            for v in arr {
                if let Some(p) = v.as_str() {
                    paths.push(p.to_string());
                }
            }
        }
        paths.dedup();
        if paths.is_empty() {
            return CallToolResult::error(vec![Content::text(
                "start_conversion requires `pdf_path` or `pdf_paths`.",
            )]);
        }

        // Read each path; bail with a clear error on the first failure.
        let mut pdfs: Vec<(String, Vec<u8>)> = Vec::new();
        for path in &paths {
            match std::fs::read(path) {
                Ok(bytes) => {
                    let name = std::path::Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "source.pdf".to_string());
                    pdfs.push((name, bytes));
                }
                Err(e) => {
                    return CallToolResult::error(vec![Content::text(format!(
                        "Could not read {path:?}: {e}"
                    ))]);
                }
            }
        }

        let profile = args
            .get("profile")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let label = pdfs
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        // Version the conversion into the shared edit-history DB so the desktop
        // app can later review it. Falls back to a derived id if the DB is
        // unavailable.
        let doc_hash = agent::db::document_hash(&pdfs);
        agent::db::upsert_document(&doc_hash, &label);
        let session = agent::db::create_session(&doc_hash, profile.as_deref(), &label)
            .unwrap_or_else(|| format!("mcp-{doc_hash}"));
        agent::db::insert_edit(&session, "Initial (empty)", "[]");

        // No live AEM connection over MCP: profile-derived config/packaging still
        // works; `upload_to_aem` and the fetch tools report no connection.
        let count = pdfs.len();
        let new_agent = ConversionAgent::new(profile, pdfs, None, session.clone());
        *self.agent.lock().await = Some(new_agent);

        CallToolResult::success(vec![Content::text(format!(
            "Loaded {count} PDF(s) [{label}] (session {session}). Call list_states / \
             get_source_info to inspect the source, then convert. When done, build_aem_package \
             then write_package to export the ZIP to a path. Note: upload_to_aem is unavailable \
             over MCP (no live connection)."
        ))])
    }

    /// Handle the MCP-only `write_package` tool: write the built package bytes
    /// to a file path. Binaries leave the server by path, never inlined.
    async fn write_package(&self, args: &serde_json::Value) -> CallToolResult {
        let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
            return CallToolResult::error(vec![Content::text("write_package requires `path`.")]);
        };
        let guard = self.agent.lock().await;
        let Some(conv) = guard.as_ref() else {
            return CallToolResult::error(vec![Content::text(
                "No conversion loaded. Call `start_conversion` first.",
            )]);
        };
        let Some(pkg) = conv.package() else {
            return CallToolResult::error(vec![Content::text(
                "No package built yet. Call build_aem_package first.",
            )]);
        };
        let size = pkg.len();
        match std::fs::write(path, &pkg) {
            Ok(()) => CallToolResult::success(vec![Content::text(format!(
                "Wrote package ({size} bytes) to {path:?}."
            ))]),
            Err(e) => CallToolResult::error(vec![Content::text(format!(
                "Could not write {path:?}: {e}"
            ))]),
        }
    }
}

impl ServerHandler for Blueprint {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("blueprint", env!("CARGO_PKG_VERSION"));
        // Advertise the shared workflow guidance (the same text the desktop app
        // injects as the agent's opening message), wrapped in the MCP-specific
        // bootstrap/teardown the engine tools don't cover: `start_conversion`
        // must run first, the finished ZIP leaves by path via `write_package`,
        // and there is no live AEM connection over MCP.
        info.instructions = Some(format!(
            "Blueprint form-conversion tools (MCP).\n\n\
             FIRST: call `start_conversion` with a `pdf_path` (or `pdf_paths`) and optional \
             `profile`. It must precede every other tool; every tool then operates on that \
             loaded conversion.\n\n\
             {SYSTEM_PROMPT}\n\n\
             MCP specifics: all file inputs/outputs are local file paths, never inlined bytes. \
             Instead of `finish`, export the finished package with `write_package` (writes the \
             built ZIP to a path) after build_aem_package. `upload_to_aem` and the fetch tools \
             are unavailable over MCP (no live AEM connection); profile-derived config and \
             packaging still work.",
            SYSTEM_PROMPT = agent::SYSTEM_PROMPT,
        ));
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = tool_catalog().iter().filter_map(to_mcp_tool).collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let name = request.name.as_ref();
        let input = serde_json::Value::Object(request.arguments.unwrap_or_default());

        if name == "start_conversion" {
            return Ok(self.start_conversion(&input).await);
        }
        if name == "write_package" {
            return Ok(self.write_package(&input).await);
        }

        let mut guard = self.agent.lock().await;
        let Some(conv) = guard.as_mut() else {
            return Ok(CallToolResult::error(vec![Content::text(
                "No conversion loaded. Call `start_conversion` first.",
            )]));
        };
        let reply = conv.execute(name, &input).await;
        Ok(reply_to_result(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let transport = rmcp::transport::stdio();
    let service = Blueprint::new().serve(transport).await?;
    service.waiting().await?;
    Ok(())
}
