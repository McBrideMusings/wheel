//! The usage journal and the apps' log lines reach a Loki push endpoint with their labels, each
//! line once; a store that is down holds them back without losing them; a store that refuses a
//! batch is shown as failing and not asked again in a loop; and the shipper's own lines stay home.

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{extract::State, routing::post, Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{AppLogLine, ClientMsg, LogShipping, ServerMsg};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "logship-test-token";

/// Every push body the fake store accepted.
type Pushes = Arc<Mutex<Vec<Value>>>;

/// A fake Loki push endpoint.
struct Store {
    url: String,
    /// What it accepted.
    pushes: Pushes,
    /// The status it answers every push with; 204 accepts.
    answer: Arc<AtomicU16>,
    /// How many pushes reached it, accepted or not.
    tries: Arc<AtomicU16>,
}

type StoreState = (Pushes, Arc<AtomicU16>, Arc<AtomicU16>);

async fn fake_loki() -> Store {
    let store = Store {
        url: String::new(),
        pushes: Arc::default(),
        answer: Arc::new(AtomicU16::new(204)),
        tries: Arc::new(AtomicU16::new(0)),
    };
    let state: StoreState = (
        store.pushes.clone(),
        store.answer.clone(),
        store.tries.clone(),
    );
    let app = Router::new()
        .route(
            "/loki/api/v1/push",
            post(
                |State((pushes, answer, tries)): State<StoreState>, Json(body): Json<Value>| async move {
                    tries.fetch_add(1, Ordering::SeqCst);
                    let code = answer.load(Ordering::SeqCst);
                    if code == 204 {
                        pushes.lock().unwrap().push(body);
                    }
                    axum::http::StatusCode::from_u16(code).unwrap()
                },
            ),
        )
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    Store {
        url: format!("http://{addr}/loki/api/v1/push"),
        ..store
    }
}

/// Every (labels, line) the store holds, across all pushes.
fn lines(pushes: &Pushes) -> Vec<(Value, String)> {
    let mut out = Vec::new();
    for push in pushes.lock().unwrap().iter() {
        for stream in push["streams"].as_array().unwrap() {
            for value in stream["values"].as_array().unwrap() {
                let line = value[1].as_str().unwrap().to_string();
                out.push((stream["stream"].clone(), line));
            }
        }
    }
    out
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("wheel-logship-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

async fn start_daemon(instance: PathBuf, log: &str) -> (Ws, common::Daemon) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = wheeld::AppState::new(
        log.into(),
        TOKEN.into(),
        instance,
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    let url = format!("ws://{addr}/ws?token={TOKEN}");
    let ws = tokio_tungstenite::connect_async(&url).await.unwrap().0;
    (ws, common::Daemon::new(serving, state))
}

/// Send a request and return its reply, skipping broadcasts.
async fn call(ws: &mut Ws, msg: ClientMsg) -> ServerMsg {
    ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
        .await
        .unwrap();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(60), ws.next())
            .await
            .expect("daemon replied")
            .unwrap()
            .unwrap();
        let Message::Text(text) = frame else { continue };
        match serde_json::from_str(&text).unwrap() {
            ServerMsg::Spaces { .. }
            | ServerMsg::StatusChanged { .. }
            | ServerMsg::Settings { req: None, .. } => {}
            reply => return reply,
        }
    }
}

async fn set_push_url(ws: &mut Ws, url: &str) {
    let reply = call(
        ws,
        ClientMsg::SettingSet {
            req: Some(1),
            key: "logs.push_url".into(),
            value: url.into(),
            space: None,
            machine: true,
        },
    )
    .await;
    assert!(
        matches!(reply, ServerMsg::Settings { .. }),
        "set refused: {reply:?}"
    );
}

async fn new_space(ws: &mut Ws, base: &std::path::Path) {
    let project = base.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let created = call(
        ws,
        ClientMsg::SpaceCreate {
            req: Some(3),
            cwd: project.to_string_lossy().into(),
            name: None,
            background: false,
        },
    )
    .await;
    assert!(matches!(created, ServerMsg::Created { .. }), "{created:?}");
}

async fn app_log(ws: &mut Ws, client: &str, ts_ms: u64, msg: &str) {
    let reply = call(
        ws,
        ClientMsg::Log {
            req: Some(5),
            client: client.into(),
            inode: None,
            lines: vec![AppLogLine {
                ts_ms,
                msg: msg.into(),
                end: None,
            }],
        },
    )
    .await;
    assert!(matches!(reply, ServerMsg::Ok { .. }), "{reply:?}");
}

async fn shipping(ws: &mut Ws) -> LogShipping {
    match call(ws, ClientMsg::Status { req: Some(2) }).await {
        ServerMsg::Status { logs, .. } => *logs,
        other => panic!("expected status, got {other:?}"),
    }
}

async fn ship(daemon: &common::Daemon) {
    let state = daemon.state().clone();
    tokio::task::spawn_blocking(move || state.ship_now())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn actions_and_app_lines_reach_the_store_once_with_their_labels() {
    let store = fake_loki().await;
    let base = dir("labels");
    let (mut ws, daemon) = start_daemon(base.join("instance"), "/dev/null").await;
    set_push_url(&mut ws, &store.url).await;
    new_space(&mut ws, &base).await;
    // A request that fails is journaled as a failure with the daemon's reason.
    let closed = call(
        &mut ws,
        ClientMsg::SpaceClose {
            req: Some(4),
            id: "no-such-space".into(),
            if_empty: false,
        },
    )
    .await;
    assert!(matches!(closed, ServerMsg::Error { .. }), "{closed:?}");
    app_log(
        &mut ws,
        "iphone",
        1_792_692_085_869,
        "phone could not reach wheeld",
    )
    .await;
    // An app name the daemon does not know is a label value, so it is kept as `other`.
    app_log(
        &mut ws,
        "../../made-up-app",
        1_792_692_085_870,
        "from elsewhere",
    )
    .await;

    ship(&daemon).await;
    let got = lines(&store.pushes);
    let journal: Vec<&(Value, String)> = got
        .iter()
        .filter(|(l, _)| l["source"] == "journal")
        .collect();
    let line_of = |kind: &str| -> (Value, Value) {
        let (labels, line) = journal
            .iter()
            .find(|(l, _)| l["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind} line in {got:#?}"));
        ((*labels).clone(), serde_json::from_str(line).unwrap())
    };

    let (labels, line) = line_of("space_create");
    assert_eq!(
        (labels["app"].as_str(), labels["client"].as_str()),
        (Some("wheel"), Some("local"))
    );
    assert!(labels["host"].as_str().is_some_and(|h| !h.is_empty()));
    assert_eq!(line["ok"], true, "{line}");
    assert!(line["ms"].is_u64() && line["ts_ms"].is_u64(), "{line}");

    let (_, line) = line_of("space_close");
    assert_eq!(
        (line["ok"].as_bool(), line["space"].as_str()),
        (Some(false), Some("no-such-space"))
    );
    assert!(
        line["err"].as_str().is_some_and(|e| !e.is_empty()),
        "{line}"
    );

    // Polls and the apps' own lines are not actions.
    assert!(
        !journal
            .iter()
            .any(|(l, _)| l["kind"] == "status" || l["kind"] == "log"),
        "{journal:#?}"
    );

    let app: Vec<(Option<&str>, &str)> = got
        .iter()
        .filter(|(l, _)| l["source"] == "app")
        .map(|(l, line)| (l["client"].as_str(), line.as_str()))
        .collect();
    assert_eq!(
        app,
        vec![
            (Some("iphone"), "phone could not reach wheeld"),
            (Some("other"), "from elsewhere"),
        ]
    );

    // Delivered lines are not sent again.
    let before = got.len();
    ship(&daemon).await;
    assert_eq!(lines(&store.pushes).len(), before);

    let state = shipping(&mut ws).await;
    assert_eq!(
        (state.state.as_str(), state.waiting_bytes),
        ("ok", 0),
        "{state:?}"
    );
    assert!(
        state.last_ok_ms.is_some() && state.error.is_none(),
        "{state:?}"
    );
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_store_that_is_down_holds_the_lines_until_it_answers() {
    // A port nothing listens on.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = format!("http://{}/loki/api/v1/push", closed.local_addr().unwrap());
    drop(closed);

    let base = dir("down");
    let (mut ws, daemon) = start_daemon(base.join("instance"), "/dev/null").await;
    set_push_url(&mut ws, &dead).await;
    new_space(&mut ws, &base).await;
    ship(&daemon).await;
    let state = shipping(&mut ws).await;
    assert_eq!(state.state, "failing", "{state:?}");
    assert!(
        state.error.as_deref().is_some_and(|e| !e.is_empty()),
        "{state:?}"
    );
    assert!(state.waiting_bytes > 0, "{state:?}");

    // The store answers at a new address: everything held back arrives, once.
    let store = fake_loki().await;
    set_push_url(&mut ws, &store.url).await;
    ship(&daemon).await;
    let got = lines(&store.pushes);
    let creates = got
        .iter()
        .filter(|(l, _)| l["kind"] == "space_create")
        .count();
    assert_eq!(creates, 1, "{got:#?}");
    let state = shipping(&mut ws).await;
    assert_eq!(
        (state.state.as_str(), state.waiting_bytes),
        ("ok", 0),
        "{state:?}"
    );
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_the_store_refuses_is_skipped_shown_as_failing_and_not_asked_again_at_once() {
    let refusing = fake_loki().await;
    refusing.answer.store(400, Ordering::SeqCst);
    let base = dir("refused");
    let (mut ws, daemon) = start_daemon(base.join("instance"), "/dev/null").await;
    set_push_url(&mut ws, &refusing.url).await;
    new_space(&mut ws, &base).await;
    let started = std::time::Instant::now();
    ship(&daemon).await;
    ship(&daemon).await;
    // One refused push, then the pass stops and waits out its 5s back-off. A machine so loaded
    // that the back-off ran out before the second pass leaves nothing to assert here.
    let tries = refusing.tries.load(Ordering::SeqCst);
    if started.elapsed() < Duration::from_secs(4) {
        assert_eq!(tries, 1);
    }
    let state = shipping(&mut ws).await;
    assert_eq!(state.state, "failing", "{state:?}");
    assert!(
        state
            .error
            .as_deref()
            .is_some_and(|e| e.contains("HTTP 400")),
        "{state:?}"
    );

    // A store that takes lines: the refused batch stays skipped, later lines arrive.
    let store = fake_loki().await;
    set_push_url(&mut ws, &store.url).await;
    app_log(&mut ws, "mac", 1_792_692_085_871, "after the refusal").await;
    ship(&daemon).await;
    let got = lines(&store.pushes);
    assert!(
        got.iter().any(|(_, line)| line == "after the refusal"),
        "{got:#?}"
    );
    assert!(
        !got.iter().any(|(l, _)| l["kind"] == "space_create"),
        "{got:#?}"
    );
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shippers_own_log_lines_never_ship() {
    let store = fake_loki().await;
    let base = dir("own-lines");
    let log = base.join("wheeld.log");
    std::fs::write(
        &log,
        "2026-10-22T19:46:01.652079Z  INFO wheeld::spaces: space created space=s3\n\
         2026-10-22T19:46:02.000000Z  WARN wheeld::logship: cannot ship log lines error=HTTP 400\n",
    )
    .unwrap();
    let (mut ws, daemon) = start_daemon(base.join("instance"), &log.to_string_lossy()).await;
    set_push_url(&mut ws, &store.url).await;
    ship(&daemon).await;
    let got: Vec<String> = lines(&store.pushes)
        .into_iter()
        .filter(|(l, _)| l["source"] == "wheeld")
        .map(|(_, line)| line)
        .collect();
    assert_eq!(got.len(), 1, "{got:#?}");
    assert!(got[0].contains("space created"), "{got:#?}");
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_push_url_nothing_is_sent_and_status_says_off() {
    let base = dir("off");
    let (mut ws, daemon) = start_daemon(base.join("instance"), "/dev/null").await;
    new_space(&mut ws, &base).await;
    ship(&daemon).await;
    let state = shipping(&mut ws).await;
    assert_eq!(state.state, "off", "{state:?}");
    // The journal is kept on this machine whether or not anything ships it.
    let journal = std::fs::read_to_string(base.join("instance").join("usage.jsonl")).unwrap();
    assert!(journal.contains("\"op\":\"space_create\""), "{journal}");
    daemon.abort();
}

/// An app sends a batch again when the connection dropped after wheeld took it and before the
/// `ok` arrived, and the longer batch it sends next carries the lines written since. The store
/// gets each line once.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_sent_again_after_a_dropped_ok_reaches_the_store_once() {
    let store = fake_loki().await;
    let base = dir("resent");
    let (mut ws, daemon) = start_daemon(base.join("instance"), "/dev/null").await;
    set_push_url(&mut ws, &store.url).await;
    let line = |n: u64| AppLogLine {
        ts_ms: 1_792_692_086_000 + n,
        msg: format!("resend line {n}"),
        end: Some(n * 40),
    };
    let send = |inode: u64, lines: Vec<AppLogLine>| ClientMsg::Log {
        req: Some(6),
        client: "mac".into(),
        inode: Some(inode),
        lines,
    };
    for msg in [
        send(77, vec![line(1), line(2)]),
        // The connection dropped before the ok: the same lines go again, with one more.
        send(77, vec![line(1), line(2), line(3)]),
        // The whole batch again.
        send(77, vec![line(1), line(2), line(3)]),
        // A rolled file is a different inode, read from its start.
        send(78, vec![line(1)]),
    ] {
        let reply = call(&mut ws, msg).await;
        assert!(matches!(reply, ServerMsg::Ok { .. }), "{reply:?}");
    }

    ship(&daemon).await;
    let got: Vec<String> = lines(&store.pushes)
        .into_iter()
        .filter(|(l, _)| l["source"] == "app")
        .map(|(_, line)| line)
        .collect();
    assert_eq!(
        got,
        [
            "resend line 1",
            "resend line 2",
            "resend line 3",
            "resend line 1"
        ],
        "{got:#?}"
    );
}
