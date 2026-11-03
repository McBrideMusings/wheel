//! An MCP Apps server for the daemon's tests. `show` links the `ui://test/view` page; `bump` is
//! visible to the page only; `secret` is visible to the model only; `report` stores a line the
//! page sends and `reported` returns the lines so far, which is how a test reads what the page
//! saw. `stdio` serves one session on stdin and stdout; `http <port>` serves streamable HTTP at
//! `/mcp` on 127.0.0.1.

use std::sync::{Arc, Mutex};

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{
    MetaObject, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{
    schemars, service::RequestContext, tool, tool_handler, tool_router, ErrorData, RoleServer,
    ServerHandler, ServiceExt,
};

/// The lines the page has reported, shared by every session of this process.
static REPORTED: Mutex<Vec<String>> = Mutex::new(Vec::new());

const VIEW: &str = r#"<!doctype html>
<body>
<h1 id="title">waiting</h1>
<button id="bump">bump</button>
<p id="out"></p>
<script>
let next = 1;
const pending = {};
function request(method, params) {
  return new Promise((resolve) => {
    const id = next++;
    pending[id] = resolve;
    parent.postMessage({ jsonrpc: "2.0", id, method, params }, "*");
  });
}
async function report(text) {
  await request("tools/call", { name: "report", arguments: { text } });
}
addEventListener("message", (event) => {
  const m = event.data;
  if (m.id !== undefined && pending[m.id]) { pending[m.id](m); delete pending[m.id]; return; }
  if (m.method === "ui/notifications/tool-result") {
    const text = m.params.content[0].text;
    document.getElementById("title").textContent = text;
    report("result:" + text);
  }
});
document.getElementById("bump").onclick = async () => {
  const r = await request("tools/call", { name: "bump", arguments: {} });
  const text = r.result.content[0].text;
  document.getElementById("out").textContent = text;
  await report("bump:" + text);
  const refused = await request("tools/call", { name: "secret", arguments: {} });
  await report("secret:" + (refused.error ? "refused" : "ALLOWED"));
};
(async () => {
  await request("ui/initialize", {
    protocolVersion: "2026-01-26",
    appInfo: { name: "test view", version: "1" },
    appCapabilities: {},
  });
  parent.postMessage({ jsonrpc: "2.0", method: "ui/notifications/initialized" }, "*");
  const found = [];
  try { found.push("document:" + typeof parent.document.body); } catch (e) { found.push("document:blocked"); }
  try { found.push("wheel:" + typeof parent.wheel); } catch (e) { found.push("wheel:blocked"); }
  try { await fetch("/artifacts/"); found.push("fetch:allowed"); } catch (e) { found.push("fetch:blocked"); }
  await report("escape:" + found.join(","));
  // A real click on the button, so a check needs no pointer into the frame.
  setTimeout(() => document.getElementById("bump").click(), 300);
})();
</script>
"#;

fn ui_meta(visibility: &str) -> MetaObject {
    let value = serde_json::json!({ "ui": {
        "resourceUri": "ui://test/view",
        "visibility": serde_json::from_str::<serde_json::Value>(visibility).unwrap(),
    } });
    MetaObject(value.as_object().unwrap().clone())
}

fn visibility_only(visibility: &str) -> MetaObject {
    let value = serde_json::json!({ "ui": {
        "visibility": serde_json::from_str::<serde_json::Value>(visibility).unwrap(),
    } });
    MetaObject(value.as_object().unwrap().clone())
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Report {
    /// One line of what the page saw.
    text: String,
}

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
    bumps: Arc<Mutex<u32>>,
}

#[tool_router]
impl Server {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            bumps: Arc::new(Mutex::new(0)),
        }
    }

    #[tool(description = "The tool the view belongs to", meta = ui_meta(r#"["model","app"]"#))]
    fn show(&self) -> String {
        "hello from show".to_string()
    }

    #[tool(description = "Count a click", meta = visibility_only(r#"["app"]"#))]
    fn bump(&self) -> String {
        let mut bumps = self.bumps.lock().unwrap();
        *bumps += 1;
        format!("bumped {}", *bumps)
    }

    #[tool(description = "Not for the page", meta = visibility_only(r#"["model"]"#))]
    fn secret(&self) -> String {
        "secret".to_string()
    }

    #[tool(description = "Store a line the page reports", meta = visibility_only(r#"["app"]"#))]
    fn report(&self, Parameters(Report { text }): Parameters<Report>) -> String {
        REPORTED.lock().unwrap().push(text);
        "ok".to_string()
    }

    #[tool(description = "The lines the page has reported")]
    fn reported(&self) -> String {
        REPORTED.lock().unwrap().join("|")
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
        if request.uri != "ui://test/view" {
            return Err(ErrorData::resource_not_found(
                format!("no resource {}", request.uri),
                None,
            ));
        }
        Ok(ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
            uri: request.uri,
            mime_type: Some("text/html;profile=mcp-app".into()),
            text: VIEW.to_string(),
            meta: None,
        }])
        .into())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("stdio") => {
            Server::new()
                .serve(rmcp::transport::stdio())
                .await?
                .waiting()
                .await?;
        }
        Some("http") => {
            let port: u16 = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("http <port>"))?
                .parse()?;
            let service = StreamableHttpService::new(
                || Ok(Server::new()),
                Arc::new(LocalSessionManager::default()),
                StreamableHttpServerConfig::default(),
            );
            let app = axum::Router::new().nest_service("/mcp", service);
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
            axum::serve(listener, app).await?;
        }
        _ => anyhow::bail!("usage: mcp_ui_test_server stdio | http <port>"),
    }
    Ok(())
}
