//! An A2UI-over-MCP server for the daemon's tests and for looking at an A2UI pane. The layout is
//! the resource `a2ui://notes` (`application/a2ui+json`: `createSurface` and `updateComponents`
//! with data bindings, one component outside the basic catalog); `get_notes` links it through
//! `_meta.ui` and returns the first `updateDataModel`; `save_note` is the button's action: it keeps
//! the text and answers with an `updateDataModel` that changes `/status` and nothing else. `stdio`
//! serves one session on stdin and stdout.

use std::sync::{Arc, Mutex};

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{
    CallToolResult, ContentBlock, MetaObject, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::{
    schemars, service::RequestContext, tool, tool_handler, tool_router, ErrorData, RoleServer,
    ServerHandler, ServiceExt,
};

const MIME: &str = "application/a2ui+json";
const URI: &str = "a2ui://notes";

fn template() -> serde_json::Value {
    serde_json::json!([
        { "version": "v0.9", "createSurface": {
            "surfaceId": "notes",
            "catalogId": "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json" } },
        { "version": "v0.9", "updateComponents": { "surfaceId": "notes", "components": [
            { "id": "root", "component": "Column", "align": "stretch",
              "children": ["title", "draft", "save", "status", "gizmo", "list"] },
            { "id": "title", "component": "Text", "variant": "h2", "text": "Notes" },
            { "id": "draft", "component": "TextField", "label": "New note",
              "value": { "path": "/draft" } },
            { "id": "save", "component": "Button", "child": "save_label", "variant": "primary",
              "action": { "event": { "name": "save_note",
                "context": { "text": { "path": "/draft" } } } } },
            { "id": "save_label", "component": "Text", "text": "Add note" },
            { "id": "status", "component": "Text", "variant": "caption",
              "text": { "path": "/status" } },
            { "id": "gizmo", "component": "Gizmo", "size": 3 },
            { "id": "list", "component": "Text", "text": { "path": "/count" } }
        ] } }
    ])
}

fn data(path: &str, value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "version": "v0.9",
        "updateDataModel": { "surfaceId": "notes", "path": path, "value": value } })
}

fn ui_meta() -> MetaObject {
    let value = serde_json::json!({ "ui": { "resourceUri": URI, "mimeType": MIME } });
    MetaObject(value.as_object().unwrap().clone())
}

/// A result's content: the message as an embedded `application/a2ui+json` resource, then a text
/// fallback.
fn embedded(message: serde_json::Value) -> CallToolResult {
    let text = message.to_string();
    CallToolResult::success(vec![
        ContentBlock::resource(ResourceContents::TextResourceContents {
            uri: URI.into(),
            mime_type: Some(MIME.into()),
            text: text.clone(),
            meta: None,
        }),
        ContentBlock::text(text),
    ])
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SaveNote {
    /// The note to keep.
    text: String,
}

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
    notes: Arc<Mutex<Vec<String>>>,
}

#[tool_router]
impl Server {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            notes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[tool(description = "The notes surface", meta = ui_meta())]
    fn get_notes(&self) -> CallToolResult {
        embedded(data(
            "/",
            serde_json::json!({
                "draft": "Ship the spike", "status": "no notes yet", "count": "0 notes" }),
        ))
    }

    #[tool(description = "Keep a note", meta = ui_meta())]
    fn save_note(&self, Parameters(SaveNote { text }): Parameters<SaveNote>) -> CallToolResult {
        let mut notes = self.notes.lock().unwrap();
        notes.push(text.clone());
        embedded(serde_json::json!([
            data("/status", format!("saved: {text}").into()),
            data("/count", format!("{} notes", notes.len()).into()),
        ]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if request.uri != URI {
            return Err(ErrorData::resource_not_found(
                format!("no resource {}", request.uri),
                None,
            ));
        }
        Ok(ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
            uri: request.uri,
            mime_type: Some(MIME.into()),
            text: template().to_string(),
            meta: None,
        }])
        .into())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("stdio") => {
            Server::new()
                .serve(rmcp::transport::stdio())
                .await?
                .waiting()
                .await?;
            Ok(())
        }
        _ => anyhow::bail!("usage: mcp_a2ui_test_server stdio"),
    }
}
