//! Voice mode on the Mac (wheel-e1xk.11): what the layer over an agent pane shows for the state
//! of WheelUI's `Voice`, kept apart from the drawing so it is tested.

use wheel_proto::CardRef;

/// One voice session: the agent pane it covers and what it has done with wheeld.
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub agent: String,
    pub pane: String,
    /// Shrunk to the pill over the transcript (the panes button, Type instead…, a card).
    pub pill: bool,
    /// The `lease_claim` request, until wheeld answers it.
    pub claim_req: Option<u64>,
    /// This connection's id as the `voice` lease names its holder, once claimed.
    pub holder: Option<String>,
    /// The last spoken turn already sent, so the same event is never sent twice.
    pub sent: u64,
    /// The transcript's row count when voice opened: a card after it shows above the orb.
    pub rows_at_open: usize,
}

/// How the orb looks: its colour, its size against the full orb (`0..=1.2`), and the word under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orb {
    pub color: u32,
    pub scale: f32,
    pub word: &'static str,
}

/// The orb for `phase` (`Voice.Phase`), `muted` and the `level` moving it (design §5 Orb states):
/// Listening blue with rings at the microphone's level, Thinking grey, Speaking bright with the
/// speech's level, Muted dimmed.
pub fn orb(phase: &str, muted: bool, level: f32) -> Orb {
    let level = level.clamp(0., 1.);
    if muted {
        return Orb { color: 0x3a4a63, scale: 0.9, word: "Muted" };
    }
    match phase {
        "listening" => Orb { color: 0x5b8def, scale: 0.92 + 0.2 * level, word: "Listening" },
        "hearing" => Orb { color: 0x5b8def, scale: 0.95 + 0.25 * level, word: "Listening" },
        "transcribing" | "thinking" => Orb { color: 0x7d8794, scale: 0.9, word: "Thinking" },
        "speaking" => Orb { color: 0x9cc2ff, scale: 0.95 + 0.25 * level, word: "Speaking · tap to interrupt" },
        "asking" | "downloading" => Orb { color: 0x3a4a63, scale: 0.85, word: "Needs the speech models" },
        "starting" => Orb { color: 0x3a4a63, scale: 0.85, word: "Starting…" },
        _ => Orb { color: 0x3a4a63, scale: 0.85, word: "Off" },
    }
}

/// The name a card's chip above the orb shows.
pub fn card_title(card: &CardRef) -> String {
    match card {
        CardRef::Pane { pane } => format!("Pane {pane}"),
        CardRef::Artifact { name, .. } => name.clone(),
        CardRef::Unknown => "A card".into(),
    }
}

/// The caption under the orb: what is being said, cut to its last `max` characters so the newest
/// words stay in view.
pub fn caption(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(count - max + 1).collect();
    format!("…{}", tail.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_orb_follows_the_phase_and_mute_wins() {
        assert_eq!(orb("listening", false, 0.).word, "Listening");
        assert!(orb("listening", false, 1.).scale > orb("listening", false, 0.).scale);
        assert_eq!(orb("thinking", false, 1.).scale, 0.9);
        assert_eq!(orb("speaking", false, 0.).word, "Speaking · tap to interrupt");
        assert_eq!(orb("speaking", true, 0.5).word, "Muted");
        assert_eq!(orb("listening", false, 7.).scale, orb("listening", false, 1.).scale);
    }

    #[test]
    fn a_long_caption_keeps_its_end() {
        assert_eq!(caption("short", 10), "short");
        assert_eq!(caption("one two three four", 10), "…hree four");
        assert_eq!(caption("one two three four", 10).chars().count(), 10);
    }

    #[test]
    fn a_card_is_named_by_what_it_shows() {
        assert_eq!(card_title(&CardRef::Pane { pane: "p5".into() }), "Pane p5");
        assert_eq!(card_title(&CardRef::Unknown), "A card");
    }
}
