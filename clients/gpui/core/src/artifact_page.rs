//! Where an artifact's page opens and what it may reach (ADR-0048): the `wheel-page` origin it is
//! opened under, the daemon address each of its requests is fetched from, and the browser storage
//! it keeps. The rules are WheelKit's `WebPanes`, so the phone and the Mac open a page alike.
use sha2::{Digest, Sha256};

/// The scheme a web pane's page is opened under. A page's storage is keyed by its origin, so the
/// origin cannot hold the daemon's port: the client answers this scheme itself.
pub const SCHEME: &str = "wheel-page";

/// Where web pane `pane` opens: the artifact's folder under a fixed origin.
pub fn page_url(pane: &str) -> String {
    format!("{SCHEME}://pages/artifacts/{pane}/")
}

/// The browser storage an artifact's page keeps, as a UUID string: one per artifact, the same on
/// every launch. A project artifact belongs to its space's directory; a machine artifact is one in
/// every space. The first 16 bytes of a SHA-256 of those parts, marked as a version 8 UUID — the
/// id `WebPanes.store` gives, so no page loses its storage.
pub fn store_id(cwd: &str, machine: bool, name: &str) -> String {
    let text = if machine { format!("machine\n{name}") } else { format!("project\n{cwd}\n{name}") };
    let mut b: [u8; 16] = Sha256::digest(text.as_bytes())[..16].try_into().unwrap();
    b[6] = (b[6] & 0x0F) | 0x80;
    b[8] = (b[8] & 0x3F) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// The daemon's HTTP address for a request a page in `pane` made to `page` (its path and query,
/// as sent), with `daemon` the socket address (`ws://host:port/...`; `wss` becomes `https`). None
/// for anything outside `/artifacts/<pane>/`.
pub fn daemon_url(page: &str, daemon: &str, pane: &str) -> Option<String> {
    let rest = page.strip_prefix(SCHEME)?.strip_prefix("://")?;
    let path = &rest[rest.find('/')?..];
    let path = path.split('#').next().unwrap_or(path);
    if !path.starts_with(&format!("/artifacts/{pane}/")) {
        return None;
    }
    let (scheme, host) = daemon.split_once("://")?;
    let scheme = if scheme == "wss" || scheme == "https" { "https" } else { "http" };
    let host = host.split(['/', '?', '#']).next()?;
    Some(format!("{scheme}://{host}{path}"))
}

/// Whether `url` (a navigation target) stays inside `pane`'s folder under the page's origin.
pub fn in_folder(url: &str, pane: &str) -> bool {
    url.strip_prefix(&format!("{SCHEME}://pages")).is_some_and(|p| p.starts_with(&format!("/artifacts/{pane}/")))
}

/// Whether a frame inside the page may load `target`: its own inline document (`about:srcdoc`,
/// `about:blank`). The pane's main frame never navigates to `about:`.
pub fn allows_inline_frame(target: &str, main_frame: bool) -> bool {
    !main_frame && matches!(target, "about:srcdoc" | "about:blank")
}

/// The daemon address and token a pane's socket address (`ws://host:port/ws?token=…`) names.
pub fn daemon_of(ws: &str) -> Option<(String, String)> {
    let (base, query) = ws.split_once('?')?;
    let token = query.split('&').find_map(|kv| kv.strip_prefix("token="))?;
    Some((base.to_owned(), token.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_ids_match_webpanes_store() {
        // Computed by WheelKit's `WebPanes.store` in Swift: a change here would open every artifact's storage empty.
        for (cwd, machine, name, id) in [
            ("/work/a", false, "site", "40CDFA28-991D-8A34-BF20-267AE15CA56D"),
            ("/work/b", false, "site", "354C784F-6FF8-8AE5-978E-EA4FF71D0518"),
            ("/work/a", false, "docs", "4DB02601-87DC-868C-8752-D4659F1D5321"),
            ("/work/a", true, "site", "70557BBE-9ED7-83FB-9D7E-77A81F84257C"),
            ("/work/b", true, "site", "70557BBE-9ED7-83FB-9D7E-77A81F84257C"),
            ("/Users/pierce/Projects/wheel", false, "gpui-no-swift", "595743AE-7417-88FD-9A42-DAE201CD18FD"),
            ("", true, "turn-feed", "BBCCCDC1-14B5-8D22-BC43-350AA821D4F7"),
            ("", false, "x", "04D2FA0D-1980-872C-8490-C17B9030A043"),
        ] {
            assert_eq!(store_id(cwd, machine, name), id, "{cwd} {machine} {name}");
        }
    }

    #[test]
    fn pages_reach_their_own_folder_only() {
        assert_eq!(page_url("p2"), "wheel-page://pages/artifacts/p2/");
        assert_eq!(daemon_url("wheel-page://pages/artifacts/p2/", "ws://127.0.0.1:7499/ws", "p2").as_deref(), Some("http://127.0.0.1:7499/artifacts/p2/"));
        assert_eq!(
            daemon_url("wheel-page://pages/artifacts/p2/data/rows.json?a=b%2Bc", "wss://mac.local:7420/ws", "p2").as_deref(),
            Some("https://mac.local:7420/artifacts/p2/data/rows.json?a=b%2Bc")
        );
        for other in ["http://evil.example/", "wheel-page://pages/artifacts/p3/", "wheel-page://pages/artifacts/p2", "wheel-page://pages/ws", "wheel-page://pages/artifacts/p22/"] {
            assert_eq!(daemon_url(other, "ws://127.0.0.1:7499/ws", "p2"), None, "{other}");
            assert!(!in_folder(other, "p2"), "{other}");
        }
        assert!(in_folder("wheel-page://pages/artifacts/p2/b.html#x", "p2"));
        assert_eq!(daemon_of("ws://127.0.0.1:7499/ws?token=abc&x=1"), Some(("ws://127.0.0.1:7499/ws".into(), "abc".into())));
    }

    #[test]
    fn frames_load_inline_documents_only() {
        for inline in ["about:srcdoc", "about:blank"] {
            assert!(allows_inline_frame(inline, false));
            assert!(!allows_inline_frame(inline, true));
        }
        for other in ["about:config", "http://evil.example/", "wheel-page://pages/artifacts/p2/", "file:///etc/hosts"] {
            assert!(!allows_inline_frame(other, false), "{other}");
        }
    }
}
