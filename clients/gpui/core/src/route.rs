//! One connection to a wheeld, shared by every window of the app that shows that wheeld's
//! sessions (ADR-0057, 8B). Each subscriber is a small index `n`; a request it sends goes out with
//! `req` rewritten to `(n << 40) | req`, and the reply comes back to `n` alone with its own `req`.
//! A frame with no `req`, or with index 0 (the connection's own `status`), goes to every
//! subscriber. wheeld keeps some state per socket rather than per request, so those ops are
//! merged here: an agent stream and a feed stay attached while any subscriber holds them, a
//! process watch follows the latest subscriber that asked, and a render or window command goes to
//! the subscribers that offered to answer it.
use crate::daemon::Event;
use futures::channel::mpsc as fmpsc;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use wheel_proto::ServerMsg;

/// A request id's low bits: what the subscriber chose. The high bits name the subscriber.
const SHIFT: u32 = 40;
const MASK: u64 = (1 << SHIFT) - 1;

/// The subscriber a rewritten `req` belongs to, and the `req` it sent.
pub fn split(req: u64) -> (u32, u64) {
    ((req >> SHIFT) as u32, req & MASK)
}
pub fn join(n: u32, req: u64) -> u64 {
    (u64::from(n) << SHIFT) | (req & MASK)
}

/// What a subscriber's frame becomes on the way to wheeld.
#[derive(Debug, PartialEq)]
pub enum Outgoing {
    /// Send this text.
    Send(String),
    /// Send nothing: the merged state already says it.
    Hold,
    /// Send nothing and answer the subscriber with this frame.
    Answer(ServerMsg),
}

/// What one socket's lifetime holds per subscriber; a new socket starts empty, and each window
/// asks again after `Up`.
#[derive(Default)]
struct Held {
    /// Subscribers attached to each agent's stream.
    agents: HashMap<String, BTreeSet<u32>>,
    /// Subscribers that attached with no `req` while another already held the agent: wheeld
    /// replays the agent once more, and that replay is theirs alone.
    replays: HashMap<String, VecDeque<u32>>,
    /// Each feed's holders and the `req` each subscribed with, by wheeld's own key.
    feeds: HashMap<(String, String, String), BTreeMap<u32, Option<u64>>>,
    /// Who asked to watch which space's processes, oldest first; the last one is watched.
    procs: Vec<(u32, String)>,
    procs_sent: Option<String>,
    /// Subscribers that sent `render_offer` / `window_offer`.
    render_offered: BTreeSet<u32>,
    window_offered: BTreeSet<u32>,
    /// A `render_pane` or `inspect_pane` sent to several subscribers: who has not answered yet.
    renders: HashMap<u64, BTreeSet<u32>>,
}

#[derive(Default)]
pub struct Router {
    pub subscribers: BTreeMap<u32, fmpsc::UnboundedSender<Event>>,
    held: Held,
}

impl Router {
    /// A new socket opened: what the last one held is gone.
    pub fn reset(&mut self) {
        self.held = Held::default();
    }

    pub fn join(&mut self, n: u32, events: fmpsc::UnboundedSender<Event>) {
        self.subscribers.insert(n, events);
    }

    /// Subscriber `n` left. The frames that undo what only it held, for a live socket.
    pub fn leave(&mut self, n: u32) -> Vec<String> {
        self.subscribers.remove(&n);
        let held = &mut self.held;
        let mut out = vec![];
        for (agent, holders) in &mut held.agents {
            if holders.remove(&n) && holders.is_empty() {
                out.push(json!({"op": "agent_detach", "agent": agent}).to_string());
            }
        }
        held.agents.retain(|_, h| !h.is_empty());
        for q in held.replays.values_mut() {
            q.retain(|s| *s != n);
        }
        for ((pane, feed, params), holders) in &mut held.feeds {
            if holders.remove(&n).is_some() && holders.is_empty() {
                let params: Value = serde_json::from_str(params).unwrap_or(Value::Null);
                out.push(json!({"op": "feed_unsubscribe", "pane": pane, "feed": feed, "params": params}).to_string());
            }
        }
        held.feeds.retain(|_, h| !h.is_empty());
        held.procs.retain(|(s, _)| *s != n);
        out.extend(self.procs_change());
        self.held.render_offered.remove(&n);
        self.held.window_offered.remove(&n);
        // A render waiting on the one who left: its silence is a refusal.
        let ids: Vec<u64> = self.held.renders.keys().copied().collect();
        for id in ids {
            out.extend(self.refused(id, n, "not shown"));
        }
        out
    }

    fn procs_change(&mut self) -> Option<String> {
        let now = self.held.procs.last().map(|(_, space)| space.clone());
        if now == self.held.procs_sent {
            return None;
        }
        self.held.procs_sent = now.clone();
        // hack: one space's processes stream per wheeld at a time, the latest window's; two windows
        // with Processes open on different spaces of one wheeld see only the newer one live. Upgrade
        // when wheeld keys procs_watch by window.
        Some(json!({"op": "procs_watch", "space": now}).to_string())
    }

    /// Subscriber `n` refused render `id` (or left): wheeld hears the refusal once every
    /// subscriber asked has refused.
    fn refused(&mut self, id: u64, n: u32, message: &str) -> Option<String> {
        let waiting = self.held.renders.get_mut(&id)?;
        if !waiting.remove(&n) || !waiting.is_empty() {
            return None;
        }
        self.held.renders.remove(&id);
        Some(json!({"op": "render_refused", "id": id, "message": message}).to_string())
    }

    /// Subscriber `n` sends `text`.
    pub fn outgoing(&mut self, n: u32, text: &str) -> Outgoing {
        let Ok(Value::Object(mut frame)) = serde_json::from_str::<Value>(text) else { return Outgoing::Send(text.to_owned()) };
        let op = frame.get("op").and_then(Value::as_str).unwrap_or_default().to_owned();
        let req = frame.get("req").and_then(Value::as_u64);
        let s = |frame: &serde_json::Map<String, Value>, k: &str| frame.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let held = &mut self.held;
        match op.as_str() {
            "agent_attach" => {
                let holders = held.agents.entry(s(&frame, "agent")).or_default();
                if req.is_none() && !holders.is_empty() && !holders.contains(&n) {
                    held.replays.entry(s(&frame, "agent")).or_default().push_back(n);
                }
                holders.insert(n);
            }
            "agent_detach" => {
                let agent = s(&frame, "agent");
                let Some(holders) = held.agents.get_mut(&agent) else { return Outgoing::Hold };
                holders.remove(&n);
                if !holders.is_empty() {
                    return Outgoing::Hold;
                }
                held.agents.remove(&agent);
            }
            "feed_subscribe" => {
                let key = feed_key(&frame);
                held.feeds.entry(key).or_default().insert(n, req);
            }
            "feed_unsubscribe" => {
                let key = feed_key(&frame);
                let Some(holders) = held.feeds.get_mut(&key) else { return answer_ok(n, req) };
                holders.remove(&n);
                if !holders.is_empty() {
                    return answer_ok(n, req);
                }
                held.feeds.remove(&key);
            }
            "procs_watch" => {
                held.procs.retain(|(s, _)| *s != n);
                if let Some(space) = frame.get("space").and_then(Value::as_str) {
                    held.procs.push((n, space.to_owned()));
                }
                return match self.procs_change() {
                    Some(text) if req.is_none() => Outgoing::Send(text),
                    Some(text) => {
                        let mut v: Value = serde_json::from_str(&text).expect("json");
                        v["req"] = json!(join(n, req.unwrap_or_default()));
                        Outgoing::Send(v.to_string())
                    }
                    None => answer_ok(n, req),
                };
            }
            "render_offer" => {
                held.render_offered.insert(n);
            }
            "window_offer" => {
                held.window_offered.insert(n);
            }
            "render_refused" => {
                let id = frame.get("id").and_then(Value::as_u64).unwrap_or_default();
                if held.renders.contains_key(&id) {
                    let message = s(&frame, "message");
                    return self.refused(id, n, &message).map_or(Outgoing::Hold, Outgoing::Send);
                }
            }
            "rendered" | "inspected" | "recorded" => {
                if let Some(id) = frame.get("id").and_then(Value::as_u64) {
                    held.renders.remove(&id);
                }
            }
            _ => {}
        }
        match req {
            Some(r) if n > 0 => {
                frame.insert("req".into(), json!(join(n, r)));
                Outgoing::Send(Value::Object(frame).to_string())
            }
            _ => Outgoing::Send(text.to_owned()),
        }
    }

    /// The subscribers a frame from wheeld goes to, each with the frame it gets. `raw` is the
    /// frame's text, read for its `req`.
    pub fn incoming(&mut self, message: ServerMsg, raw: &str) -> Vec<(u32, ServerMsg)> {
        let everyone = || self.subscribers.keys().copied().collect::<Vec<_>>();
        let to = |ns: Vec<u32>, m: ServerMsg| ns.into_iter().map(|n| (n, m.clone())).collect::<Vec<_>>();
        match &message {
            ServerMsg::WindowRequest { .. } => {
                // One app answers a window command once: the first window that offered.
                let n = self.held.window_offered.iter().next().copied().or_else(|| everyone().first().copied());
                return n.map(|n| vec![(n, message)]).unwrap_or_default();
            }
            ServerMsg::RenderPane { id, .. } | ServerMsg::InspectPane { id, .. } => {
                let asked: BTreeSet<u32> = self.held.render_offered.iter().copied().filter(|n| self.subscribers.contains_key(n)).collect();
                if asked.is_empty() {
                    return vec![];
                }
                self.held.renders.insert(*id, asked.clone());
                return to(asked.into_iter().collect(), message);
            }
            ServerMsg::Agent { agent, .. } => {
                let holders = self.held.agents.get(agent).map(|h| h.iter().copied().collect()).unwrap_or_default();
                return to(holders, message);
            }
            ServerMsg::AgentAttached { req: None, agent, .. } => {
                if let Some(n) = self.held.replays.get_mut(agent).and_then(VecDeque::pop_front) {
                    return vec![(n, message)];
                }
                let holders = self.held.agents.get(agent).map(|h| h.iter().copied().collect()).unwrap_or_default();
                return to(holders, message);
            }
            ServerMsg::FeedUpdate { pane, feed, params, .. } => {
                let key = (pane.clone(), feed.clone(), params.to_string());
                let Some(holders) = self.held.feeds.get(&key) else { return vec![] };
                let mut out = vec![];
                for (n, req) in holders.clone() {
                    let mut m = message.clone();
                    if let ServerMsg::FeedUpdate { req: r, .. } = &mut m {
                        *r = req;
                    }
                    out.push((n, m));
                }
                return out;
            }
            _ => {}
        }
        #[derive(serde::Deserialize)]
        struct Req {
            req: Option<u64>,
        }
        let req = serde_json::from_str::<Req>(raw).ok().and_then(|r| r.req);
        match req.map(split) {
            Some((n, r)) if n > 0 => {
                let mut v = serde_json::to_value(&message).expect("a ServerMsg serializes");
                v["req"] = json!(r);
                match serde_json::from_value(v) {
                    Ok(m) => vec![(n, m)],
                    Err(_) => vec![(n, message)],
                }
            }
            Some((_, r)) => {
                let mut v = serde_json::to_value(&message).expect("a ServerMsg serializes");
                v["req"] = json!(r);
                let m: ServerMsg = serde_json::from_value(v).unwrap_or(message);
                to(everyone(), m)
            }
            None => to(everyone(), message),
        }
    }
}

fn feed_key(frame: &serde_json::Map<String, Value>) -> (String, String, String) {
    let s = |k: &str| frame.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    (s("pane"), s("feed"), frame.get("params").cloned().unwrap_or(Value::Null).to_string())
}

fn answer_ok(_n: u32, req: Option<u64>) -> Outgoing {
    match req {
        Some(req) => Outgoing::Answer(ServerMsg::Ok { req }),
        None => Outgoing::Hold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router(n: u32) -> (Router, Vec<fmpsc::UnboundedReceiver<Event>>) {
        let mut r = Router::default();
        let mut rx = vec![];
        for i in 1..=n {
            let (s, x) = fmpsc::unbounded();
            r.join(i, s);
            rx.push(x);
        }
        (r, rx)
    }
    fn sent(o: Outgoing) -> Value {
        match o {
            Outgoing::Send(t) => serde_json::from_str(&t).unwrap(),
            other => panic!("not sent: {other:?}"),
        }
    }
    fn msg(v: Value) -> (ServerMsg, String) {
        (serde_json::from_value(v.clone()).unwrap(), v.to_string())
    }

    #[test]
    fn a_reply_goes_to_the_window_that_asked_with_its_own_req() {
        let (mut r, _rx) = router(2);
        let out = sent(r.outgoing(2, r#"{"op":"settings_ls","req":7}"#));
        assert_eq!(out["req"], json!(join(2, 7)));
        let (m, raw) = msg(json!({"ev":"ok","req": join(2, 7)}));
        assert_eq!(r.incoming(m, &raw), vec![(2, ServerMsg::Ok { req: 7 })]);
        // Index 0 is the connection's own status: every window gets it, req restored.
        let (m, raw) = msg(json!({"ev":"ok","req": 1}));
        assert_eq!(r.incoming(m, &raw), vec![(1, ServerMsg::Ok { req: 1 }), (2, ServerMsg::Ok { req: 1 })]);
    }

    #[test]
    fn an_agent_detaches_only_when_its_last_window_lets_go_and_a_second_attach_replays_to_it_alone() {
        let (mut r, _rx) = router(2);
        sent(r.outgoing(1, r#"{"op":"agent_attach","agent":"a"}"#));
        sent(r.outgoing(2, r#"{"op":"agent_attach","agent":"a"}"#));
        let (m, raw) = msg(json!({"ev":"agent_attached","agent":"a","now":0,"state":{"phase":"ready"},"events":[]}));
        let to: Vec<u32> = r.incoming(m, &raw).into_iter().map(|(n, _)| n).collect();
        assert_eq!(to, [2], "the replay is the second window's");
        let (m, raw) = msg(json!({"ev":"agent","agent":"a","event":{"kind":"text","text":"x"}}));
        assert_eq!(r.incoming(m, &raw).len(), 2);
        assert_eq!(r.outgoing(1, r#"{"op":"agent_detach","agent":"a"}"#), Outgoing::Hold);
        assert_eq!(r.leave(2), vec![json!({"op":"agent_detach","agent":"a"}).to_string()]);
    }

    #[test]
    fn a_feed_is_subscribed_once_and_each_holder_hears_it_with_its_req() {
        let (mut r, _rx) = router(2);
        let f = |req| format!(r#"{{"op":"feed_subscribe","req":{req},"pane":"p","feed":"q","params":{{"dir":"/d"}}}}"#);
        sent(r.outgoing(1, &f(5)));
        sent(r.outgoing(2, &f(9)));
        let (m, raw) = msg(json!({"ev":"feed_update","req": join(2, 9),"pane":"p","feed":"q","params":{"dir":"/d"},"value":1}));
        let reqs: Vec<(u32, Option<u64>)> = r.incoming(m, &raw).into_iter().map(|(n, m)| (n, match m { ServerMsg::FeedUpdate { req, .. } => req, _ => None })).collect();
        assert_eq!(reqs, [(1, Some(5)), (2, Some(9))]);
        let un = r#"{"op":"feed_unsubscribe","pane":"p","feed":"q","params":{"dir":"/d"}}"#;
        assert_eq!(r.outgoing(1, un), Outgoing::Hold);
        assert_eq!(sent(r.outgoing(2, un))["op"], "feed_unsubscribe");
    }

    #[test]
    fn the_latest_process_watch_wins_and_the_earlier_one_returns() {
        let (mut r, _rx) = router(2);
        assert_eq!(sent(r.outgoing(1, r#"{"op":"procs_watch","space":"s1"}"#))["space"], "s1");
        assert_eq!(sent(r.outgoing(2, r#"{"op":"procs_watch","space":"s2"}"#))["space"], "s2");
        assert_eq!(r.leave(2), vec![json!({"op":"procs_watch","space":"s1"}).to_string()]);
    }

    #[test]
    fn a_render_is_refused_only_when_every_window_refused() {
        let (mut r, _rx) = router(2);
        sent(r.outgoing(1, r#"{"op":"render_offer"}"#));
        sent(r.outgoing(2, r#"{"op":"render_offer"}"#));
        let (m, raw) = msg(json!({"ev":"render_pane","id":4,"pane":"p"}));
        assert_eq!(r.incoming(m, &raw).len(), 2);
        assert_eq!(r.outgoing(1, r#"{"op":"render_refused","id":4,"message":"not shown"}"#), Outgoing::Hold);
        assert_eq!(sent(r.outgoing(2, r#"{"op":"render_refused","id":4,"message":"not shown"}"#))["id"], 4);
        // A window command goes to one window.
        sent(r.outgoing(2, r#"{"op":"window_offer"}"#));
        let (m, raw) = msg(json!({"ev":"window_request","id":1,"command":"ls"}));
        assert_eq!(r.incoming(m, &raw).into_iter().map(|(n, _)| n).collect::<Vec<_>>(), [2]);
    }
}
