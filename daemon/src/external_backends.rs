//! `[[external_backend]]` entries (ADR-0041): a backend the person runs themselves, which wheeld
//! connects to and never starts, stops or restarts. One entry is `id` (the plugin whose feeds and
//! actions it serves, `<id>.<name>`), `connect` (`unix:///abs/path.sock` or `ws://host:port/path`)
//! and `token` (sent over `ws://` as `Authorization: Bearer <token>`; a `ws://` entry needs one).
//! Entries come from this machine's `wheel.toml` and the space's own, and a project entry replaces
//! the machine's with the same `id`. Every read goes to the files, so an edit is in force at the
//! next read.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use wheel_proto::PluginScope;

use crate::settings::FILE;

/// Where wheeld connects. Its `Debug` never prints the token.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Connect {
    /// A unix socket the backend listens on.
    Unix(PathBuf),
    /// A WebSocket URL; each text message is one JSON-RPC message.
    Ws { url: String, token: String },
}

impl std::fmt::Debug for Connect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Connect({})", self.address())
    }
}

impl Connect {
    /// The address as the entry wrote it, without the token.
    pub(crate) fn address(&self) -> String {
        match self {
            Connect::Unix(path) => format!("unix://{}", path.display()),
            Connect::Ws { url, .. } => url.clone(),
        }
    }
}

/// One `[[external_backend]]` entry. `connect` is why it cannot be used when it is refused: the
/// reason is what a read of one of its feeds is told.
#[derive(Debug, Clone)]
pub(crate) struct External {
    pub id: String,
    pub connect: Result<Connect, String>,
    /// The `wheel.toml` that holds it.
    pub file: PathBuf,
    pub scope: PluginScope,
}

impl External {
    /// What its feeds are filed under in the hub: one connection per address and token, whichever
    /// spaces read it, so an entry with another token never shares a connection another token
    /// opened. The token goes in as a digest, never as itself.
    pub(crate) fn key(&self) -> String {
        use sha2::{Digest, Sha256};
        match &self.connect {
            Ok(Connect::Ws { url, token }) => {
                let digest = Sha256::digest(token.as_bytes());
                let short: String = digest[..6].iter().map(|b| format!("{b:02x}")).collect();
                format!("external:{}@{url}#{short}", self.id)
            }
            Ok(c) => format!("external:{}@{}", self.id, c.address()),
            Err(_) => format!("external:{}", self.id),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    id: Option<String>,
    connect: Option<String>,
    token: Option<String>,
}

/// `connect` and `token` as wheeld uses them, or why they are refused.
fn parse_connect(connect: Option<&str>, token: Option<&str>) -> Result<Connect, String> {
    let connect = connect.map(str::trim).filter(|c| !c.is_empty()).ok_or("it has no connect")?;
    let token = token.map(str::trim).filter(|t| !t.is_empty());
    if let Some(path) = connect.strip_prefix("unix://") {
        if !path.starts_with('/') {
            return Err(format!("connect {connect:?} names no absolute path (unix:///path/to.sock)"));
        }
        if token.is_some() {
            return Err("a token is sent only over ws://; a unix socket is guarded by its file permissions".into());
        }
        return Ok(Connect::Unix(PathBuf::from(path)));
    }
    if connect.starts_with("ws://") {
        let Some(token) = token else {
            return Err(format!("connect {connect:?} is a ws:// URL and the entry has no token"));
        };
        return Ok(Connect::Ws {
            url: connect.to_string(),
            token: token.to_string(),
        });
    }
    Err(format!("connect {connect:?} is neither unix:///path nor ws://host:port/path"))
}

/// Whether `id` can name a plugin's feeds (`<id>.<name>`) and its log (`<id>.log`): one plain
/// name, without a dot.
fn plain_id(id: &str) -> bool {
    crate::plugins::plain_name(id) && !id.contains('.')
}

/// The entries of one `wheel.toml`, in file order. A missing file or one that is not TOML has
/// none; an entry with no `id`, or a second with the same `id`, is skipped and logged.
pub(crate) fn read(path: &Path, scope: PluginScope) -> Vec<External> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let table: toml::Table = match toml::from_str(&text) {
        Ok(table) => table,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "a wheel.toml is not valid TOML; it names no external backends");
            return Vec::new();
        }
    };
    let Some(list) = table.get("external_backend").and_then(toml::Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<External> = Vec::new();
    for value in list {
        let (id, connect) = match value.clone().try_into::<Raw>() {
            Ok(raw) => match raw.id.map(|s| s.trim().to_string()).filter(|s| plain_id(s)) {
                Some(id) => (id, parse_connect(raw.connect.as_deref(), raw.token.as_deref())),
                None => {
                    tracing::warn!(path = %path.display(), "an [[external_backend]] entry has no id that is one plain name without a dot; it is skipped");
                    continue;
                }
            },
            Err(e) => {
                // The id is still read, so a read of its feeds is told what is wrong.
                let Some(id) = value.get("id").and_then(toml::Value::as_str).map(str::trim).filter(|s| plain_id(s))
                else {
                    tracing::warn!(path = %path.display(), error = %e, "an [[external_backend]] entry cannot be read; it is skipped");
                    continue;
                };
                (id.to_string(), Err(e.message().to_string()))
            }
        };
        if out.iter().any(|e| e.id == id) {
            tracing::warn!(path = %path.display(), %id, "two [[external_backend]] entries in one file share an id; the second is skipped");
            continue;
        }
        out.push(External {
            id,
            connect,
            file: path.to_path_buf(),
            scope,
        });
    }
    out
}

/// This machine's entries, then the project's at `cwd`: a project entry replaces the machine's
/// with the same `id`.
pub(crate) fn resolve(machine_file: &Path, cwd: Option<&str>) -> Vec<External> {
    let mut out = read(machine_file, PluginScope::Machine);
    let Some(cwd) = cwd else { return out };
    for entry in read(&Path::new(cwd).join(FILE), PluginScope::Project) {
        match out.iter().position(|e| e.id == entry.id) {
            Some(at) => out[at] = entry,
            None => out.push(entry),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, text: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(FILE);
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn entries_parse_and_a_project_entry_replaces_the_machines() {
        let dir = std::env::temp_dir().join(format!("ext-be-{}", std::process::id()));
        let machine = write(
            &dir,
            "[[external_backend]]\nid = 'clock'\nconnect = 'unix:///tmp/m.sock'\n\
             [[external_backend]]\nid = 'feed'\nconnect = 'ws://127.0.0.1:9/x'\ntoken = 't'\n",
        );
        let project = dir.join("p");
        write(&project, "[[external_backend]]\nid = 'clock'\nconnect = 'unix:///tmp/p.sock'\n");
        let all = resolve(&machine, Some(project.to_str().unwrap()));
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].connect, Ok(Connect::Unix("/tmp/p.sock".into())));
        assert_eq!(all[0].scope, PluginScope::Project);
        assert_eq!(
            all[1].connect,
            Ok(Connect::Ws { url: "ws://127.0.0.1:9/x".into(), token: "t".into() })
        );
        assert_eq!(all[0].key(), "external:clock@unix:///tmp/p.sock");
        let other = External {
            connect: Ok(Connect::Ws { url: "ws://127.0.0.1:9/x".into(), token: "u".into() }),
            ..all[1].clone()
        };
        assert_ne!(all[1].key(), other.key(), "another token is another connection");
        let secret = External {
            connect: Ok(Connect::Ws { url: "ws://h:1/".into(), token: "s3cret".into() }),
            ..all[1].clone()
        };
        assert!(!secret.key().contains("s3cret"), "the key never holds the token");
        assert!(!format!("{secret:?}").contains("s3cret"), "Debug never prints the token");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_ws_entry_without_a_token_and_other_bad_entries_are_refused_with_why() {
        let refused = |c: Option<&str>, t: Option<&str>| parse_connect(c, t).unwrap_err();
        assert!(refused(Some("ws://h:1/"), None).contains("has no token"));
        assert!(refused(Some("ws://h:1/"), Some("  ")).contains("has no token"));
        assert!(refused(Some("unix://rel.sock"), None).contains("absolute path"));
        assert!(refused(Some("unix:///a.sock"), Some("t")).contains("only over ws://"));
        assert!(refused(Some("http://h/"), None).contains("neither"));
        assert!(refused(Some("wss://h/"), Some("t")).contains("neither"));
        assert!(refused(None, None).contains("no connect"));
    }

    #[test]
    fn an_entry_with_an_unknown_key_keeps_its_id_and_says_why() {
        let dir = std::env::temp_dir().join(format!("ext-be-bad-{}", std::process::id()));
        let file = write(
            &dir,
            "[[external_backend]]\nid = 'x'\nconect = 'unix:///a'\n[[external_backend]]\nconnect = 'unix:///b'\n\
             [[external_backend]]\nid = '../../up'\nconnect = 'unix:///c'\n[[external_backend]]\nid = 'a.b'\nconnect = 'unix:///d'\n",
        );
        let all = read(&file, PluginScope::Machine);
        assert_eq!(all.len(), 1);
        assert!(all[0].connect.as_ref().unwrap_err().contains("conect"), "{:?}", all[0].connect);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
