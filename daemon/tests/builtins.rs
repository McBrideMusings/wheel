//! Built-in artifacts over the socket: install writes the folder, the machine entry and
//! `installed.json`; an artifact Wheel did not install is never installed over; an update reports,
//! keeps or replaces a file a person edited; remove takes all three away again.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{BuiltinInfo, ClientMsg, OnEdited, ServerMsg};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "builtins-test-token";
/// The one built-in this repo ships for testing the mechanism: `artifacts/sample`.
const NAME: &str = "sample";

async fn start(instance: &Path) -> (Ws, common::Daemon) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        instance.to_path_buf(),
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    let url = format!("ws://{addr}/ws?token={TOKEN}");
    let ws = tokio_tungstenite::connect_async(url).await.unwrap().0;
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
            ServerMsg::ArtifactGone { .. } | ServerMsg::Settings { req: None, .. } => {}
            reply => return reply,
        }
    }
}

async fn builtins(ws: &mut Ws, msg: ClientMsg) -> Vec<BuiltinInfo> {
    match call(ws, msg).await {
        ServerMsg::Builtins { builtins, .. } => builtins,
        other => panic!("expected builtins, got {other:?}"),
    }
}

async fn refused(ws: &mut Ws, msg: ClientMsg) -> String {
    match call(ws, msg).await {
        ServerMsg::Error { message, .. } => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn instance(case: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wheel-builtins-{case}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The sample's row of a listing.
fn sample(list: &[BuiltinInfo]) -> &BuiltinInfo {
    list.iter()
        .find(|b| b.name == NAME)
        .expect("sample is shipped")
}

fn install() -> ClientMsg {
    ClientMsg::BuiltinInstall {
        req: Some(1),
        name: NAME.into(),
    }
}

fn update(on_edited: OnEdited) -> ClientMsg {
    ClientMsg::BuiltinUpdate {
        req: Some(2),
        name: NAME.into(),
        on_edited,
    }
}

fn installed(inst: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(inst.join("installed.json")).unwrap()).unwrap()
}

fn machine_entry(inst: &Path) -> Option<toml::Value> {
    let text = std::fs::read_to_string(inst.join("wheel.toml")).ok()?;
    let table: toml::Table = text.parse().unwrap();
    table.get("artifacts")?.get(NAME).cloned()
}

#[tokio::test]
async fn install_writes_the_folder_the_entry_and_the_record() {
    let inst = instance("install");
    let (mut ws, daemon) = start(&inst).await;
    let listed = builtins(&mut ws, ClientMsg::BuiltinLs { req: Some(1) }).await;
    assert_eq!(sample(&listed).installed_version, None);

    let after = builtins(&mut ws, install()).await;
    let sample = sample(&after);
    assert_eq!(sample.installed_version, Some(sample.shipped_version));
    assert!(sample.edited_files.is_empty());

    let page = inst.join("artifacts/sample/index.html");
    let bytes = std::fs::read(&page).unwrap();
    assert!(
        !inst.join("artifacts/sample/builtin.toml").exists(),
        "the description is not a file of the artifact"
    );
    let record = installed(&inst);
    assert_eq!(record[NAME]["version"], sample.shipped_version);
    assert_eq!(record[NAME]["files"]["index.html"], sha256(&bytes));
    let entry = machine_entry(&inst).expect("a machine entry");
    assert_eq!(entry["path"].as_str(), Some("artifacts/sample"));
    assert!(
        entry.get("command").is_none(),
        "a built-in installs no refresh command"
    );
    daemon.abort();
}

#[tokio::test]
async fn an_artifact_wheel_did_not_install_is_never_installed_over() {
    // A machine entry of that name.
    let inst = instance("entry");
    std::fs::write(
        inst.join("wheel.toml"),
        "[artifacts.sample]\npath = \"/somewhere/else\"\n",
    )
    .unwrap();
    let (mut ws, daemon) = start(&inst).await;
    let why = refused(&mut ws, install()).await;
    assert!(why.contains("did not install"), "{why}");
    assert!(!inst.join("artifacts/sample").exists());
    assert!(!inst.join("installed.json").exists());
    daemon.abort();

    // A folder of that name.
    let inst = instance("folder");
    std::fs::create_dir_all(inst.join("artifacts/sample")).unwrap();
    std::fs::write(inst.join("artifacts/sample/index.html"), "mine").unwrap();
    let (mut ws, daemon) = start(&inst).await;
    let why = refused(&mut ws, install()).await;
    assert!(why.contains("did not install"), "{why}");
    assert_eq!(
        std::fs::read_to_string(inst.join("artifacts/sample/index.html")).unwrap(),
        "mine"
    );
    assert!(machine_entry(&inst).is_none());
    daemon.abort();

    // Installed already: update is for that.
    let inst = instance("twice");
    let (mut ws, daemon) = start(&inst).await;
    builtins(&mut ws, install()).await;
    let why = refused(&mut ws, install()).await;
    assert!(why.contains("already installed"), "{why}");
    daemon.abort();
}

#[tokio::test]
async fn an_update_asks_keeps_or_replaces_an_edited_file() {
    let inst = instance("update");
    let (mut ws, daemon) = start(&inst).await;
    builtins(&mut ws, install()).await;
    let page = inst.join("artifacts/sample/index.html");
    let shipped = std::fs::read_to_string(&page).unwrap();
    std::fs::write(&page, "<p>my edit</p>\n").unwrap();
    // Wheel's record says an older version, as one installed by an earlier wheeld would.
    let mut record = installed(&inst);
    record[NAME]["version"] = 0.into();
    std::fs::write(inst.join("installed.json"), record.to_string()).unwrap();

    let listed = builtins(&mut ws, ClientMsg::BuiltinLs { req: Some(1) }).await;
    assert_eq!(sample(&listed).edited_files, ["index.html"]);

    // No choice: the edited file is reported with its diff, and nothing changes.
    match call(&mut ws, update(OnEdited::Ask)).await {
        ServerMsg::BuiltinEdited { name, edited, .. } => {
            assert_eq!(name, NAME);
            assert_eq!(edited.len(), 1);
            assert_eq!(edited[0].path, "index.html");
            assert!(
                edited[0].diff.contains("-<p>my edit</p>"),
                "{}",
                edited[0].diff
            );
        }
        other => panic!("expected builtin_edited, got {other:?}"),
    }
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "<p>my edit</p>\n");
    assert_eq!(installed(&inst)[NAME]["version"], 0);

    // Keep: the edit stays, the version moves, and the file still counts as edited.
    let kept = builtins(&mut ws, update(OnEdited::Keep)).await;
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "<p>my edit</p>\n");
    assert_eq!(
        sample(&kept).installed_version,
        Some(sample(&kept).shipped_version)
    );
    assert_eq!(sample(&kept).edited_files, ["index.html"]);

    // Replace: the shipped page is back and nothing counts as edited.
    let replaced = builtins(&mut ws, update(OnEdited::Replace)).await;
    assert_eq!(std::fs::read_to_string(&page).unwrap(), shipped);
    assert!(sample(&replaced).edited_files.is_empty());
    daemon.abort();
}

/// A file Wheel never wrote, where the version it carries has one, is a person's: an update reports
/// it rather than overwriting it. And an installed version newer than the one carried is never
/// written over with the older one.
#[tokio::test]
async fn an_update_never_overwrites_a_file_wheel_did_not_write_or_a_newer_version() {
    let inst = instance("theirs");
    let (mut ws, daemon) = start(&inst).await;
    builtins(&mut ws, install()).await;
    let page = inst.join("artifacts/sample/index.html");
    // As if the installed version had no index.html and the person made one.
    let mut record = installed(&inst);
    record[NAME]["files"]
        .as_object_mut()
        .unwrap()
        .remove("index.html");
    std::fs::write(inst.join("installed.json"), record.to_string()).unwrap();
    std::fs::write(&page, "<p>theirs</p>\n").unwrap();

    let listed = builtins(&mut ws, ClientMsg::BuiltinLs { req: Some(1) }).await;
    assert_eq!(sample(&listed).edited_files, ["index.html"]);
    match call(&mut ws, update(OnEdited::Ask)).await {
        ServerMsg::BuiltinEdited { edited, .. } => assert_eq!(edited[0].path, "index.html"),
        other => panic!("expected builtin_edited, got {other:?}"),
    }
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "<p>theirs</p>\n");

    let mut record = installed(&inst);
    record[NAME]["version"] = 99.into();
    std::fs::write(inst.join("installed.json"), record.to_string()).unwrap();
    let why = refused(&mut ws, update(OnEdited::Replace)).await;
    assert!(why.contains("newer than"), "{why}");
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "<p>theirs</p>\n");
    daemon.abort();
}

#[tokio::test]
async fn remove_takes_away_the_folder_the_entry_and_the_record() {
    let inst = instance("remove");
    std::fs::write(
        inst.join("wheel.toml"),
        "[browser]\nhomepage = \"about:blank\"\n",
    )
    .unwrap();
    let (mut ws, daemon) = start(&inst).await;
    builtins(&mut ws, install()).await;
    let after = builtins(
        &mut ws,
        ClientMsg::BuiltinRemove {
            req: Some(3),
            name: NAME.into(),
        },
    )
    .await;
    assert_eq!(sample(&after).installed_version, None);
    assert!(!inst.join("artifacts/sample").exists());
    assert!(machine_entry(&inst).is_none());
    assert!(installed(&inst).get(NAME).is_none());
    let left = std::fs::read_to_string(inst.join("wheel.toml")).unwrap();
    assert!(
        left.contains("homepage"),
        "the rest of the machine file stays: {left}"
    );
    daemon.abort();
}
