//! The daemon as an MCP client: an artifact entry that names a server (`mcp`, a command run
//! through `sh -c`, or `mcp_url`, a streamable HTTP endpoint) gets one session, held here and
//! shared by every caller of `artifact tools` and `artifact call`.
//!
//! A session is made on the first call that needs it and replaced when the entry's definition
//! changes or the server's end of the connection closes. A stdio server runs in the space's
//! directory with the login PATH; its process dies with its session.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use rmcp::service::{Peer, RoleClient, RunningService};
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::ServiceExt;
use tokio::io::AsyncBufReadExt;
use wheel_proto::{ArtifactScope, McpTool, McpToolResult};

use crate::artifacts::Entry;
use crate::server::AppState;

/// How long a server has to start and answer `initialize`.
const CONNECT: Duration = Duration::from_secs(30);

/// What an entry says its server is. Two equal definitions are the same server, so a session
/// outlives edits to the entry that do not touch these.
#[derive(Clone, Debug, PartialEq)]
enum Def {
    Stdio(String),
    Http(String),
}

struct Session {
    def: Def,
    service: RunningService<RoleClient, ()>,
}

/// The sessions of every artifact, keyed by scope, name and the space's directory. Each key has a
/// lock of its own, so one server connecting never holds up another's calls.
#[derive(Default)]
pub(crate) struct Sessions {
    by_key: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Option<Session>>>>>,
}

impl Sessions {
    fn slot(&self, key: &str) -> Arc<tokio::sync::Mutex<Option<Session>>> {
        self.by_key
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_default()
            .clone()
    }
}

impl Def {
    fn of(entry: &Entry) -> Result<Def, String> {
        match (&entry.mcp, &entry.mcp_url) {
            (Some(command), None) => Ok(Def::Stdio(command.clone())),
            (None, Some(url)) => Ok(Def::Http(url.clone())),
            (None, None) => {
                Err("the entry names no MCP server: it has neither mcp nor mcp_url".into())
            }
            (Some(_), Some(_)) => Err("the entry names two MCP servers: mcp and mcp_url".into()),
        }
    }
}

impl AppState {
    /// The peer of artifact `name`'s MCP session, connecting first when there is none.
    async fn mcp_peer(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
    ) -> Result<Peer<RoleClient>, String> {
        let cwd = self.artifact_space(space)?;
        let artifact = self.artifact_ref(scope, name, &cwd)?;
        let entry = self.artifact_entry(&artifact, &cwd)?.ok_or_else(|| {
            format!(
                "no {} artifact {name}: {} has no entry for it",
                artifact.scope().name(),
                self.artifact_registry(artifact.scope(), &cwd).display()
            )
        })?;
        if let Some(problem) = entry.problems().first() {
            return Err(format!("the entry for {name} is invalid: {problem}"));
        }
        let def = Def::of(&entry)?;
        let key = format!("{}\0{}\0{}", artifact.scope().name(), artifact.name(), cwd);
        let slot = self.mcp.slot(&key);
        let mut held = slot.lock().await;
        if let Some(session) = held.as_ref() {
            if session.def == def && !session.service.is_closed() {
                return Ok(session.service.peer().clone());
            }
        }
        // Dropping the old session ends its process, or its HTTP session, before the new one starts.
        *held = None;
        let service = tokio::time::timeout(CONNECT, self.mcp_connect(&def, &cwd, name))
            .await
            .map_err(|_| {
                format!(
                    "{name}: the MCP server did not finish starting within {}s",
                    CONNECT.as_secs()
                )
            })??;
        tracing::info!(scope = artifact.scope().name(), %name, ?def, "MCP session open");
        let peer = service.peer().clone();
        *held = Some(Session { def, service });
        Ok(peer)
    }

    async fn mcp_connect(
        &self,
        def: &Def,
        cwd: &str,
        name: &str,
    ) -> Result<RunningService<RoleClient, ()>, String> {
        match def {
            Def::Stdio(command) => {
                let mut cmd = tokio::process::Command::new("/bin/sh");
                cmd.arg("-c")
                    .arg(command)
                    .current_dir(cwd)
                    .envs(crate::shell::command_env(
                        self.control.lock().unwrap().as_ref(),
                        &self.instance_dir,
                        &self.login_path.lock().unwrap().path,
                    ))
                    .env("WHEEL_SPACE_DIR", cwd);
                let (transport, stderr) = TokioChildProcess::builder(cmd)
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("run {command:?}: {e}"))?;
                if let Some(stderr) = stderr {
                    let name = name.to_string();
                    tokio::spawn(async move {
                        let mut lines = tokio::io::BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            tracing::debug!(%name, "mcp stderr: {line}");
                        }
                    });
                }
                ().serve(transport)
                    .await
                    .map_err(|e| format!("{name}: {command:?} did not start an MCP session: {e}"))
            }
            Def::Http(url) => {
                // A https URL needs the process's TLS provider, which nothing else may have set.
                let _ = rustls::crypto::ring::default_provider().install_default();
                ().serve(StreamableHttpClientTransport::from_uri(url.as_str()))
                    .await
                    .map_err(|e| format!("{name}: cannot open an MCP session with {url}: {e}"))
            }
        }
    }

    /// The tools of artifact `name`'s MCP server.
    pub async fn mcp_tools(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
    ) -> Result<Vec<McpTool>, String> {
        let peer = self.mcp_peer(name, scope, space).await?;
        let tools = peer
            .list_all_tools()
            .await
            .map_err(|e| format!("{name}: list tools: {e}"))?;
        Ok(tools
            .into_iter()
            .map(|t| McpTool {
                name: t.name.into_owned(),
                description: t.description.map(|d| d.into_owned()),
                input_schema: serde_json::Value::Object((*t.input_schema).clone()),
            })
            .collect())
    }

    /// Call `tool` of artifact `name`'s MCP server with `arguments`.
    pub async fn mcp_call(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
        tool: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<McpToolResult, String> {
        let arguments = match arguments {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Object(map)) => Some(map),
            Some(other) => {
                return Err(format!(
                    "the arguments of a tool are a JSON object, not {other}"
                ))
            }
        };
        let peer = self.mcp_peer(name, scope, space).await?;
        let mut params = CallToolRequestParams::new(tool.to_string());
        params.arguments = arguments;
        let result = peer
            .call_tool(params)
            .await
            .map_err(|e| format!("{name}: call {tool}: {e}"))?;
        let content = result
            .content
            .iter()
            .map(|c| serde_json::to_value(c).map_err(|e| format!("{name}: {tool}'s result: {e}")))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(McpToolResult {
            content,
            structured: result.structured_content,
            is_error: result.is_error.unwrap_or(false),
        })
    }
}
