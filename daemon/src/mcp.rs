//! The daemon as an MCP client: an artifact entry that names a server (`mcp`, a command run
//! through `sh -c`, or `mcp_url`, a streamable HTTP endpoint) gets one session, held here and
//! shared by every caller of `artifact tools` and `artifact call`.
//!
//! A session is made on the first call that needs it and replaced when the entry's definition
//! changes or the server's end of the connection closes. A project artifact's stdio server runs
//! in the space's directory with the login PATH; its process dies with its session.
//!
//! An entry that says `shared = true` is one process for every space, and entries that name the
//! same server share it (ADR-0028). The daemon names
//! the asking pane, its project and its artifact in `_meta` on every call and read (`wheel/pane`,
//! `wheel/dir`, `wheel/artifact`),
//! reads a resource as `<uri>?dir=<project>`, and subscribes to that URI while a pane shows the
//! project (`resources/subscribe`, `resources/unsubscribe`). The server answers a change with
//! `notifications/resources/updated` for the URI, which reaches the panes showing that project.

use std::collections::{HashMap, HashSet};
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
    service: RunningService<RoleClient, Watch>,
    subs: Subs,
}

/// One subscribed resource: the project it is read for and the artifacts whose panes read it.
struct Sub {
    dir: String,
    names: HashSet<String>,
}

/// The resources a shared session is subscribed to, by URI (with its `?dir=`). The session's
/// notification handler reads it to find the panes a change reaches.
type Subs = Arc<Mutex<HashMap<String, Sub>>>;

/// What a caller holds of a session: the peer, the project directory it asks for, and the key the
/// session is held under, so a dead one can be dropped.
pub(crate) struct Conn {
    pub peer: Peer<RoleClient>,
    pub dir: String,
    key: String,
    name: String,
    shared: bool,
}

impl Conn {
    /// `_meta` for a request to a shared server: the asking pane, its project and the artifact it
    /// shows, which is how one server tells its views apart.
    fn meta(&self, pane: Option<&str>) -> Option<rmcp::model::RequestMetaObject> {
        if !self.shared {
            return None;
        }
        let mut map = serde_json::Map::new();
        map.insert("wheel/dir".into(), self.dir.clone().into());
        map.insert("wheel/artifact".into(), self.name.clone().into());
        if let Some(pane) = pane {
            map.insert("wheel/pane".into(), pane.into());
        }
        Some(rmcp::model::RequestMetaObject(rmcp::model::MetaObject(map)))
    }
}

impl Conn {
    /// Every tool of the server, listed with the same `_meta` a call carries, so a shared server
    /// lists the tools of the artifact asking, not of every artifact it serves.
    async fn list_tools(&self) -> Result<Vec<rmcp::model::Tool>, rmcp::ServiceError> {
        let mut tools = Vec::new();
        let mut cursor = None;
        loop {
            let mut params = rmcp::model::PaginatedRequestParams::default().with_cursor(cursor);
            params.meta = self.meta(None);
            let result = self.peer.list_tools(Some(params)).await?;
            tools.extend(result.tools);
            cursor = result.next_cursor;
            if cursor.is_none() {
                return Ok(tools);
            }
        }
    }
}

/// `uri` read for project `dir`: the same resource with `?dir=<dir>` added.
fn project_uri(uri: &str, dir: &str) -> String {
    match url::Url::parse(uri) {
        Ok(mut u) => {
            u.query_pairs_mut().append_pair("dir", dir);
            u.to_string()
        }
        Err(_) => uri.to_string(),
    }
}

/// Whether a request failed because the server's end is gone.
fn is_closed(e: &rmcp::ServiceError) -> bool {
    matches!(e, rmcp::ServiceError::TransportClosed)
}

/// The sessions of every artifact. A shared server is keyed by its scope and what runs it, so
/// entries naming the same command share one; any other is keyed by scope and name and the space's
/// directory. Each key has a lock of its own, so one server connecting never holds up another's
/// calls.
#[derive(Default)]
pub(crate) struct Sessions {
    by_key: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Option<Session>>>>>,
    /// The shared session each shared artifact (`scope\0name`) last used, so a session no entry
    /// names any more can end.
    owners: Mutex<HashMap<String, String>>,
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

    /// Record that the shared artifact `owner` now uses the session under `key`. Returns the
    /// key it used before when it moved off it.
    fn claim(&self, owner: &str, key: &str) -> Option<String> {
        let before = self.owners.lock().unwrap().insert(owner.to_string(), key.to_string())?;
        (before != key).then_some(before)
    }

    /// Record that `owner` uses no shared session. Returns the key it used.
    fn release(&self, owner: &str) -> Option<String> {
        self.owners.lock().unwrap().remove(owner)
    }

    fn in_use(&self, key: &str) -> bool {
        self.owners.lock().unwrap().values().any(|k| k == key)
    }
}

/// The client side of a session: its `notifications/resources/updated`, which reach every connection as `mcp_updated` for each pane
/// showing the artifact: the panes of the project a `?dir=` names, or of this session's own
/// directory.
#[derive(Clone)]
struct Watch {
    state: std::sync::Weak<AppState>,
    scope: ArtifactScope,
    name: String,
    shared: bool,
    cwd: String,
    subs: Subs,
}

impl rmcp::ClientHandler for Watch {
    async fn on_resource_updated(
        &self,
        params: rmcp::model::ResourceUpdatedNotificationParam,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        let Some(state) = self.state.upgrade() else { return };
        // A shared session serves several artifacts: the change reaches the panes of the
        // artifacts that read this resource, in the project it was read for.
        let panes: Vec<String> = if self.shared {
            let subs = self.subs.lock().unwrap();
            match subs.get(&params.uri) {
                Some(sub) => sub.names.iter().flat_map(|n| state.mcp_panes(self.scope, n, Some(&sub.dir))).collect(),
                None => Vec::new(),
            }
        } else {
            state.mcp_panes(self.scope, &self.name, Some(&self.cwd))
        };
        for pane in panes {
            let _ = state.global.send(wheel_proto::ServerMsg::McpUpdated { pane });
        }
    }
}

impl Def {
    /// What names this server in a session key: two entries with the same one run the same server.
    fn key(&self) -> String {
        match self {
            Def::Stdio(command) => format!("sh\0{command}"),
            Def::Http(url) => format!("http\0{url}"),
        }
    }

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
    /// A connection to artifact `name`'s MCP session, connecting first when there is none.
    async fn mcp_peer(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
    ) -> Result<Conn, String> {
        let cwd = self.artifact_space(space)?;
        let artifact = self.artifact_ref(scope, name, &cwd)?;
        let owner = format!("{}\0{}", artifact.scope().name(), artifact.name());
        let Some(entry) = self.artifact_entry(&artifact, &cwd)? else {
            self.mcp_release(&owner).await;
            return Err(format!(
                "no {} artifact {name}: {} has no entry for it",
                artifact.scope().name(),
                self.artifact_registry(artifact.scope(), &cwd).display()
            ));
        };
        if let Some(problem) = entry.problems().first() {
            return Err(format!("the entry for {name} is invalid: {problem}"));
        }
        let def = Def::of(&entry)?;
        // One process for every space only where the entry declares its server serves them all
        // (`shared = true`, machine scope); every other server is one process per space.
        if entry.shared && artifact.scope() != ArtifactScope::Machine {
            return Err(format!("the entry for {name} is shared, which only a machine artifact can be"));
        }
        let shared = entry.shared;
        let key = if shared {
            format!("shared\0{}\0{}", artifact.scope().name(), def.key())
        } else {
            format!("space\0{}\0{}\0{}", artifact.scope().name(), artifact.name(), cwd)
        };
        // An entry that moved off a shared server, or is not shared, leaves it for the entries
        // still naming it; with none left it ends.
        let left = if shared { self.mcp.claim(&owner, &key) } else { self.mcp.release(&owner) };
        if let Some(left) = left {
            self.mcp_end_unused(&left, name).await;
        }
        let slot = self.mcp.slot(&key);
        let mut held = slot.lock().await;
        if let Some(session) = held.as_mut() {
            if session.def == def && !session.service.is_closed() {
                if shared {
                    self.mcp_unsubscribe_closed(session, artifact.scope()).await;
                }
                return Ok(Conn { peer: session.service.peer().clone(), dir: cwd, key, name: name.to_string(), shared });
            }
        }
        // Dropping the old session ends its process, or its HTTP session, before the new one starts.
        *held = None;
        let subs = Subs::default();
        let watch = Watch {
            state: self.me.clone(),
            scope: artifact.scope(),
            name: name.to_string(),
            shared,
            cwd: cwd.clone(),
            subs: subs.clone(),
        };
        let service = tokio::time::timeout(CONNECT, self.mcp_connect(&def, &cwd, name, watch, shared))
            .await
            .map_err(|_| {
                format!(
                    "{name}: the MCP server did not finish starting within {}s",
                    CONNECT.as_secs()
                )
            })??;
        tracing::info!(scope = artifact.scope().name(), %name, ?def, shared, "MCP session open");
        let peer = service.peer().clone();
        *held = Some(Session { def, service, subs });
        Ok(Conn { peer, dir: cwd, key, name: name.to_string(), shared })
    }

    /// Unsubscribe from the resources no pane reads any more: a resource stays while a pane of
    /// an artifact that read it still shows its project.
    async fn mcp_unsubscribe_closed(&self, session: &mut Session, scope: ArtifactScope) {
        let closed: Vec<String> = {
            let mut subs = session.subs.lock().unwrap();
            let mut closed = Vec::new();
            subs.retain(|uri, sub| {
                sub.names.retain(|name| self.mcp_roots(scope, name).contains(&sub.dir));
                if sub.names.is_empty() {
                    closed.push(uri.clone());
                }
                !sub.names.is_empty()
            });
            closed
        };
        for uri in closed {
            let _ = session.service.peer().unsubscribe(rmcp::model::UnsubscribeRequestParams::new(uri)).await;
        }
    }

    /// Subscribe to `uri` for project `dir`, once per resource, for a shared server, and note
    /// that this artifact's panes read it.
    async fn mcp_subscribe(&self, conn: &Conn, uri: &str) {
        let slot = self.mcp.slot(&conn.key);
        let held = slot.lock().await;
        let Some(session) = held.as_ref() else { return };
        let first = {
            let mut subs = session.subs.lock().unwrap();
            match subs.get_mut(uri) {
                Some(sub) => {
                    sub.names.insert(conn.name.clone());
                    false
                }
                None => {
                    let names = HashSet::from([conn.name.clone()]);
                    subs.insert(uri.to_string(), Sub { dir: conn.dir.clone(), names });
                    true
                }
            }
        };
        if first
            && session
                .service
                .peer()
                .subscribe(rmcp::model::SubscribeRequestParams::new(uri.to_string()))
                .await
                .is_err()
        {
            session.subs.lock().unwrap().remove(uri);
        }
    }

    /// Forget that `owner` uses a shared session, ending it when no entry uses it.
    pub(crate) async fn mcp_release(&self, owner: &str) {
        if let Some(left) = self.mcp.release(owner) {
            self.mcp_end_unused(&left, owner).await;
        }
    }

    /// End the shared session under `key` unless an entry uses it. Checked under the session's
    /// lock, so an entry that connects to it meanwhile keeps it.
    async fn mcp_end_unused(&self, key: &str, name: &str) {
        let slot = self.mcp.slot(key);
        let mut held = slot.lock().await;
        if !self.mcp.in_use(key) && held.take().is_some() {
            tracing::info!(%name, "MCP session no entry names any more; ended it");
        }
    }

    /// Drop the session under `key`: its process ends, and the next call starts a new one.
    async fn mcp_evict(&self, key: &str) {
        *self.mcp.slot(key).lock().await = None;
    }

    async fn mcp_connect(
        &self,
        def: &Def,
        cwd: &str,
        name: &str,
        watch: Watch,
        shared: bool,
    ) -> Result<RunningService<RoleClient, Watch>, String> {
        match def {
            Def::Stdio(command) => {
                let mut cmd = tokio::process::Command::new("/bin/sh");
                // A shared server belongs to no project, so it starts in the home directory
                // and is told the projects as roots.
                let dir = if shared { std::env::var("HOME").unwrap_or_else(|_| cwd.to_string()) } else { cwd.to_string() };
                cmd.arg("-c")
                    .arg(command)
                    .current_dir(dir)
                    .envs(crate::shell::command_env(
                        self.control.lock().unwrap().as_ref(),
                        &self.instance_dir,
                        &self.login_path.lock().unwrap().path,
                    ));
                if !shared {
                    cmd.env("WHEEL_SPACE_DIR", cwd);
                }
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
                watch.serve(transport)
                    .await
                    .map_err(|e| format!("{name}: {command:?} did not start an MCP session: {e}"))
            }
            Def::Http(url) => {
                // A https URL needs the process's TLS provider, which nothing else may have set.
                let _ = rustls::crypto::ring::default_provider().install_default();
                watch.serve(StreamableHttpClientTransport::from_uri(url.as_str()))
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
        _pane: Option<&str>,
    ) -> Result<Vec<McpTool>, String> {
        let conn = self.mcp_peer(name, scope, space).await?;
        let tools = match conn.list_tools().await {
            Ok(t) => t,
            Err(e) => {
                if is_closed(&e) {
                    self.mcp_evict(&conn.key).await;
                }
                return Err(format!("{name}: list tools: {e}"));
            }
        };
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
        pane: Option<&str>,
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
        let conn = self.mcp_peer(name, scope, space).await?;
        let mut params = CallToolRequestParams::new(tool.to_string());
        params.arguments = arguments;
        params.meta = conn.meta(pane);
        // A call may have run before the server's end went away, so a dead session is dropped
        // and the error returned: the next call starts a new one.
        let result = match conn.peer.call_tool(params).await {
            Ok(r) => r,
            Err(e) => {
                if is_closed(&e) {
                    self.mcp_evict(&conn.key).await;
                }
                return Err(format!("{name}: call {tool}: {e}"));
            }
        };
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
        pane: Option<&str>,
        uri: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let read = self.mcp_read(name, scope, space, pane, uri).await?;
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
        let conn = self.mcp_peer(name, scope, space).await?;
        let peer = conn.peer.clone();
        let tools = conn
            .list_tools()
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
            let read = self.mcp_read(name, scope, space, None, &uri).await?;
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
        pane: Option<&str>,
        uri: &str,
    ) -> Result<serde_json::Value, String> {
        let conn = self.mcp_peer(name, scope, space).await?;
        let uri = &if conn.shared { project_uri(uri, &conn.dir) } else { uri.to_string() };
        if conn.shared {
            self.mcp_subscribe(&conn, uri).await;
        }
        let mut params = rmcp::model::ReadResourceRequestParams::new(uri.to_string());
        params.meta = conn.meta(pane);
        let read = match conn.peer.read_resource(params).await {
            Ok(r) => r,
            Err(e) => {
                if is_closed(&e) {
                    self.mcp_evict(&conn.key).await;
                }
                return Err(format!("{name}: read {uri}: {e}"));
            }
        };
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
            let conn = self.mcp_peer(name, scope, space).await?;
            let listed = conn.peer.list_all_resources().await.unwrap_or_default();
            if !listed.iter().any(|r| r.uri == uri) {
                return Err(format!("{uri} is not a resource the server lists"));
            }
        }
        self.mcp_read(name, scope, space, None, uri).await
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
        let conn = self.mcp_peer(name, scope, space).await?;
        let tools = conn
            .list_tools()
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
        self.mcp_call(name, scope, space, None, tool, arguments).await
    }

    /// The web panes showing artifact `name` of `scope`, in the spaces whose directory is `cwd`
    /// (every space when `cwd` is none).
    fn mcp_panes(&self, scope: ArtifactScope, name: &str, cwd: Option<&str>) -> Vec<String> {
        let spaces = self.spaces.lock().unwrap();
        spaces
            .spaces
            .iter()
            .filter(|s| cwd.map_or(true, |c| s.cwd == c))
            .flat_map(crate::spaces::space_panes)
            .filter_map(|(pane, kind)| match kind {
                wheel_proto::PaneKind::Web { artifact, .. }
                    if artifact.scope() == scope && artifact.name() == name =>
                {
                    Some(pane.to_string())
                }
                _ => None,
            })
            .collect()
    }

    /// The directories of the spaces showing artifact `name` of `scope`, sorted: the projects a
    /// shared server is subscribed to.
    fn mcp_roots(&self, scope: ArtifactScope, name: &str) -> Vec<String> {
        let spaces = self.spaces.lock().unwrap();
        let mut dirs: Vec<String> = spaces
            .spaces
            .iter()
            .filter(|s| {
                crate::spaces::space_panes(s).iter().any(|(_, kind)| {
                    matches!(kind, wheel_proto::PaneKind::Web { artifact, .. }
                        if artifact.scope() == scope && artifact.name() == name)
                })
            })
            .map(|s| s.cwd.clone())
            .collect();
        dirs.sort();
        dirs.dedup();
        dirs
    }

    /// The MCP server artifact pane `pane` shows: its name, scope and the id of its space. None
    /// when the pane is no web pane or its entry names no MCP server.
    pub(crate) fn mcp_pane(&self, pane: &str) -> Option<(String, ArtifactScope, String)> {
        let (artifact, cwd, space) = self.spaces.lock().unwrap().spaces.iter().find_map(|s| {
            match crate::spaces::space_find(s, pane) {
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
