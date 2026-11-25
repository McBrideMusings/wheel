//! The servers this `wheel` is paired with (ADR-0057): the list in `~/.wheel/servers.toml`, each
//! server's credential in the macOS Keychain (a `0600` file beside the list elsewhere). The local
//! wheeld is built in and never listed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The label that always names the local wheeld.
pub const LOCAL: &str = "local";

/// One paired server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Server {
    /// What `--server` and the other commands call it; unique in the list.
    pub label: String,
    /// The `host.id` the server reported when it paired; a server reporting another is refused.
    pub id: String,
    /// The remote listener, `wss://<address>:<port>/ws`.
    pub url: String,
    /// SHA-256 of the certificate it presented when it paired, lowercase hex.
    pub fingerprint: String,
    /// The device id the server gave this `wheel`, which `wheel remote revoke` takes on it.
    #[serde(default)]
    pub device: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Default, Serialize, Deserialize)]
struct ListFile {
    #[serde(default, rename = "server")]
    servers: Vec<Server>,
}

/// Where the list lives: `WHEEL_SERVERS_FILE`, else `~/.wheel/servers.toml`.
pub fn path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("WHEEL_SERVERS_FILE").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(Path::new(&home).join(".wheel").join("servers.toml"))
}

/// The list; empty when the file does not exist.
pub fn load(path: &Path) -> Result<Vec<Server>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str::<ListFile>(&text)
            .map(|f| f.servers)
            .map_err(|e| format!("read {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// Write the list whole, through a temporary file and a rename, so a reader never sees half.
pub fn save(path: &Path, servers: &[Server]) -> Result<(), String> {
    let text = toml::to_string(&ListFile {
        servers: servers.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("write {}: {e}", path.display()))
}

/// The server `selector` names: a label first, else an id, so a renamed label never hides one.
pub fn find<'a>(servers: &'a [Server], selector: &str) -> Option<&'a Server> {
    servers
        .iter()
        .find(|s| s.label == selector)
        .or_else(|| servers.iter().find(|s| s.id == selector))
}

/// Refuse a label that is empty, names the local wheeld, or would name another server: taken by
/// its label or equal to its id. `own` is the id of the server taking the label.
pub fn check_label(servers: &[Server], label: &str, own: &str) -> Result<(), String> {
    if label.is_empty() || label.chars().any(char::is_whitespace) {
        return Err(format!("a server label has no spaces and is not empty: {label:?}"));
    }
    if label == LOCAL {
        return Err(format!("`{LOCAL}` always names this machine's wheeld"));
    }
    match servers
        .iter()
        .find(|s| s.id != own && (s.label == label || s.id == label))
    {
        Some(other) => Err(format!(
            "`{label}` already names server {} ({}); pick another with --label",
            other.label, other.id
        )),
        None => Ok(()),
    }
}

/// Refuse a server id that is not a UUID's letters: the id names a Keychain item and, off macOS,
/// a file, so one a server chose to escape that folder must never be used.
pub fn check_id(id: &str) -> Result<(), String> {
    match !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        true => Ok(()),
        false => Err(format!("the server reported an id that is not a UUID: {id:?}")),
    }
}

/// A label made from a server's name: lowercase, every run of other characters one `-`.
pub fn label_of(name: &str) -> String {
    let mut label = String::new();
    for c in name.chars() {
        match c.is_ascii_alphanumeric() {
            true => label.push(c.to_ascii_lowercase()),
            false if !label.is_empty() && !label.ends_with('-') => label.push('-'),
            false => {}
        }
    }
    label.trim_end_matches('-').to_string()
}

/// The Keychain service each credential is kept under, its account the server's id.
#[cfg(target_os = "macos")]
const SERVICE: &str = "Wheel Server";

/// Keep `credential` for server `id`.
pub fn credential_set(list: &Path, id: &str, credential: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = list;
        security_framework::passwords::set_generic_password(SERVICE, id, credential.as_bytes())
            .map_err(|e| format!("keep the credential in the Keychain: {e}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::io::Write as _;
        use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
        check_id(id)?;
        let dir = credentials_dir(list);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("create {}: {e}", dir.display()))?;
        let path = dir.join(id);
        let _ = std::fs::remove_file(&path);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut f| f.write_all(credential.as_bytes()))
            .map_err(|e| format!("write {}: {e}", path.display()))
    }
}

/// The credential kept for server `id`.
pub fn credential_get(list: &Path, id: &str) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = list;
        security_framework::passwords::get_generic_password(SERVICE, id)
            .map_err(|e| format!("read the credential for {id} from the Keychain: {e}"))
            .and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        check_id(id)?;
        let path = credentials_dir(list).join(id);
        std::fs::read_to_string(&path)
            .map(|c| c.trim().to_string())
            .map_err(|e| format!("read {}: {e}", path.display()))
    }
}

/// Forget the credential for server `id`; one already gone is no error.
pub fn credential_delete(list: &Path, id: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = list;
        match security_framework::passwords::delete_generic_password(SERVICE, id) {
            Ok(()) => Ok(()),
            // errSecItemNotFound
            Err(e) if e.code() == -25300 => Ok(()),
            Err(e) => Err(format!("remove the credential for {id} from the Keychain: {e}")),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        check_id(id)?;
        let path = credentials_dir(list).join(id);
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("remove {}: {e}", path.display()))
            }
            _ => Ok(()),
        }
    }
}

/// Where credentials live off macOS: `credentials/` beside the list.
#[cfg(not(target_os = "macos"))]
fn credentials_dir(list: &Path) -> PathBuf {
    list.parent()
        .unwrap_or(Path::new("."))
        .join("credentials")
}

/// The value `--server` was given on the command line, read before clap parses so a remote
/// server's command never takes a default from this machine's `WHEEL_PANE` or `WHEEL_SPACE`.
pub fn selector_in(args: &[String]) -> Option<String> {
    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--" {
            return None;
        }
        if arg == "--server" {
            return args.next().cloned();
        }
        if let Some(value) = arg.strip_prefix("--server=") {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(label: &str, id: &str) -> Server {
        Server {
            label: label.into(),
            id: id.into(),
            url: "wss://100.64.0.1:7421/ws".into(),
            fingerprint: "ab".repeat(32),
            device: "d1".into(),
            enabled: true,
        }
    }

    #[test]
    fn a_label_wins_over_an_id_and_an_id_still_finds_a_renamed_server() {
        let list = vec![server("dev", "id-1"), server("id-1", "id-2")];
        assert_eq!(find(&list, "dev").unwrap().id, "id-1");
        assert_eq!(find(&list, "id-1").unwrap().id, "id-2");
        assert_eq!(find(&list, "id-2").unwrap().label, "id-1");
        assert!(find(&list, "nope").is_none());
    }

    #[test]
    fn labels_are_unique_and_never_local() {
        let list = vec![server("dev", "id-1")];
        assert!(check_label(&list, "dev", "id-2").is_err());
        assert!(check_label(&list, "id-1", "id-2").is_err());
        assert!(check_label(&list, "dev", "id-1").is_ok());
        assert!(check_label(&list, LOCAL, "id-2").is_err());
        assert!(check_label(&list, "", "id-2").is_err());
        assert!(check_label(&list, "a b", "id-2").is_err());
        assert!(check_label(&list, "box", "id-2").is_ok());
    }

    #[test]
    fn an_id_that_could_name_another_file_is_refused() {
        assert!(check_id("3c1d0c44-beb9-48ea-a461-7a519aa064cd").is_ok());
        assert!(check_id("../../.profile").is_err());
        assert!(check_id("/etc/passwd").is_err());
        assert!(check_id("").is_err());
    }

    #[test]
    fn a_name_becomes_a_label() {
        assert_eq!(label_of("Studio Mac"), "studio-mac");
        assert_eq!(label_of("dev"), "dev");
        assert_eq!(label_of("  Pierce's  MacBook "), "pierce-s-macbook");
    }

    #[test]
    fn the_list_round_trips_and_a_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!("wheel-servers-{}", std::process::id()));
        let path = dir.join("servers.toml");
        assert_eq!(load(&path).unwrap(), vec![]);
        let list = vec![server("dev", "id-1"), Server { enabled: false, ..server("box", "id-2") }];
        save(&path, &list).unwrap();
        assert_eq!(load(&path).unwrap(), list);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_selector_is_read_before_parsing() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(selector_in(&args(&["wheel", "--server", "dev", "space", "ls"])), Some("dev".into()));
        assert_eq!(selector_in(&args(&["wheel", "space", "ls", "--server=dev"])), Some("dev".into()));
        assert_eq!(selector_in(&args(&["wheel", "space", "ls"])), None);
        assert_eq!(selector_in(&args(&["wheel", "pane", "run", "--", "--server", "x"])), None);
    }
}
