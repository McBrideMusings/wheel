//! The `queue` feed for `wheelhouse backend`: a folder's open beads labelled `needs-you` (the
//! orchestrator's queue, ADR-0054, written by its `tools/on-event`), grouped as the queue sheet
//! shows them, with the badge a header draws. The grouping lives here once, so every client and
//! the `queue` pane draw the same groups.

use serde_json::{json, Value};

use super::*;

pub const LABEL: &str = "needs-you";

/// The groups in the order they show: id, title, and the item kinds (`metadata.kind`) each holds.
/// A kind no group names is in the first, so nothing waiting is hidden.
const GROUPS: [(&str, &str, &[&str]); 3] = [
    ("needs", "Needs you", &["permission", "question", "bug"]),
    ("review", "Ready for review", &["review"]),
    ("finished", "Finished", &["done", "unconfirmed"]),
];

fn group_of(kind: &str) -> usize {
    GROUPS.iter().position(|(_, _, kinds)| kinds.contains(&kind)).unwrap_or(0)
}

/// One item as the sheet shows it, from `bd list --json`'s row.
fn item(b: &Value) -> Value {
    let meta = &b["metadata"];
    let text = |v: &Value| v.as_str().map(String::from);
    json!({
        "id": b["id"], "title": b["title"], "priority": b["priority"].as_i64().unwrap_or(2),
        "kind": text(&meta["kind"]).unwrap_or_else(|| "item".into()),
        "agent": text(&meta["agent"]), "pane": text(&meta["pane"]), "space": text(&meta["space"]),
        "topic": text(&meta["topic"]), "created": b["created_at"],
    })
}

/// The groups and the badge for `rows`, the open queue beads: most urgent first in each group,
/// then oldest. The badge counts the first group; its tone is `red` with a P0 there, `orange`
/// with a P1, else `gray`.
fn shape(rows: &[Value]) -> Value {
    let mut groups: Vec<Vec<Value>> = vec![Vec::new(); GROUPS.len()];
    for b in rows {
        let it = item(b);
        groups[group_of(it["kind"].as_str().unwrap_or(""))].push(it);
    }
    for g in &mut groups {
        g.sort_by(|a, b| {
            let key = |v: &Value| (v["priority"].as_i64().unwrap_or(2), v["created"].as_str().unwrap_or("").to_string());
            key(a).cmp(&key(b))
        });
    }
    let top = groups[0].iter().filter_map(|i| i["priority"].as_i64()).min();
    let tone = match top {
        Some(0) => "red",
        Some(1) => "orange",
        _ => "gray",
    };
    let count = groups[0].len();
    json!({
        "badge": { "count": count, "tone": tone },
        "groups": GROUPS.iter().zip(groups).map(|((id, title, _), items)| json!({ "id": id, "title": title, "items": items })).collect::<Vec<_>>(),
    })
}

/// The `queue` feed's value for the folder `dir`: `{now, has_queue, error, badge, groups}`.
pub fn value(dir: &str) -> Value {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let has_queue = PathBuf::from(dir).join(".beads").is_dir();
    let (rows, error) = if has_queue {
        match crate::core::run::bd(dir, &["list", "--label", LABEL, "--status", "open"]) {
            Ok(rows) => (rows, None),
            Err(e) => (vec![], Some(e)),
        }
    } else {
        (vec![], None)
    };
    let mut v = shape(&rows);
    v["now"] = json!(now);
    v["has_queue"] = json!(has_queue);
    v["error"] = json!(error);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, kind: &str, priority: i64, created: &str) -> Value {
        json!({ "id": id, "title": format!("t {id}"), "priority": priority, "created_at": created,
                "metadata": { "kind": kind, "pane": "p1", "space": "wheel" }, "labels": [LABEL] })
    }

    #[test]
    fn groups_and_badge() {
        let v = shape(&[
            row("a", "question", 2, "2026-11-23T08:00:00Z"),
            row("b", "bug", 0, "2026-11-23T08:05:00Z"),
            row("c", "done", 3, "2026-11-23T08:01:00Z"),
            row("d", "review", 2, "2026-11-23T08:02:00Z"),
            row("e", "unconfirmed", 3, "2026-11-23T08:03:00Z"),
        ]);
        assert_eq!(v["badge"], json!({ "count": 2, "tone": "red" }));
        let ids = |g: usize| v["groups"][g]["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(ids(0), ["b", "a"]);
        assert_eq!(ids(1), ["d"]);
        assert_eq!(ids(2), ["c", "e"]);
        assert_eq!(v["groups"][0]["title"], "Needs you");
        assert_eq!(v["groups"][0]["items"][0]["space"], "wheel");
    }

    #[test]
    fn tone_follows_the_most_urgent_waiting() {
        assert_eq!(shape(&[row("a", "permission", 1, "")])["badge"]["tone"], "orange");
        assert_eq!(shape(&[row("a", "question", 2, ""), row("b", "done", 0, "")])["badge"], json!({ "count": 1, "tone": "gray" }));
        assert_eq!(shape(&[])["badge"], json!({ "count": 0, "tone": "gray" }));
        // A kind no group names waits on the person.
        assert_eq!(shape(&[row("a", "mystery", 2, "")])["groups"][0]["items"][0]["kind"], "mystery");
    }
}
