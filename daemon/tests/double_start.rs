//! Two starts on one pane at the same moment, as a double click on the empty-pane picker sends
//! them: the first agent keeps the pane and the second start is refused.

const TOKEN: &str = "double-start-test-token";

#[tokio::test(flavor = "multi_thread")]
async fn a_second_start_on_a_pane_still_starting_is_refused_and_the_first_agent_stays() {
    let dir = std::env::temp_dir().join(format!("wheel-double-start-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state_file = dir.join("spaces.json");
    let saved = serde_json::json!({
        "spaces": [{
            "id": "s1", "name": "tmp", "cwd": dir, "focus": "p2",
            "root": {"node": "pane", "id": "p2", "kind": "empty"},
        }],
        "next_id": 10,
    });
    std::fs::write(&state_file, saved.to_string()).unwrap();
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        dir.clone(),
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    let claude_home = dir.join("agent-home");
    std::fs::create_dir_all(&claude_home).unwrap();
    state
        .setting_set(
            "agent.claude.home",
            claude_home.to_str().unwrap(),
            None,
            true,
        )
        .unwrap();
    state
        .setting_set(
            "agent.claude.command",
            env!("CARGO_BIN_EXE_wheel-fake-agent"),
            None,
            true,
        )
        .unwrap();
    state.restore_spaces(state_file).await.unwrap();

    let start = || {
        state.pane_start(
            "s1",
            Some("p2".into()),
            wheel_proto::StartKind::Agent,
            Default::default(),
        )
    };
    let (a, b) = tokio::join!(start(), start());
    let (started, refused) = match (a, b) {
        (Ok(started), Err(refused)) | (Err(refused), Ok(started)) => (started, refused),
        (a, b) => panic!("one start should win and one be refused: {a:?} {b:?}"),
    };
    assert!(
        refused.contains("pane p2 is already starting an agent"),
        "{refused}"
    );
    // The pane holds the agent the winning start made, which never ended.
    let root = serde_json::to_value(&state.listing().spaces[0].root).unwrap();
    assert_eq!(root["agent"], started.agent.unwrap().as_str(), "{root}");
    // The refused start left no claim behind: once the first start is done, a start on the pane runs.
    start().await.unwrap();
    state.stop_agents();
    let _ = std::fs::remove_dir_all(&dir);
}
