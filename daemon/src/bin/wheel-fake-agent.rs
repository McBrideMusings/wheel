//! A scripted ACP agent for tests, so they never call a real model. Over stdio it:
//! - answers every prompt with `echo: <prompt>`;
//! - first says `context: yes` when the session was created with `_meta.systemPrompt.append`;
//! - for a prompt containing "permission", announces a tool call, asks permission for it, and
//!   reports `permission: <chosen option id>` (or `permission: cancelled`) before the echo.

use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest,
    PromptResponse, RequestPermissionOutcome, RequestPermissionRequest, SessionId,
    SessionNotification, SessionUpdate, StopReason, TextContent, ToolCall, ToolCallUpdate,
    ToolCallUpdateFields,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Result, Stdio};

fn say(cx: &ConnectionTo<Client>, session: &SessionId, text: &str) -> Result<()> {
    cx.send_notification(SessionNotification::new(
        session.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        )))),
    ))
}

#[tokio::main]
async fn main() -> Result<()> {
    let had_context = Arc::new(Mutex::new(false));
    let on_session = had_context.clone();
    Agent
        .builder()
        .name("wheel-fake-agent")
        .on_receive_request(
            async move |init: InitializeRequest, responder, _cx| {
                responder.respond(
                    InitializeResponse::new(init.protocol_version)
                        .agent_capabilities(AgentCapabilities::new()),
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
                responder.respond(NewSessionResponse::new(SessionId::new("fake-session")))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: PromptRequest, responder, cx: ConnectionTo<Client>| {
                let context = *had_context.lock().unwrap();
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
                    say(&worker, &session, &format!("echo: {text}"))?;
                    responder.respond(PromptResponse::new(StopReason::EndTurn))
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await
}
