//! What every agent wheeld starts for the person — the lead and each agent pane — is told about
//! Wheel. The text is appended to the agent's system prompt (`_meta.systemPrompt.append`), so it
//! reaches the agent whatever Claude configuration it runs with, and it ships inside the binary,
//! so no folder can go missing.

/// How to use Wheel and its `wheel` MCP tools.
const BRIEF: &str = include_str!("brief.md");

/// Supervisors and the inbox, told only when the daemon runs them (`--orchestration`).
const ORCHESTRATION: &str = include_str!("brief-orchestration.md");

/// Which agent the brief is for, so it knows where it sits.
pub(crate) enum Seat<'a> {
    Lead,
    Pane { pane: &'a str, session: &'a str },
}

/// The brief for the agent in `seat`.
pub(crate) fn text(seat: Seat, orchestration: bool) -> String {
    let seat = match seat {
        Seat::Lead => {
            "You are the lead: the agent in the floating card beside every space. You have no pane."
                .to_string()
        }
        Seat::Pane { pane, session } => {
            format!(
                "You are the agent in pane {pane} (session {session}). Closing that pane ends you."
            )
        }
    };
    let mut text = format!("{BRIEF}\n{seat}\n");
    if orchestration {
        text.push('\n');
        text.push_str(ORCHESTRATION);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lead_and_a_pane_get_the_same_brief_but_their_seat() {
        let lead = text(Seat::Lead, false);
        let pane = text(
            Seat::Pane {
                pane: "p3",
                session: "w2",
            },
            false,
        );
        assert!(lead.starts_with(BRIEF) && pane.starts_with(BRIEF));
        assert!(pane.contains("pane p3 (session w2)"), "{pane}");
        assert!(!lead.contains("start_supervisor"), "{lead}");
        assert!(text(Seat::Lead, true).contains("start_supervisor"));
    }
}
