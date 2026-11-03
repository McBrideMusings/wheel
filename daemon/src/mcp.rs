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
                meta: t.meta.and_then(|m| serde_json::to_value(m).ok()),
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
            meta: result.meta.and_then(|m| serde_json::to_value(m).ok()),
        })
    }
}

impl AppState {
    /// `resources/read` of `uri` on artifact `name`'s server: the `contents` it returned.
    pub async fn mcp_resource(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
        uri: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let read = self.mcp_read(name, scope, space, uri).await?;
        Ok(read
            .get("contents")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default())
    }
}

/// The MIME type an MCP Apps UI resource carries (SEP-1865).
const APP_MIME: &str = "text/html;profile=mcp-app";

/// Whether `mime` is `text/html` with the parameter `profile=mcp-app`, in any spacing, case or quoting.
fn is_app_mime(mime: &str) -> bool {
    let mut parts = mime.split(';').map(str::trim);
    parts.next().is_some_and(|t| t.eq_ignore_ascii_case("text/html"))
        && parts.any(|p| {
            p.split_once('=').is_some_and(|(k, v)| {
                k.trim().eq_ignore_ascii_case("profile") && v.trim().trim_matches('"') == "mcp-app"
            })
        })
}

/// The `ui://` resource a tool links to: `_meta.ui.resourceUri`, or the deprecated flat
/// `_meta["ui/resourceUri"]`.
fn linked_resource(tool: &serde_json::Value) -> Option<&str> {
    let meta = tool.get("_meta")?;
    meta.pointer("/ui/resourceUri")
        .or_else(|| meta.get("ui/resourceUri"))?
        .as_str()
}

/// Whether a page may call `tool`: its `_meta.ui.visibility` lists `app`, or is absent (default
/// `["model", "app"]`).
fn app_may_call(tool: &serde_json::Value) -> bool {
    match tool
        .pointer("/_meta/ui/visibility")
        .and_then(|v| v.as_array())
    {
        Some(visibility) => visibility.iter().any(|v| v == "app"),
        None => true,
    }
}

impl AppState {
    /// What a pane shows for the MCP server artifact `name` names: its tools as the server listed
    /// them (`_meta` included) and the MCP Apps view, if the server has one. The view is the
    /// `ui://` resource a tool links to (that tool beside it), else the first `ui://` resource in
    /// the server's resource list; `null` when there is none, which the pane answers with the
    /// tools alone.
    pub async fn mcp_view(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let peer = self.mcp_peer(name, scope, space).await?;
        let tools = peer
            .list_all_tools()
            .await
            .map_err(|e| format!("{name}: list tools: {e}"))?;
        let tools = tools
            .iter()
            .map(|t| serde_json::to_value(t).map_err(|e| format!("{name}: a tool: {e}")))
            .collect::<Result<Vec<_>, _>>()?;
        let linked = tools
            .iter()
            .find_map(|t| linked_resource(t).map(|uri| (Some(t.clone()), uri.to_string())));
        let linked = match linked {
            Some(linked) => Some(linked),
            // A server without the capability answers `resources/list` with an error, which is
            // the same as listing none.
            None => peer
                .list_all_resources()
                .await
                .ok()
                .and_then(|all| all.into_iter().find(|r| r.uri.starts_with("ui://")))
                .map(|r| (None, r.uri.clone())),
        };
        let mut view = serde_json::Value::Null;
        if let Some((tool, uri)) = linked {
            let read = self.mcp_read(name, scope, space, &uri).await?;
            let html = read
                .get("contents")
                .and_then(|c| c.as_array())
                .and_then(|c| c.first());
            if let Some(html) = html {
                let mime = html.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
                if is_app_mime(mime) {
                    view = serde_json::json!({ "tool": tool, "uri": uri, "resource": html });
                } else {
                    tracing::warn!(%name, %uri, %mime, "an MCP UI resource is not {APP_MIME}; showing the tools instead");
                }
            }
        }
        Ok(serde_json::json!({ "tools": tools, "view": view }))
    }

    /// `resources/read` of `uri` on artifact `name`'s server, as the server's JSON.
    pub async fn mcp_read(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
        uri: &str,
    ) -> Result<serde_json::Value, String> {
        let peer = self.mcp_peer(name, scope, space).await?;
        let read = peer
            .read_resource(rmcp::model::ReadResourceRequestParams::new(uri.to_string()))
            .await
            .map_err(|e| format!("{name}: read {uri}: {e}"))?;
        serde_json::to_value(read).map_err(|e| format!("{name}: {uri}: {e}"))
    }

    /// A `resources/read` a page makes: only a `ui://` resource or one the server lists.
    pub async fn mcp_app_read(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
        uri: &str,
    ) -> Result<serde_json::Value, String> {
        if !uri.starts_with("ui://") {
            let peer = self.mcp_peer(name, scope, space).await?;
            let listed = peer.list_all_resources().await.unwrap_or_default();
            if !listed.iter().any(|r| r.uri == uri) {
                return Err(format!("{uri} is not a resource the server lists"));
            }
        }
        self.mcp_read(name, scope, space, uri).await
    }

    /// A tool call from the pane: refused for a tool the server does not list, and, when a page
    /// makes it, for a tool whose visibility leaves out `app`. `initial` is the host running the
    /// tool that links the view, which only needs to be one that links a `ui://` resource. `person` is
    /// the pane's own tool list, where the person picks the tool: any tool the server lists, whatever
    /// its visibility, since no app page is running.
    pub async fn mcp_app_call(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
        tool: &str,
        arguments: Option<serde_json::Value>,
        initial: bool,
        person: bool,
    ) -> Result<McpToolResult, String> {
        let peer = self.mcp_peer(name, scope, space).await?;
        let tools = peer
            .list_all_tools()
            .await
            .map_err(|e| format!("{name}: list tools: {e}"))?;
        let listed = tools
            .iter()
            .find(|t| t.name == tool)
            .and_then(|t| serde_json::to_value(t).ok())
            .ok_or_else(|| format!("{name} has no tool {tool}"))?;
        let allowed = if person {
            true
        } else if initial {
            linked_resource(&listed).is_some()
        } else {
            app_may_call(&listed)
        };
        if !allowed {
            return Err(format!("{tool} is not visible to an app"));
        }
        self.mcp_call(name, scope, space, tool, arguments).await
    }

    /// The MCP server artifact pane `pane` shows: its name, scope and the id of its space. None
    /// when the pane is no web pane or its entry names no MCP server.
    pub(crate) fn mcp_pane(&self, pane: &str) -> Option<(String, ArtifactScope, String)> {
        let (artifact, cwd, space) = self.spaces.lock().unwrap().spaces.iter().find_map(|s| {
            match crate::spaces::find_pane(&s.root, pane) {
                Some(wheel_proto::PaneKind::Web { artifact, .. }) => {
                    Some((artifact.clone(), s.cwd.clone(), s.id.clone()))
                }
                _ => None,
            }
        })?;
        let entry = self.artifact_entry(&artifact, &cwd).ok()??;
        (entry.mcp.is_some() || entry.mcp_url.is_some())
            .then(|| (artifact.name().to_string(), artifact.scope(), space))
    }
}
