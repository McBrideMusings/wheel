//! The post store: one JSON file per post in a folder, written atomically (temp file, then
//! rename). The validation of `post` and `answer` lives here, so the tests reach it without a
//! backend connection.

use std::collections::hash_map::RandomState;
use std::collections::HashSet;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

/// The newest posts kept per space.
pub const KEEP: usize = 500;
/// The most posts the feed holds.
const FEED_MAX: usize = 200;
const TITLE_MAX: usize = 200;
const REPLY_MAX: usize = 4000;

const POST_KEYS: [&str; 6] = ["title", "body", "question", "options", "id", "page"];
const OPTION_KEYS: [&str; 6] = ["id", "action", "case", "cost", "checked", "pick"];
const OPTION_TEXTS: [&str; 4] = ["action", "case", "cost", "checked"];

/// Every read-modify-write of the store runs under this lock.
static LOCK: Mutex<()> = Mutex::new(());

pub struct Store {
    dir: PathBuf,
    keep: usize,
}

impl Store {
    /// `$TURN_FEED_DIR`, else `$HOME/.wheel/turn-feed/posts`.
    pub fn from_env() -> Store {
        let dir = match std::env::var_os("TURN_FEED_DIR") {
            Some(d) if !d.is_empty() => PathBuf::from(d),
            _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".wheel/turn-feed/posts"),
        };
        Store { dir, keep: KEEP }
    }

    #[cfg(test)]
    fn at(dir: PathBuf, keep: usize) -> Store {
        Store { dir, keep }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn read(&self, id: &str) -> Option<Value> {
        serde_json::from_slice(&fs::read(self.file(id)).ok()?).ok()
    }

    fn write(&self, post: &Value) -> Result<(), String> {
        let id = post["post"].as_str().ok_or("post has no id")?;
        self.ensure().map_err(|e| format!("cannot create {}: {e}", self.dir.display()))?;
        let tmp = self.dir.join(format!(".{id}.tmp"));
        let bytes = serde_json::to_vec_pretty(post).map_err(|e| e.to_string())?;
        let mut f = fs::File::create(&tmp).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        f.write_all(&bytes).and_then(|_| f.sync_all()).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        fs::rename(&tmp, self.file(id)).map_err(|e| format!("cannot write {}: {e}", self.file(id).display()))
    }

    /// Every stored post, newest first.
    fn all(&self) -> Vec<Value> {
        let mut posts: Vec<Value> = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                n.ends_with(".json") && !n.starts_with('.')
            })
            .filter_map(|e| serde_json::from_slice::<Value>(&fs::read(e.path()).ok()?).ok())
            .filter(|p| p["post"].is_string() && p["at"].is_string())
            .collect();
        posts.sort_by(|a, b| {
            let key = |p: &Value| (p["at"].as_str().unwrap_or("").to_string(), p["post"].as_str().unwrap_or("").to_string());
            key(b).cmp(&key(a))
        });
        posts
    }

    fn prune(&self, space: &str) {
        let mine: Vec<Value> = self.all().into_iter().filter(|p| p["space"] == space).collect();
        for old in mine.iter().skip(self.keep) {
            if let Some(id) = old["post"].as_str() {
                let _ = fs::remove_file(self.file(id));
            }
        }
    }

    /// The `posts` feed's value for a space: only the posts its own agent panes made, so two spaces
    /// on one directory never share a feed.
    pub fn feed(&self, space: &str) -> Value {
        let posts: Vec<Value> = self.all().into_iter().filter(|p| p["space"] == space).take(FEED_MAX).collect();
        json!({ "space": space, "posts": posts })
    }

    pub fn post(&self, args: &Value, caller: &Value) -> Result<Value, String> {
        let pane = caller["pane"].as_str().unwrap_or("");
        if caller["agent"].as_bool() != Some(true) {
            return Err(format!("turn-feed.post is for agent panes; pane {pane} is not one"));
        }
        let cwd = caller["cwd"].as_str().filter(|c| !c.is_empty());
        let space = caller["space"].as_str().filter(|s| !s.is_empty());
        let (Some(cwd), Some(space), false) = (cwd, space, pane.is_empty()) else {
            return Err("post refused: the call names no pane, space or directory".into());
        };
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let valid = validate(args)?;
        let now = now_rfc3339();
        let mut post = Map::new();
        let id = match &valid.replace {
            Some(old_id) => {
                let old = self.read(old_id).ok_or_else(|| format!("post refused: post {old_id} does not exist"))?;
                if old["agent_pane"] != pane {
                    return Err(format!("post refused: post {old_id} was not posted by pane {pane}"));
                }
                post.insert("at".into(), old["at"].clone());
                post.insert("updated_at".into(), json!(now));
                old_id.clone()
            }
            None => {
                post.insert("at".into(), json!(now));
                new_id()
            }
        };
        post.insert("post".into(), json!(id));
        post.insert("agent_pane".into(), json!(pane));
        post.insert("space".into(), json!(space));
        post.insert("dir".into(), json!(cwd));
        for (k, v) in valid.fields {
            post.insert(k, v);
        }
        post.insert("answer".into(), Value::Null);
        self.write(&Value::Object(post))?;
        self.prune(space);
        Ok(json!({ "post": id }))
    }

    pub fn answer(&self, args: &Value, caller: &Value) -> Result<Value, String> {
        let Some(obj) = args.as_object() else {
            return Err("answer refused: args must be an object".into());
        };
        let mut problems = unknown_keys(obj, &["post", "pick", "reply"], "");
        let id = obj.get("post").and_then(Value::as_str).unwrap_or("");
        if id.is_empty() {
            problems.push("no post".into());
        } else if !id_ok(id) {
            problems.push(format!("post {id:?} is not a post id"));
        }
        let pick = obj.get("pick");
        let reply = obj.get("reply");
        match (pick, reply) {
            (Some(_), Some(_)) => problems.push("give pick or reply, not both".into()),
            (None, None) => problems.push("give pick or reply".into()),
            _ => {}
        }
        if let Some(p) = pick {
            if p.as_str().map_or(true, str::is_empty) {
                problems.push("pick must be an option id".into());
            }
        }
        if let Some(r) = reply {
            match r.as_str() {
                Some(s) if s.trim().is_empty() => problems.push("reply is empty".into()),
                Some(s) if s.chars().count() > REPLY_MAX => problems.push(format!("reply is over {REPLY_MAX} characters")),
                Some(_) => {}
                None => problems.push("reply must be a string".into()),
            }
        }
        if !problems.is_empty() {
            return Err(format!("answer refused: {}", problems.join("; ")));
        }
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut post = self.read(id).ok_or_else(|| format!("answer refused: post {id} does not exist"))?;
        if !post["answer"].is_null() {
            let a = &post["answer"];
            let what = a["pick"].as_str().map(str::to_string).unwrap_or_else(|| "a reply".into());
            return Err(format!("post {id} was already answered: {what} at {}", a["at"].as_str().unwrap_or("?")));
        }
        let title = post["title"].as_str().unwrap_or("").to_string();
        let now = now_rfc3339();
        let by = caller["pane"].clone();
        let (answer, text) = if let Some(pick) = pick.and_then(Value::as_str) {
            let known = post["options"].as_array().is_some_and(|os| os.iter().any(|o| o["id"] == pick));
            if !known {
                let ids: Vec<&str> = post["options"].as_array().into_iter().flatten().filter_map(|o| o["id"].as_str()).collect();
                return Err(format!("answer refused: post {id} has no option {pick}; its options: {}", if ids.is_empty() { "none".into() } else { ids.join(", ") }));
            }
            let text = format!("Turn feed, \"{title}\": picked {pick}");
            (json!({ "pick": pick, "at": now, "by_pane": by, "text": text, "delivered": false }), text)
        } else {
            let reply = reply.and_then(Value::as_str).unwrap_or("");
            let text = format!("Turn feed, \"{title}\": {reply}");
            (json!({ "reply": reply, "at": now, "by_pane": by, "text": text, "delivered": false }), text)
        };
        post["answer"] = answer;
        self.write(&post)?;
        Ok(json!({ "post": id, "agent_pane": post["agent_pane"], "text": text }))
    }

    /// Marks a post's answer as received by the agent; the pane calls it after `wheel.agent.send` succeeds.
    pub fn delivered(&self, args: &Value) -> Result<Value, String> {
        let Some(obj) = args.as_object() else {
            return Err("delivered refused: args must be an object".into());
        };
        let problems = unknown_keys(obj, &["post"], "");
        let id = obj.get("post").and_then(Value::as_str).unwrap_or("");
        if !problems.is_empty() {
            return Err(format!("delivered refused: {}", problems.join("; ")));
        }
        if !id_ok(id) {
            return Err(format!("delivered refused: post {id:?} is not a post id"));
        }
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut post = self.read(id).ok_or_else(|| format!("delivered refused: post {id} does not exist"))?;
        if post["answer"].is_null() {
            return Err(format!("delivered refused: post {id} has no answer"));
        }
        post["answer"]["delivered"] = json!(true);
        self.write(&post)?;
        Ok(json!({ "post": id }))
    }
}

struct Valid {
    fields: Vec<(String, Value)>,
    replace: Option<String>,
}

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn unknown_keys(obj: &Map<String, Value>, known: &[&str], who: &str) -> Vec<String> {
    let mut keys: Vec<&String> = obj.keys().filter(|k| !known.contains(&k.as_str())).collect();
    keys.sort();
    keys.into_iter().map(|k| if who.is_empty() { format!("unknown key {k:?}") } else { format!("{who} has unknown key {k:?}") }).collect()
}

/// A required, non-empty string; the problem when it is not one.
fn text<'a>(obj: &'a Map<String, Value>, key: &str, who: &str) -> Result<&'a str, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Err(format!("{who} has no {key}")),
        Some(Value::String(s)) if s.trim().is_empty() => Err(format!("{who} has an empty {key}")),
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(format!("{who}'s {key} must be a string")),
    }
}

fn validate(args: &Value) -> Result<Valid, String> {
    let Some(obj) = args.as_object() else {
        return Err("post refused: args must be an object".into());
    };
    let mut p: Vec<String> = unknown_keys(obj, &POST_KEYS, "");
    let mut fields: Vec<(String, Value)> = Vec::new();

    match text(obj, "title", "post") {
        Ok(t) if t.contains('\n') || t.contains('\r') => p.push("title must be one line".into()),
        Ok(t) if t.chars().count() > TITLE_MAX => p.push(format!("title is over {TITLE_MAX} characters")),
        Ok(t) => fields.push(("title".into(), json!(t))),
        Err(e) => p.push(e),
    }
    match text(obj, "body", "post") {
        Ok(b) => fields.push(("body".into(), json!(b))),
        Err(e) => p.push(e),
    }

    let options = obj.get("options").filter(|v| !v.is_null());
    match options {
        None => {
            if let Some(q) = obj.get("question").filter(|v| !v.is_null()) {
                match q.as_str() {
                    Some(s) if !s.trim().is_empty() => fields.push(("question".into(), json!(s))),
                    _ => p.push("question must be a non-empty string".into()),
                }
            }
        }
        Some(options) => {
            match text(obj, "question", "post") {
                Ok(q) => fields.push(("question".into(), json!(q))),
                Err(_) => p.push("post has options but no question".into()),
            }
            match options.as_array() {
                None => p.push("options must be a list".into()),
                Some(list) if list.is_empty() => p.push("options is empty; leave it out for a post with none".into()),
                Some(list) => {
                    let mut seen: HashSet<&str> = HashSet::new();
                    let mut picks = 0;
                    for (i, o) in list.iter().enumerate() {
                        let Some(o) = o.as_object() else {
                            p.push(format!("option #{} must be an object", i + 1));
                            continue;
                        };
                        let who = match o.get("id").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                            Some(id) => {
                                if !seen.insert(id) {
                                    p.push(format!("option id {id} is used twice"));
                                }
                                format!("option {id}")
                            }
                            None => {
                                p.push(format!("option #{} has no id", i + 1));
                                format!("option #{}", i + 1)
                            }
                        };
                        p.extend(unknown_keys(o, &OPTION_KEYS, &who));
                        for k in OPTION_TEXTS {
                            if let Err(e) = text(o, k, &who) {
                                p.push(e);
                            }
                        }
                        match o.get("pick") {
                            None | Some(Value::Null) | Some(Value::Bool(false)) => {}
                            Some(Value::Bool(true)) => picks += 1,
                            Some(_) => p.push(format!("{who}'s pick must be true or false")),
                        }
                    }
                    if picks != 1 {
                        p.push(format!("exactly one option must have pick: true (found {picks})"));
                    }
                    fields.push(("options".into(), options.clone()));
                }
            }
        }
    }

    let mut replace = None;
    if let Some(id) = obj.get("id").filter(|v| !v.is_null()) {
        match id.as_str() {
            Some(s) if id_ok(s) => replace = Some(s.to_string()),
            _ => p.push("id must be the id of an earlier post".into()),
        }
    }
    if let Some(page) = obj.get("page").filter(|v| !v.is_null()) {
        match page.as_str() {
            Some(s) if !s.trim().is_empty() => fields.push(("page".into(), json!(s))),
            _ => p.push("page must be an artifact name".into()),
        }
    }
    if p.is_empty() {
        Ok(Valid { fields, replace })
    } else {
        Err(format!("post refused: {}", p.join("; ")))
    }
}

fn new_id() -> String {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let mut h = RandomState::new().build_hasher();
    h.write_u128(millis);
    format!("t{millis}{:04x}", h.finish() & 0xffff)
}

/// `2026-11-16T14:03:09.123Z`.
fn now_rfc3339() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    rfc3339(d.as_secs() as i64, d.subsec_millis())
}

fn rfc3339(secs: i64, millis: u32) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(keep: usize) -> Store {
        let dir = std::env::temp_dir().join(format!("turn-feed-test-{}-{}", std::process::id(), new_id()));
        Store::at(dir, keep)
    }

    fn agent(pane: &str, space: &str) -> Value {
        json!({ "pane": pane, "space": space, "cwd": "/p", "agent": true })
    }

    fn option(id: &str, pick: bool) -> Value {
        json!({ "id": id, "action": "do it", "case": "because", "cost": "time", "checked": "nothing claimed", "pick": pick })
    }

    fn simple() -> Value {
        json!({ "title": "Done", "body": "I did it.\n\nSecond paragraph." })
    }

    fn with_options() -> Value {
        json!({ "title": "Which?", "body": "b", "question": "1. Which one?", "options": [option("1A", true), option("1B", false)] })
    }

    #[test]
    fn missing_fields_are_all_named_at_once() {
        let s = store(KEEP);
        let mut args = with_options();
        args["options"][1] = json!({ "id": "1B", "action": "x", "case": "y" });
        let e = s.post(&args, &agent("p1", "s1")).unwrap_err();
        assert_eq!(e, "post refused: option 1B has no cost; option 1B has no checked");
        let e = s.post(&json!({ "title": "", "options": [], "tpyo": 1 }), &agent("p1", "s1")).unwrap_err();
        assert!(e.contains("unknown key \"tpyo\""), "{e}");
        assert!(e.contains("post has an empty title"), "{e}");
        assert!(e.contains("post has no body"), "{e}");
        assert!(e.contains("post has options but no question"), "{e}");
        let mut args = with_options();
        args["question"] = Value::Null;
        args["options"][0]["extra"] = json!(1);
        args["options"][1]["pick"] = json!(true);
        args["options"][1]["id"] = json!("1A");
        let e = s.post(&args, &agent("p1", "s1")).unwrap_err();
        assert!(e.contains("post has options but no question"), "{e}");
        assert!(e.contains("option 1A has unknown key \"extra\""), "{e}");
        assert!(e.contains("option id 1A is used twice"), "{e}");
        assert!(e.contains("exactly one option must have pick: true (found 2)"), "{e}");
        assert!(s.all().is_empty());
        let e = s.post(&json!({ "title": "a\nb", "body": "x" }), &agent("p1", "s1")).unwrap_err();
        assert!(e.contains("title must be one line"), "{e}");
    }

    #[test]
    fn only_agent_panes_post() {
        let s = store(KEEP);
        let caller = json!({ "pane": "p4", "space": "s1", "plugin": "turn-feed", "plugin_pane": "feed" });
        let e = s.post(&simple(), &caller).unwrap_err();
        assert_eq!(e, "turn-feed.post is for agent panes; pane p4 is not one");
    }

    #[test]
    fn a_post_is_stored_and_replaced_only_by_its_own_pane() {
        let s = store(KEEP);
        let id = s.post(&with_options(), &agent("p1", "s1")).unwrap()["post"].as_str().unwrap().to_string();
        let first = s.read(&id).unwrap();
        println!("{}", serde_json::to_string_pretty(&first).unwrap());
        assert_eq!(first["agent_pane"], "p1");
        assert_eq!(first["dir"], "/p");
        assert_eq!(first["answer"], Value::Null);
        assert!(first.get("updated_at").is_none());
        let mut next = simple();
        next["id"] = json!(id);
        let e = s.post(&next, &agent("p2", "s1")).unwrap_err();
        assert_eq!(e, format!("post refused: post {id} was not posted by pane p2"));
        next["id"] = json!("tnope");
        assert!(s.post(&next, &agent("p1", "s1")).unwrap_err().contains("does not exist"));
        // An answered post replaced by its agent has its answer reset and its time kept.
        s.answer(&json!({ "post": id, "pick": "1A" }), &json!({ "pane": "p9" })).unwrap();
        next["id"] = json!(id);
        assert_eq!(s.post(&next, &agent("p1", "s1")).unwrap()["post"], id.as_str());
        let after = s.read(&id).unwrap();
        assert_eq!(after["at"], first["at"]);
        assert_eq!(after["title"], "Done");
        assert_eq!(after["answer"], Value::Null);
        assert!(after["updated_at"].is_string());
        assert!(after.get("options").is_none());
        assert_eq!(s.all().len(), 1);
    }

    #[test]
    fn a_post_is_answered_once() {
        let s = store(KEEP);
        let id = s.post(&with_options(), &agent("p1", "s1")).unwrap()["post"].as_str().unwrap().to_string();
        let by = json!({ "pane": "p7", "space": "s1" });
        let e = s.answer(&json!({ "post": id, "pick": "9Z" }), &by).unwrap_err();
        assert!(e.contains("has no option 9Z") && e.contains("1A, 1B"), "{e}");
        assert!(s.answer(&json!({ "post": id }), &by).unwrap_err().contains("give pick or reply"));
        assert!(s.answer(&json!({ "post": id, "pick": "1A", "reply": "x" }), &by).unwrap_err().contains("not both"));
        assert!(s.answer(&json!({ "post": id, "reply": "  " }), &by).unwrap_err().contains("reply is empty"));
        assert!(s.answer(&json!({ "post": "tnone", "reply": "x" }), &by).unwrap_err().contains("does not exist"));
        let r = s.answer(&json!({ "post": id, "pick": "1A" }), &by).unwrap();
        assert_eq!(r["agent_pane"], "p1");
        assert_eq!(r["text"], "Turn feed, \"Which?\": picked 1A");
        let stored = s.read(&id).unwrap();
        assert_eq!(stored["answer"]["pick"], "1A");
        assert_eq!(stored["answer"]["by_pane"], "p7");
        assert_eq!(stored["answer"]["text"], "Turn feed, \"Which?\": picked 1A");
        assert_eq!(stored["answer"]["delivered"], false);
        let e = s.answer(&json!({ "post": id, "reply": "later" }), &by).unwrap_err();
        assert!(e.starts_with(&format!("post {id} was already answered: 1A at ")), "{e}");
    }

    #[test]
    fn delivered_marks_an_answer_received_and_refuses_an_unanswered_post() {
        let s = store(KEEP);
        let id = s.post(&simple(), &agent("p1", "s1")).unwrap()["post"].as_str().unwrap().to_string();
        let e = s.delivered(&json!({ "post": id })).unwrap_err();
        assert!(e.contains("has no answer"), "{e}");
        assert!(s.delivered(&json!({ "post": "tnone" })).unwrap_err().contains("does not exist"));
        let r = s.answer(&json!({ "post": id, "reply": "ship it" }), &json!({ "pane": "p7" })).unwrap();
        let stored = s.read(&id).unwrap();
        assert_eq!(stored["answer"]["text"], r["text"]);
        assert_eq!(stored["answer"]["delivered"], false);
        s.delivered(&json!({ "post": id })).unwrap();
        let stored = s.read(&id).unwrap();
        assert_eq!(stored["answer"]["delivered"], true);
        assert_eq!(stored["answer"]["reply"], "ship it");
    }

    #[test]
    fn a_reply_text_names_the_post_title() {
        let s = store(KEEP);
        let id = s.post(&simple(), &agent("p1", "s1")).unwrap()["post"].as_str().unwrap().to_string();
        let r = s.answer(&json!({ "post": id, "reply": "ship it" }), &json!({ "pane": "p7" })).unwrap();
        assert_eq!(r["text"], "Turn feed, \"Done\": ship it");
        let long = "x".repeat(REPLY_MAX + 1);
        let id2 = s.post(&simple(), &agent("p1", "s1")).unwrap()["post"].as_str().unwrap().to_string();
        assert!(s.answer(&json!({ "post": id2, "reply": long }), &json!({})).unwrap_err().contains("over 4000"));
        // An id that climbs out of the folder is refused before any file is read.
        assert!(s.answer(&json!({ "post": "../x", "reply": "a" }), &json!({})).unwrap_err().contains("is not a post id"));
    }

    #[test]
    fn the_newest_posts_of_a_space_are_kept() {
        let s = store(3);
        for i in 0..5 {
            let mut a = simple();
            a["title"] = json!(format!("t{i}"));
            s.post(&a, &agent("p1", "s1")).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        s.post(&simple(), &agent("p2", "s2")).unwrap();
        let feed = s.feed("s1");
        let titles: Vec<&str> = feed["posts"].as_array().unwrap().iter().map(|p| p["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["t4", "t3", "t2"]);
        assert_eq!(s.all().len(), 4, "the other space's post stays");
    }

    #[test]
    fn the_feed_holds_one_spaces_posts_newest_first_even_on_a_shared_directory() {
        let s = store(KEEP);
        for (i, space) in ["sa", "sb", "sa"].iter().enumerate() {
            let mut a = simple();
            a["title"] = json!(format!("n{i}"));
            s.post(&a, &agent("p1", space)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        let feed = s.feed("sa");
        assert_eq!(feed["space"], "sa");
        let titles: Vec<&str> = feed["posts"].as_array().unwrap().iter().map(|p| p["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["n2", "n0"]);
        assert_eq!(s.feed("snone")["posts"], json!([]));
    }

    #[test]
    fn a_post_needs_a_space() {
        let s = store(KEEP);
        let caller = json!({ "pane": "p1", "cwd": "/p", "agent": true });
        assert_eq!(s.post(&simple(), &caller).unwrap_err(), "post refused: the call names no pane, space or directory");
    }

    #[test]
    fn times_are_rfc_3339_utc() {
        assert_eq!(rfc3339(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(1_784_210_589, 7), "2026-07-16T14:03:09.007Z");
        assert_eq!(rfc3339(1_709_164_800, 0), "2024-02-29T00:00:00.000Z");
    }
}
