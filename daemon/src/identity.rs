//! Who this wheeld is to the clients paired with it: a random id that never changes, and its
//! platform (ADR-0057).

use std::io::Read;
use std::path::Path;

/// The file in the support directory holding the id.
const FILE: &str = "server-id";

/// What `server.color` takes: nothing, or one of the macOS system colours.
pub(crate) const COLOR_SETTING: [&str; 14] = [
    "", "red", "orange", "yellow", "green", "mint", "teal", "cyan", "blue", "indigo", "purple",
    "pink", "brown", "gray",
];

/// The macOS system colours a server's mark may take.
const COLORS: &[&str] = COLOR_SETTING.split_first().unwrap().1;

/// The id in `<support>/server-id`, written as a random UUID the first time. A file that is empty
/// or holds no UUID was never reported, so it is written again.
pub fn load_or_create(support: &Path) -> std::io::Result<String> {
    let path = support.join(FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let id = text.trim();
        if is_uuid(id) {
            return Ok(id.to_string());
        }
    }
    std::fs::create_dir_all(support)?;
    let mut raw = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut raw)?;
    let id = uuid_v4(raw);
    let part = support.join(format!("{FILE}.part"));
    std::fs::write(&part, format!("{id}\n"))?;
    std::fs::rename(&part, &path)?;
    Ok(id)
}

/// A version 4 UUID, lowercase and hyphenated, from 16 random bytes.
fn uuid_v4(mut raw: [u8; 16]) -> String {
    raw[6] = (raw[6] & 0x0f) | 0x40;
    raw[8] = (raw[8] & 0x3f) | 0x80;
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// `<os>_<arch>` in Go's words, which clients read to pick a server's glyph: `darwin_arm64`,
/// `linux_amd64`.
pub fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    };
    format!("{os}_{arch}")
}

/// The colour a server shows when `server.color` is empty: one of [`COLORS`] picked by its id, so
/// two servers nobody coloured still differ most of the time and each keeps its colour.
pub(crate) fn default_color(id: &str) -> &'static str {
    let sum = id.bytes().fold(0u32, |sum, b| sum.wrapping_mul(31).wrapping_add(b as u32));
    COLORS[sum as usize % COLORS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_survives_a_restart_and_a_deleted_file_gives_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create(dir.path()).unwrap();
        assert!(is_uuid(&first), "{first}");
        assert_eq!(&first[14..15], "4", "version 4: {first}");
        assert_eq!(load_or_create(dir.path()).unwrap(), first);
        std::fs::remove_file(dir.path().join(FILE)).unwrap();
        let second = load_or_create(dir.path()).unwrap();
        assert_ne!(second, first);
        std::fs::write(dir.path().join(FILE), "").unwrap();
        assert!(is_uuid(&load_or_create(dir.path()).unwrap()));
    }

    #[test]
    fn platform_uses_go_names() {
        let p = platform();
        assert!(!p.contains("macos") && !p.contains("aarch64") && !p.contains("x86_64"), "{p}");
        assert_eq!(p.split('_').count(), 2, "{p}");
    }

    #[test]
    fn the_default_colour_is_stable_and_a_system_colour() {
        let id = "0b6f6c56-3d0a-4b8e-9d5c-0d2f1e8a7b11";
        assert_eq!(default_color(id), default_color(id));
        assert!(COLORS.contains(&default_color(id)));
    }
}
