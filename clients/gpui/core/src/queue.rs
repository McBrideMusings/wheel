//! The orchestrator's queue (ADR-0054) as Orchestrator mode's tray shows it: the badge's count and
//! tone and the groups the popover lists, read from the `wheelhouse.queue` feed. The grouping is the
//! feed's (`plugins/wheelhouse/src/beads/queue.rs`), never the client's. The view is `view/queue.rs`.
use serde_json::Value;

/// The feed the tray reads, and the plugin pane wheeld grants it to (its manifest's `reads`).
pub const FEED: &str = "wheelhouse.queue";
pub const PANE_LABEL: &str = "wheelhouse/queue";

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub kind: String,
    /// The space and the agent (else the pane) it came from, joined by ` · `.
    pub from: String,
    pub created: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: String,
    pub title: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Queue {
    pub count: usize,
    /// `red`, `orange` or `gray`.
    pub tone: String,
    pub groups: Vec<Group>,
    /// wheeld's clock in Unix seconds when the value was read; ages count from it.
    pub now: i64,
    pub error: Option<String>,
    pub has_queue: bool,
}

impl Queue {
    /// The feed's value as the tray reads it; None for a value that is not an object (null until
    /// the backend sends one).
    pub fn of(value: &Value) -> Option<Queue> {
        let o = value.as_object()?;
        let text = |v: &Value| v.as_str().map(str::to_owned);
        let groups = o.get("groups").and_then(Value::as_array).into_iter().flatten().filter_map(|g| {
            let items = g["items"].as_array().into_iter().flatten().filter_map(|i| {
                let from: Vec<String> = [text(&i["space"]), text(&i["agent"]).or_else(|| text(&i["pane"]))].into_iter().flatten().collect();
                Some(Item {
                    id: text(&i["id"])?,
                    title: text(&i["title"]).unwrap_or_default(),
                    kind: text(&i["kind"]).unwrap_or_else(|| "item".into()),
                    from: from.join(" · "),
                    created: text(&i["created"]).unwrap_or_default(),
                })
            });
            Some(Group { id: text(&g["id"])?, title: text(&g["title"]).unwrap_or_default(), items: items.collect() })
        });
        Some(Queue {
            count: o.get("badge").and_then(|b| b["count"].as_u64()).unwrap_or(0) as usize,
            tone: o.get("badge").and_then(|b| text(&b["tone"])).unwrap_or_else(|| "gray".into()),
            groups: groups.collect(),
            now: o.get("now").and_then(Value::as_i64).unwrap_or(0),
            error: o.get("error").and_then(text),
            has_queue: o.get("has_queue").and_then(Value::as_bool).unwrap_or(true),
        })
    }

    /// How long before `now` an RFC 3339 time was, as `5m`, `3h` or `2d`, rounded as the Queue pane's
    /// `time.ago` and the phone's sheet round; empty when it does not parse.
    pub fn age(&self, created: &str) -> String {
        let Some(then) = crate::strip::parse_rfc3339(created).filter(|_| self.now > 0) else { return String::new() };
        let minutes = (self.now - then).max(0) as f64 / 60.;
        if minutes < 60. {
            format!("{}m", minutes.round() as i64)
        } else if minutes < 1440. {
            format!("{}h", (minutes / 60.).round() as i64)
        } else {
            format!("{}d", (minutes / 1440.).round() as i64)
        }
    }
}

/// What pressing an item sends to the orchestrator's chat, as if typed there (the phone's
/// `OrchestratorQueue.message`).
pub fn message(item: &Item) -> String {
    format!("Let's talk about {}: {}", item.id, item.title)
}

/// The tray's state: the subscription it holds, the value it last read, and whether the popover is open.
#[derive(Default)]
pub struct Tray {
    /// The Queue pane and folder the subscription was sent for.
    pub subscribed: Option<(String, String)>,
    pub value: Option<Value>,
    pub open: bool,
}

impl Tray {
    /// Take a `feed_update`: the whole value, or an RFC 6902 patch against the one held. A patch
    /// that does not apply drops the value, so the tray never shows a half-patched queue.
    pub fn update(&mut self, value: Option<&Value>, patch: Option<&Value>) -> Result<(), String> {
        if let Some(v) = value {
            self.value = Some(v.clone());
            return Ok(());
        }
        let (Some(patch), Some(held)) = (patch, self.value.as_mut()) else { return Ok(()) };
        let applied = serde_json::from_value::<json_patch::Patch>(patch.clone()).map_err(|e| e.to_string()).and_then(|p| json_patch::patch(held, &p).map_err(|e| e.to_string()));
        if applied.is_err() {
            self.value = None;
        }
        applied
    }
    pub fn queue(&self) -> Option<Queue> {
        self.value.as_ref().and_then(Queue::of)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn value() -> Value {
        json!({"now": 1_800_000_000, "has_queue": true, "error": null, "badge": {"count": 1, "tone": "orange"},
            "groups": [{"id": "needs", "title": "Needs you", "items": [{"id": "q-1", "title": "Allow rm?", "kind": "permission", "space": "wheel", "agent": "w12", "pane": "p3", "created": "2027-01-15T07:50:00Z"}]},
                {"id": "review", "title": "Ready for review", "items": []}]})
    }

    #[test]
    fn reads_the_feeds_badge_groups_and_ages() {
        let q = Queue::of(&value()).unwrap();
        assert_eq!((q.count, q.tone.as_str()), (1, "orange"));
        assert_eq!(q.groups.iter().map(|g| g.title.as_str()).collect::<Vec<_>>(), ["Needs you", "Ready for review"]);
        let item = &q.groups[0].items[0];
        assert_eq!((item.kind.as_str(), item.from.as_str()), ("permission", "wheel · w12"));
        // 1_800_000_000 is 2027-01-15T08:00:00Z: ten minutes after the item.
        assert_eq!(q.age(&item.created), "10m");
        assert_eq!(q.age("not a time"), "");
        assert_eq!(message(item), "Let's talk about q-1: Allow rm?");
        assert!(Queue::of(&Value::Null).is_none());
    }

    #[test]
    fn a_patch_changes_the_held_value_and_a_bad_one_drops_it() {
        let mut tray = Tray::default();
        tray.update(None, Some(&json!([{"op": "replace", "path": "/badge/count", "value": 2}]))).unwrap();
        assert!(tray.value.is_none(), "a patch with nothing held changes nothing");
        tray.update(Some(&value()), None).unwrap();
        tray.update(None, Some(&json!([{"op": "replace", "path": "/badge/count", "value": 2}]))).unwrap();
        assert_eq!(tray.queue().unwrap().count, 2);
        assert!(tray.update(None, Some(&json!([{"op": "remove", "path": "/nope"}]))).is_err());
        assert!(tray.value.is_none());
    }
}
