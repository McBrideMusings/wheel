//! A scripted ACP agent for tests, so they never call a real model. Over stdio it:
//! - answers every prompt with `echo: <prompt>`;
//! - first says `context: yes` when the session was created with `_meta.systemPrompt.append`;
//! - for a prompt containing "permission", announces a tool call, asks permission for it, and
//!   reports `permission: <chosen option id>` (or `permission: cancelled`) before the echo;
//! - for a prompt containing "space", calls `create_space` for `/tmp` on the session's http MCP
//!   server and reports `mcp: <the tool's text>` (or `mcp error: ...`) before the echo.

use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    McpCapabilities, McpServer, McpServerHttp, NewSessionRequest, NewSessionResponse,
    PermissionOption, PermissionOptionKind, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, SessionId, SessionNotification,
    SessionUpdate, StopReason, TextContent, ToolCall, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Result, Stdio};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn say(cx: &ConnectionTo<Client>, session: &SessionId, text: &str) -> Result<()> {
    cx.send_notification(SessionNotification::new(
        session.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        )))),
    ))
}

/// One MCP `tools/call` as a bare HTTP/1.1 POST; the daemon answers a stateless request with
/// one JSON body. Returns the tool's text, or why it failed.
async fn call_tool(
    server: &McpServerHttp,
    tool: &str,
    args: serde_json::Value,
) -> std::result::Result<String, String> {
    let rest = server
        .url
        .strip_prefix("http://")
        .ok_or("not an http url")?;
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    })
    .to_string();
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for header in &server.headers {
        request.push_str(&format!("{}: {}\r\n", header.name, header.value));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    let mut stream = TcpStream::connect(authority)
        .await
        .map_err(|e| e.to_string())?;
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .map_err(|e| e.to_string())?;
    let (head, body) = response.split_once("\r\n\r\n").ok_or("no http body")?;
    let status = head.lines().next().unwrap_or_default();
    if !status.contains(" 200 ") {
        return Err(status.to_string());
    }
    let reply: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let result = &reply["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    if result["isError"] == true {
        Err(text.to_string())
    } else {
        Ok(text.to_string())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let had_context = Arc::new(Mutex::new(false));
    let on_session = had_context.clone();
    let mcp: Arc<Mutex<Option<McpServerHttp>>> = Arc::default();
    let on_session_mcp = mcp.clone();
    Agent
        .builder()
        .name("wheel-fake-agent")
        .on_receive_request(
            async move |init: InitializeRequest, responder, _cx| {
                responder.respond(
                    InitializeResponse::new(init.protocol_version).agent_capabilities(
                        AgentCapabilities::new()
                            .mcp_capabilities(McpCapabilities::new().http(true)),
                    ),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: NewSessionRequest, responder, _cx| {
                let append = req
                    .meta
                    .as_ref()
                    .and_then(|m| m.get("systemPrompt"))
                    .and_then(|p| p.get("append"))
                    .and_then(|a| a.as_str());
                *on_session.lock().unwrap() = append.is_some_and(|a| !a.is_empty());
                *on_session_mcp.lock().unwrap() = req.mcp_servers.iter().find_map(|s| match s {
                    McpServer::Http(http) => Some(http.clone()),
                    _ => None,
                });
                responder.respond(NewSessionResponse::new(SessionId::new("fake-session")))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: PromptRequest, responder, cx: ConnectionTo<Client>| {
                let context = *had_context.lock().unwrap();
                let mcp = mcp.lock().unwrap().clone();
                let worker = cx.clone();
                cx.spawn(async move {
                    let text: String = req
                        .prompt
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text(t) = b {
                                Some(t.text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect();
                    let session = req.session_id.clone();
                    if text.contains("die now") {
                        // Simulate the agent process crashing mid-turn.
                        std::process::exit(3);
                    }
                    if context {
                        say(&worker, &session, "context: yes\n")?;
                    }
                    if text.contains("permission") {
                        worker.send_notification(SessionNotification::new(
                            session.clone(),
                            SessionUpdate::ToolCall(ToolCall::new("call-1", "rm -rf build")),
                        ))?;
                        let ask = RequestPermissionRequest::new(
                            session.clone(),
                            ToolCallUpdate::new(
                                "call-1",
                                ToolCallUpdateFields::new().title("rm -rf build"),
                            ),
                            vec![
                                PermissionOption::new(
                                    "allow-once",
                                    "Allow",
                                    PermissionOptionKind::AllowOnce,
                                ),
                                PermissionOption::new(
                                    "reject-once",
                                    "Deny",
                                    PermissionOptionKind::RejectOnce,
                                ),
                            ],
                        );
                        let outcome = worker.send_request(ask).block_task().await?.outcome;
                        let chosen = match outcome {
                            RequestPermissionOutcome::Selected(s) => s.option_id.to_string(),
                            _ => "cancelled".to_string(),
                        };
                        say(&worker, &session, &format!("permission: {chosen}\n"))?;
                    }
                    if text.contains("space") {
                        let line = match &mcp {
                            Some(server) => match call_tool(
                                server,
                                "create_space",
                                serde_json::json!({ "cwd": "/tmp" }),
                            )
                            .await
                            {
                                Ok(text) => format!("mcp: {text}\n"),
                                Err(e) => format!("mcp error: {e}\n"),
                            },
                            None => "mcp error: the session has no http MCP server\n".into(),
                        };
                        say(&worker, &session, &line)?;
                    }
                    say(&worker, &session, &format!("echo: {text}"))?;
                    responder.respond(PromptResponse::new(StopReason::EndTurn))
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await
}
