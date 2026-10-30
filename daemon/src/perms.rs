//! Permission requests waiting on a person. Every connected client sees each request, and the
//! first answer resolves it; any later answer is refused.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tokio::sync::{broadcast, oneshot};
use wheel_proto::{PermInfo, PermWhy, ServerMsg};

struct Pending {
    info: PermInfo,
    answer: oneshot::Sender<String>,
}

pub struct Perms {
    next_id: AtomicU64,
    pending: Mutex<HashMap<String, Pending>>,
    /// Daemon-wide events every connection forwards.
    events: broadcast::Sender<ServerMsg>,
}

impl Perms {
    pub fn new(events: broadcast::Sender<ServerMsg>) -> Perms {
        Perms {
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            events,
        }
    }

    /// Register a request, announce it, and wait for the chosen option id. Dropping the future
    /// first withdraws the request, so a caller that gave up leaves nothing waiting.
    pub async fn ask(
        &self,
        session: &str,
        title: String,
        options: Vec<wheel_proto::PermOption>,
    ) -> Result<String, String> {
        let (id, answer) = self.open(session, title, options, None, PermWhy::default());
        let _withdraw = WithdrawOnDrop { perms: self, id };
        answer
            .await
            .map_err(|_| "the request was withdrawn before anyone answered".to_string())
    }

    /// Register a request and announce it. The receiver yields the chosen option id. The request
    /// waits until answered or `drop_session`; dropping the receiver does not withdraw it.
    /// `tool_call` is the agent's id for the tool call the request gates, when an agent asked.
    pub fn open(
        &self,
        session: &str,
        title: String,
        options: Vec<wheel_proto::PermOption>,
        tool_call: Option<String>,
        why: PermWhy,
    ) -> (String, oneshot::Receiver<String>) {
        let id = format!("p{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let info = PermInfo {
            id: id.clone(),
            session: session.to_string(),
            title,
            options,
            tool_call,
            why,
        };
        let (tx, rx) = oneshot::channel();
        // The title and reason can quote what an agent wants to run, so the log gets their sizes; the rule is the one program the person is asked to allow.
        tracing::info!(perm = %id, session, tool_call = info.tool_call.as_deref().unwrap_or("-"), title_chars = info.title.chars().count(), reason_chars = info.why.reason.as_deref().map_or(0, |r| r.chars().count()), always_rule = info.why.always_rule.as_deref().unwrap_or("-"), "perm requested");
        self.pending.lock().unwrap().insert(
            id.clone(),
            Pending {
                info: info.clone(),
                answer: tx,
            },
        );
        let _ = self.events.send(ServerMsg::PermRequested { perm: info });
        (id, rx)
    }

    /// Resolve a request. `choice` is an option id, or `allow` / `deny` for the first
    /// allow-once / reject-once option. Returns the option id chosen.
    pub fn answer(&self, id: &str, choice: &str, by: &str) -> Result<String, String> {
        let mut pending = self.pending.lock().unwrap();
        let Some(p) = pending.get(id) else {
            return Err(format!(
                "no waiting permission {id}; it may already be answered"
            ));
        };
        let wanted = match choice {
            "allow" => Some("allow_once"),
            "deny" => Some("reject_once"),
            _ => None,
        };
        let option = p
            .info
            .options
            .iter()
            .find(|o| match wanted {
                Some(kind) => o.kind == kind,
                None => o.id == choice,
            })
            .map(|o| o.id.clone())
            .ok_or_else(|| {
                format!(
                    "{id} has no option {choice:?}; options: {}",
                    p.info
                        .options
                        .iter()
                        .map(|o| o.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let p = pending.remove(id).expect("checked above");
        drop(pending);
        if p.answer.send(option.clone()).is_err() {
            // The turn that asked is already over; the agent got Cancelled.
            let _ = self.events.send(ServerMsg::PermResolved {
                id: id.to_string(),
                choice: "withdrawn".into(),
            });
            return Err(format!(
                "{id} was withdrawn: the session no longer waits for it"
            ));
        }
        tracing::info!(perm = %id, choice = %option, by, "perm answered");
        let _ = self.events.send(ServerMsg::PermResolved {
            id: id.to_string(),
            choice: option.clone(),
        });
        Ok(option)
    }

    /// A request the agent withdrew (its turn was cancelled or its process died).
    pub fn drop_session(&self, session: &str) {
        let mut pending = self.pending.lock().unwrap();
        let gone: Vec<String> = pending
            .iter()
            .filter(|(_, p)| p.info.session == session)
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            pending.remove(&id);
            self.withdrawn(id, session);
        }
    }

    /// Withdraw one request if it is still waiting.
    fn withdraw(&self, id: &str) {
        let removed = self.pending.lock().unwrap().remove(id);
        if let Some(p) = removed {
            self.withdrawn(id.to_string(), &p.info.session);
        }
    }

    fn withdrawn(&self, id: String, session: &str) {
        tracing::info!(perm = %id, session, "perm withdrawn");
        let _ = self.events.send(ServerMsg::PermResolved {
            id,
            choice: "withdrawn".into(),
        });
    }

    /// The daemon-wide events, for a caller that follows requests being asked and answered.
    pub fn subscribe(&self) -> broadcast::Receiver<ServerMsg> {
        self.events.subscribe()
    }

    pub fn list(&self) -> Vec<PermInfo> {
        let mut list: Vec<_> = self
            .pending
            .lock()
            .unwrap()
            .values()
            .map(|p| p.info.clone())
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }
}

struct WithdrawOnDrop<'a> {
    perms: &'a Perms,
    id: String,
}

impl Drop for WithdrawOnDrop<'_> {
    fn drop(&mut self) {
        self.perms.withdraw(&self.id);
    }
}
