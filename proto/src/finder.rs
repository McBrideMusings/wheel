//! Finding a literal piece of text in terminal output that arrives in pieces.

/// Where the escape-sequence stripper stands between pieces.
#[derive(Clone, Copy, PartialEq)]
enum Esc {
    Ground,
    /// After ESC.
    Start,
    /// After ESC and an intermediate byte (`ESC ( B`): one final byte follows.
    Intermediate,
    /// Inside `ESC [`, up to its final byte.
    Csi,
    /// Inside `ESC ]` up to BEL or ST.
    Osc,
    /// Inside DCS, SOS, PM or APC up to ST.
    Str,
    /// An ESC seen inside Osc or Str: a following `\` ends it.
    StrEnd,
}

/// Looks for one literal text in the text a terminal shows, in a stream of bytes. Escape
/// sequences are dropped before matching, so styled cells (`build` in one colour, `ok` in
/// another) still match `build ok`; a cursor move to another row counts as a line break and a move along a row as a space. It
/// works on bytes, not text, so a character split across two pieces still matches. Used by
/// `wheel term read --until` and by the lead's terminal tools, so both agree on what "appeared"
/// means.
pub struct TextFinder {
    esc: Esc,
    /// Parameter bytes of the CSI sequence in progress.
    params: Vec<u8>,
    /// The row of the last absolute cursor move (`CSI row;col H`).
    row: Option<u32>,
    needle: Vec<u8>,
    /// The last `needle.len() - 1` bytes seen: the only ones that can still start a match.
    tail: Vec<u8>,
}

impl TextFinder {
    pub fn new(needle: &str) -> Self {
        TextFinder {
            esc: Esc::Ground,
            params: Vec::new(),
            row: Some(1),
            needle: needle.as_bytes().to_vec(),
            tail: Vec::new(),
        }
    }

    /// Take the next piece of the stream; true once the text has appeared. An empty text appears
    /// with the first piece.
    pub fn feed(&mut self, piece: &[u8]) -> bool {
        if self.needle.is_empty() {
            return true;
        }
        self.strip_into_tail(piece);
        if self
            .tail
            .windows(self.needle.len())
            .any(|w| w == self.needle)
        {
            return true;
        }
        let keep = self.needle.len().saturating_sub(1);
        if self.tail.len() > keep {
            self.tail.drain(..self.tail.len() - keep);
        }
        false
    }

    /// Forget everything seen, as when the stream starts over.
    pub fn clear(&mut self) {
        self.tail.clear();
        self.esc = Esc::Ground;
        self.params.clear();
        self.row = Some(1);
    }

    /// Append the printable part of `piece` to the tail, skipping escape sequences.
    fn strip_into_tail(&mut self, piece: &[u8]) {
        for &b in piece {
            if matches!(b, 0x18 | 0x1a) {
                // CAN and SUB abort any sequence in progress.
                self.esc = Esc::Ground;
                continue;
            }
            self.esc = match (self.esc, b) {
                (Esc::Ground, 0x1b) => Esc::Start,
                (Esc::Ground, _) => {
                    self.tail.push(b);
                    Esc::Ground
                }
                (Esc::Start, b'[') => {
                    self.params.clear();
                    Esc::Csi
                }
                (Esc::Start, b']') => Esc::Osc,
                (Esc::Start, b'P' | b'X' | b'^' | b'_') => Esc::Str,
                // SS2 and SS3 take one more byte.
                (Esc::Start, b'N' | b'O') => Esc::Intermediate,
                (Esc::Start, 0x20..=0x2f) => Esc::Intermediate,
                (Esc::Start, 0x1b) => Esc::Start,
                (Esc::Start, _) | (Esc::Intermediate, 0x30..=0x7e) => Esc::Ground,
                (Esc::Intermediate, _) => Esc::Intermediate,
                (Esc::Csi, 0x40..=0x7e) => {
                    self.csi_final(b);
                    Esc::Ground
                }
                (Esc::Csi, 0x1b) => Esc::Start,
                (Esc::Csi, _) => {
                    self.params.push(b);
                    Esc::Csi
                }
                (Esc::Osc | Esc::Str, 0x07) => Esc::Ground,
                (Esc::Osc | Esc::Str, 0x1b) => Esc::StrEnd,
                (Esc::Osc, _) => Esc::Osc,
                (Esc::Str, _) => Esc::Str,
                (Esc::StrEnd, b'\\') => Esc::Ground,
                (Esc::StrEnd, 0x1b) => Esc::StrEnd,
                (Esc::StrEnd, _) => Esc::Start,
            };
        }
    }

    /// A finished CSI sequence: moving to another row is a line break, moving along a row is a
    /// space, and everything else (colours, modes) is dropped.
    fn csi_final(&mut self, last: u8) {
        match last {
            b'H' | b'f' => {
                let first = self.params.split(|&c| c == b';').next().unwrap_or(&[]);
                let row = std::str::from_utf8(first)
                    .ok()
                    .and_then(|r| r.parse::<u32>().ok())
                    .unwrap_or(1);
                if self.row.is_some_and(|r| r != row) {
                    self.tail.push(b'\n');
                }
                self.row = Some(row);
            }
            b'A' | b'B' | b'E' | b'F' | b'd' => {
                self.tail.push(b'\n');
                self.row = None;
            }
            b'C' | b'G' => self.tail.push(b' '),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TextFinder;

    fn found(needle: &str, pieces: &[&[u8]]) -> bool {
        let mut f = TextFinder::new(needle);
        pieces.iter().any(|p| f.feed(p))
    }

    #[test]
    fn styled_cells_match_as_one_text() {
        assert!(found(
            "build ok",
            &[b"\x1b[1;1H\x1b[31mbuild\x1b[0m \x1b[32mok\x1b[0m"]
        ));
    }

    #[test]
    fn a_sequence_split_across_pieces_is_still_dropped() {
        assert!(found(
            "build ok",
            &[b"bui\x1b[3", b"1mld \x1b]0;ti", b"tle\x07ok"]
        ));
        assert!(found("ab", &[b"a\x1b]0;t\x1b", b"\\b"]));
    }

    #[test]
    fn escape_bytes_are_not_matchable() {
        assert!(!found("31m", &[b"\x1b[31mred"]));
    }

    #[test]
    fn a_row_change_is_a_line_break() {
        assert!(found("one\ntwo", &[b"one\x1b[2;1Htwo"]));
        assert!(!found("onetwo", &[b"one\x1b[2;1Htwo"]));
    }

    #[test]
    fn a_move_along_the_same_row_is_not_a_line_break() {
        assert!(!found("build\nok", &[b"\x1b[1;1Hbuild\x1b[1;7Hok"]));
        assert!(found("buildok", &[b"\x1b[1;1Hbuild\x1b[1;6Hok"]));
        assert!(found("build ok", &[b"\x1b[1;1Hbuild\x1b[Cok"]));
    }

    #[test]
    fn clear_forgets_a_half_read_sequence() {
        let mut f = TextFinder::new("ok");
        f.feed(b"\x1b[3");
        f.clear();
        assert!(f.feed(b"ok"));
    }

    #[test]
    fn can_aborts_a_sequence() {
        assert!(found("ok", &[b"\x1b]0;never ended\x18ok"]));
    }

    #[test]
    fn plain_text_still_matches() {
        assert!(found("cli-42", &[b"cli-", b"42\r\n"]));
    }
}
