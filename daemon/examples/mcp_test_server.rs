//! An MCP server for the daemon's tests, with two tools: `echo` returns its `text`, `fail`
//! reports a failure of its own. `stdio` serves one session on stdin and stdout; `http <port>`
//! serves streamable HTTP at `/mcp` on 127.0.0.1.

use std::sync::Arc;

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EchoRequest {
    /// What to send back.
    text: String,
}

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Server {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Return the text given")]
    fn echo(&self, Parameters(EchoRequest { text }): Parameters<EchoRequest>) -> String {
        text
    }

    #[tool(description = "Always report an error")]
    fn fail(&self) -> CallToolResult {
        CallToolResult::error(vec![ContentBlock::text("the fail tool failed")])
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
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
        _ => anyhow::bail!("usage: mcp_test_server stdio | http <port>"),
    }
    Ok(())
}
