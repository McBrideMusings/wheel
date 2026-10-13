//! Finding a literal piece of text in terminal output that arrives in pieces.

/// Looks for one literal text in a stream of bytes. It works on bytes, not text, so a character
/// split across two pieces still matches. Used by `wheel term read --until` and by the lead's
/// terminal tools, so both agree on what "appeared" means.
pub struct TextFinder {
    needle: Vec<u8>,
    /// The last `needle.len() - 1` bytes seen: the only ones that can still start a match.
    tail: Vec<u8>,
}

impl TextFinder {
    pub fn new(needle: &str) -> Self {
        TextFinder {
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
        self.tail.extend_from_slice(piece);
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
    }
}
