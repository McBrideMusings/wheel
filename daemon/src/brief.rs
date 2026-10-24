//! What every agent pane wheeld starts is told about Wheel. The text is appended to the agent's
//! system prompt (`_meta.systemPrompt.append`), so it reaches the agent whatever Claude
//! configuration it runs with, and it ships inside the binary, so no folder can go missing.

/// How to use Wheel through the `wheel` command.
const BRIEF: &str = include_str!("brief.md");

/// The brief for the agent in `pane`, whose session is `session`.
pub(crate) fn text(pane: &str, session: &str) -> String {
    format!("{BRIEF}\nYou are the agent in pane {pane} (session {session}). Closing that pane ends you.\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_gets_the_brief_and_its_seat() {
        let pane = text("p3", "w2");
        assert!(pane.starts_with(BRIEF));
        assert!(pane.contains("pane p3 (session w2)"), "{pane}");
    }

    #[test]
    fn the_brief_sends_canvas_posts_to_wheel_artifacts() {
        assert!(BRIEF.contains("replaces a Canvas post"));
        assert!(BRIEF.contains("run no `canvas post`"));
    }
}
