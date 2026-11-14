//! The Beads pane's data and buttons for `wheelhouse backend`: one project's beads as the
//! `beads` feed's value, and the two actions a pane presses, `press` and `run_list`. The
//! backend sends data only; the pane (Luau) decides what to draw.

use serde_json::{json, Value};

use super::buttons::{self, State};
use super::*;
use crate::core::run;

fn verb(v: &buttons::Verb) -> Value {
    json!({ "label": v.label, "run": v.run })
}

fn group(g: &Option<buttons::Group>) -> Value {
    match g {
        Some(g) => json!({ "start": verb(&g.start), "resume": verb(&g.resume), "focus": verb(&g.focus) }),
        None => Value::Null,
    }
}

/// The `beads` feed's value for the project in `dir`.
pub fn value(dir: &str) -> Value {
    let snap = collect(dir);
    let records: serde_json::Map<String, Value> = if snap.has_repo {
        buttons::recorded_all(&buttons::records_file(), &snap.path)
            .into_iter()
            .map(|(id, r)| (id, json!({ "pane": r.pane, "conversation": r.conversation })))
            .collect()
    } else {
        Default::default()
    };
    let pressing: Vec<&str> = snap.beads.iter().filter(|b| buttons::pressed(&snap.path, &b.id)).map(|b| b.id.as_str()).collect();
    let beads: Vec<Value> = snap
        .beads
        .iter()
        .map(|b| {
            json!({
                "id": b.id, "title": b.title, "status": b.status, "kind": b.kind, "priority": b.priority,
                "parent": b.parent, "labels": b.labels, "owner": b.owner, "updated": b.updated,
                "blocked_by": b.blocked_by, "description": b.description, "ready": b.ready, "epic": b.epic,
            })
        })
        .collect();
    json!({
        "name": snap.name,
        "path": snap.path,
        "has_repo": snap.has_repo,
        "error": snap.error,
        "problems": snap.problems,
        "buttons": { "task": group(&snap.buttons.task), "epic": group(&snap.buttons.epic), "closed": snap.buttons.closed },
        "scripts": snap.scripts.iter().map(|s| json!({ "name": s.name, "label": s.label() })).collect::<Vec<_>>(),
        "records": records,
        "pressing": pressing,
        "beads": beads,
    })
}

/// The pane a button's command runs for; `default` when the caller named none.
fn press_pane(pane: &str) -> &str {
    if pane.is_empty() {
        "default"
    } else {
        pane
    }
}

/// Press `bead`'s button in the project `dir` for the pane `pane`: focus its pane, resume it or
/// start it, as its state says. `changed` runs whenever a press claims or releases a bead.
pub fn press(dir: &str, bead: &str, pane: &str, changed: impl Fn() + Send + Sync + 'static) -> Result<String, String> {
    let pane = press_pane(pane);
    let mut snap = collect(dir);
    fill_acts(&mut snap);
    let Some(b) = snap.beads.iter().find(|b| b.id == bead) else {
        return Err(format!("no bead {bead}"));
    };
    let Some(act) = snap.acts.get(bead).cloned().filter(|a| !a.disabled) else {
        return Err(format!("{bead} has no button to press"));
    };
    if let State::Live(p) = &act.state {
        run::text("wheel", &snap.path, &["pane", "focus", p])?;
        return Ok(format!("Focused {p} for {bead}."));
    }
    let epic = groups(b);
    let Some(cmd) = buttons::command(&snap.buttons, epic, &act.state) else {
        return Err(format!("{bead} has no command for {}", act.label));
    };
    let log = std::env::temp_dir().join(format!("wheel-beads-{}.log", sane(&b.id)));
    let conversation = buttons::recorded(&buttons::records_file(), &snap.path, bead).map(|r| r.conversation).unwrap_or_default();
    let mut env = vec![("BEAD_ID", b.id.as_str()), ("BEAD_TITLE", &b.title), ("BEAD_TYPE", &b.kind), ("BEAD_REPO", &snap.name), ("BEAD_PATH", &snap.path)];
    if act.state == State::Saved {
        env.push(("CONVERSATION", conversation.as_str()));
    }
    if !buttons::claim(&snap.path, bead) {
        return Err(format!("{bead} is already starting"));
    }
    changed();
    let changed = std::sync::Arc::new(changed);
    let (path, id, after) = (snap.path.clone(), bead.to_string(), changed.clone());
    let remember = move |started: Option<String>| {
        if let Some(pane) = started.filter(|p| !p.is_empty()) {
            let conversation = buttons::conversation_of(&path, &pane).unwrap_or_else(|e| {
                eprintln!("beads: {id}: {e}");
                String::new()
            });
            if let Err(e) = buttons::record(&buttons::records_file(), &path, &id, buttons::Record { pane, conversation }) {
                eprintln!("beads: could not record {id}: {e}");
            }
        }
        buttons::release(&path, &id);
        after();
    };
    match run::button_then(&act.label, cmd, &snap.path, pane, &env, &log, remember) {
        Ok(()) => Ok(format!("{} started for {bead}. See the new pane.", act.label)),
        Err(e) => {
            buttons::release(&snap.path, bead);
            changed();
            Err(e)
        }
    }
}

/// Run the project's list button `script` over `ids` for the pane `pane`.
pub fn run_list(dir: &str, script: &str, ids: &[String], pane: &str) -> Result<String, String> {
    let (scripts, _) = crate::core::buttons::scripts(dir, "beads");
    let name = crate::core::project::name_of(dir);
    let env = [("BEAD_REPO", name.as_str()), ("BEAD_PATH", dir)];
    crate::core::buttons::run_list(&scripts, script, ids, "BEAD_IDS", &env, dir, press_pane(pane), "beads")
}
