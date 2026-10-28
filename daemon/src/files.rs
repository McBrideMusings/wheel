//! Reading a few lines of a file for a client: the preview behind a file path in a chat reply.
//! Every client reads the same answer here, so a phone previews a file on the daemon's machine.

use std::io::Read;
use std::path::Path;

use wheel_proto::ServerMsg;

use crate::dirs::expand_home;

/// How many bytes of a file are looked at, so a log of gigabytes costs the same as a note.
const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Context lines each side of the wanted line when the request names none.
const DEFAULT_CONTEXT: u32 = 5;
/// The most context lines a request may ask for.
const MAX_CONTEXT: u32 = 200;

/// What `peek` found.
#[derive(Debug, PartialEq)]
pub(crate) struct Peek {
    pub path: String,
    pub found: bool,
    pub start: u32,
    pub lines: Vec<String>,
    pub error: Option<String>,
}

/// The lines of `path` around `line`, or the first ones with no `line`. A relative path is read
/// against `cwd`.
pub(crate) fn peek(path: &str, cwd: Option<&str>, line: Option<u32>, context: Option<u32>) -> Peek {
    let expanded = expand_home(path.trim());
    let missing = |error: Option<String>| Peek {
        path: expanded.clone(),
        found: false,
        start: 1,
        lines: Vec::new(),
        error,
    };
    let joined = if Path::new(&expanded).is_absolute() {
        expanded.clone()
    } else if let Some(cwd) = cwd.filter(|c| Path::new(&expand_home(c)).is_absolute()) {
        format!("{}/{expanded}", expand_home(cwd).trim_end_matches('/'))
    } else {
        return missing(None);
    };
    let Ok(real) = std::fs::canonicalize(&joined) else {
        return missing(None);
    };
    if !std::fs::metadata(&real).is_ok_and(|m| m.is_file()) {
        return missing(None);
    }
    let real_str = real.to_string_lossy().into_owned();
    // A context of 0 with no line asks only whether the file is there: nothing is read.
    if line.is_none() && context == Some(0) {
        return Peek {
            path: real_str,
            found: true,
            start: 1,
            lines: Vec::new(),
            error: None,
        };
    }
    let found = |start: u32, lines: Vec<String>, error: Option<String>| Peek {
        path: real_str.clone(),
        found: true,
        start,
        lines,
        error,
    };
    let mut bytes = Vec::new();
    let read = std::fs::File::open(&real).and_then(|f| f.take(MAX_BYTES).read_to_end(&mut bytes));
    if let Err(e) = read {
        let why = match e.kind() {
            std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
            _ => format!("cannot read: {e}"),
        };
        return found(1, Vec::new(), Some(why));
    }
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return found(1, Vec::new(), Some("binary file".into()));
    }
    let truncated = bytes.len() as u64 == MAX_BYTES;
    let text = String::from_utf8_lossy(&bytes);
    let context = context.unwrap_or(DEFAULT_CONTEXT).min(MAX_CONTEXT) as usize;
    let all: Vec<&str> = text.lines().collect();
    let (first, last) = match line {
        Some(line) => {
            let line = (line.max(1) as usize).min(all.len().max(1));
            (line.saturating_sub(context).max(1), line + context)
        }
        None => (1, context * 2 + 1),
    };
    if truncated && line.is_some_and(|l| l as usize > all.len()) {
        return found(
            1,
            Vec::new(),
            Some("that line is past the first 2 MB of the file".into()),
        );
    }
    let lines: Vec<String> = all
        .iter()
        .skip(first - 1)
        .take(last + 1 - first)
        .map(|l| l.to_string())
        .collect();
    found(first as u32, lines, None)
}

/// Answer `file_peek` and log what was asked and what came back.
pub(crate) fn file_peek(
    req: u64,
    path: &str,
    cwd: Option<&str>,
    line: Option<u32>,
    context: Option<u32>,
) -> ServerMsg {
    let p = peek(path, cwd, line, context);
    tracing::info!(
        path, ?cwd, ?line, resolved = %p.path, found = p.found, start = p.start,
        count = p.lines.len(), error = ?p.error, "file_peek"
    );
    ServerMsg::FilePeek {
        req,
        path: p.path,
        found: p.found,
        start: p.start,
        lines: p.lines,
        error: p.error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wheel-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lines_around_the_wanted_one_and_the_first_lines_without_one() {
        let dir = scratch("lines");
        let body: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        std::fs::write(dir.join("a.txt"), body).unwrap();
        let cwd = dir.to_string_lossy().into_owned();
        let p = peek("a.txt", Some(&cwd), Some(10), Some(2));
        assert_eq!((p.found, p.start), (true, 8));
        assert_eq!(
            p.lines,
            ["line 8", "line 9", "line 10", "line 11", "line 12"]
        );
        let p = peek("a.txt", Some(&cwd), Some(1), Some(2));
        assert_eq!((p.start, p.lines.len()), (1, 3));
        let p = peek("a.txt", Some(&cwd), Some(30), Some(2));
        assert_eq!(
            (p.start, p.lines.last().map(String::as_str)),
            (28, Some("line 30"))
        );
        let p = peek("a.txt", Some(&cwd), None, None);
        assert_eq!((p.start, p.lines.len()), (1, 11));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_that_is_not_a_file_is_not_found() {
        let dir = scratch("missing");
        let cwd = dir.to_string_lossy().into_owned();
        assert!(!peek("nope.txt", Some(&cwd), None, None).found);
        assert!(
            !peek(&cwd, None, None, None).found,
            "a folder is not a file"
        );
        assert!(!peek("relative.txt", None, None, None).found);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_context_of_zero_with_no_line_only_says_the_file_is_there() {
        let dir = scratch("exists");
        std::fs::write(dir.join("big.bin"), [0u8; 16]).unwrap();
        let p = peek(&dir.join("big.bin").to_string_lossy(), None, None, Some(0));
        assert_eq!((p.found, p.lines.len(), p.error), (true, 0, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_binary_file_exists_but_gives_no_lines() {
        let dir = scratch("binary");
        std::fs::write(dir.join("b.bin"), [1u8, 0, 2, 3]).unwrap();
        let p = peek(&dir.join("b.bin").to_string_lossy(), None, None, None);
        assert_eq!(
            (p.found, p.lines.len(), p.error.as_deref()),
            (true, 0, Some("binary file"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
