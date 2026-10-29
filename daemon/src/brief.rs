//! What every agent pane wheeld starts is told about Wheel. The text is appended to the agent's
//! system prompt (`_meta.systemPrompt.append`), so it reaches the agent whatever Claude
//! configuration it runs with, and it ships inside the binary, so no folder can go missing.

use wheel_proto::ArtifactInfo;

/// How to use Wheel through the `wheel` command.
const BRIEF: &str = include_str!("brief.md");

/// Most artifacts the brief lists, and most bytes its artifacts section takes (ADR-0018).
const LISTED_MAX: usize = 20;
const SECTION_MAX: usize = 2000;
/// Most bytes of one artifact's description in the section.
const DESCRIPTION_MAX: usize = 200;

/// The brief for the agent in `pane`, whose session is `session`; `artifacts` is
/// [`artifacts_section`].
pub(crate) fn text(pane: &str, session: &str, artifacts: &str) -> String {
    format!("{BRIEF}\n{artifacts}\nYou are the agent in pane {pane} (session {session}). Closing that pane ends you.\n")
}

/// The brief a terminal pane's wrappers hand its agent: the shared brief and the artifacts section.
pub(crate) fn file_text(artifacts: &str) -> String {
    format!("{BRIEF}\n{artifacts}")
}

/// The artifacts that exist in a space, as of when the pane started, one line each. Past
/// [`LISTED_MAX`] entries or [`SECTION_MAX`] bytes it lists what fits and gives the count.
pub(crate) fn artifacts_section(artifacts: &[ArtifactInfo]) -> String {
    let header = "Artifacts in your space, as of when your pane started (`wheel artifact ls --json` lists them now):\n";
    if artifacts.is_empty() {
        return format!("{header}none\n");
    }
    let lines: Vec<String> = artifacts.iter().map(line).collect();
    let more = format!(
        "{} artifacts in all; the rest are in `wheel artifact ls --json`\n",
        lines.len()
    );
    let mut shown = 0;
    let mut used = header.len();
    for l in lines.iter().take(LISTED_MAX) {
        // Room for the count line stays unless this line is the last one there is.
        let reserve = if shown + 1 == lines.len() {
            0
        } else {
            more.len()
        };
        if used + l.len() + reserve > SECTION_MAX {
            break;
        }
        used += l.len();
        shown += 1;
    }
    let mut out = header.to_string();
    for l in &lines[..shown] {
        out.push_str(l);
    }
    if shown < lines.len() {
        out.push_str(&more);
    }
    out
}

/// The section when the artifacts cannot be listed, the reason cut and quoted like a description.
pub(crate) fn unreadable_section(reason: &str) -> String {
    format!(
        "Artifacts in your space: the list could not be read ({}).\n",
        cut(reason)
    )
}

/// `text` cut to [`DESCRIPTION_MAX`] bytes, a run of three quotes in it shortened, because Codex
/// receives the brief in a literal string.
fn cut(text: &str) -> String {
    let mut d = text.replace("'''", "'");
    if d.len() > DESCRIPTION_MAX {
        let mut end = DESCRIPTION_MAX;
        while !d.is_char_boundary(end) {
            end -= 1;
        }
        d.truncate(end);
        d.push('…');
    }
    d
}

/// `- name (scope): description`. A description is cut to [`DESCRIPTION_MAX`] bytes, and a run of
/// three quotes in it is shortened, because Codex receives the brief in a literal string.
fn line(a: &ArtifactInfo) -> String {
    let scope = a.scope.name();
    match a.description.as_deref().filter(|d| !d.is_empty()) {
        None => format!("- {} ({scope})\n", a.name),
        Some(d) => format!("- {} ({scope}): {}\n", a.name, cut(d)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::ArtifactScope;

    fn artifact(name: &str, scope: ArtifactScope, description: Option<&str>) -> ArtifactInfo {
        serde_json::from_value(serde_json::json!({
            "scope": scope.name(),
            "name": name,
            "entry": "",
            "dir": "",
            "title": name,
            "description": description,
            "pane": false,
            "problems": [],
        }))
        .unwrap()
    }

    #[test]
    fn a_pane_gets_the_brief_and_its_seat() {
        let pane = text("p3", "w2", "");
        assert!(pane.starts_with(BRIEF));
        assert!(pane.contains("pane p3 (session w2)"), "{pane}");
    }

    #[test]
    fn two_artifacts_are_both_named_with_scope_and_description() {
        let section = artifacts_section(&[
            artifact("board", ArtifactScope::Project, Some("Open beads by state")),
            artifact("clock", ArtifactScope::Machine, None),
        ]);
        assert!(
            section.contains("- board (project): Open beads by state\n"),
            "{section}"
        );
        assert!(section.contains("- clock (machine)\n"), "{section}");
        assert!(!section.contains("artifacts in all"), "{section}");
        assert!(text("p1", "w1", &section).contains("- clock (machine)"));
        assert!(file_text(&section).contains("- board (project)"));
    }

    #[test]
    fn thirty_artifacts_list_twenty_and_the_count() {
        let all: Vec<_> = (0..30)
            .map(|i| artifact(&format!("a{i:02}"), ArtifactScope::Project, Some("d")))
            .collect();
        let section = artifacts_section(&all);
        assert_eq!(section.matches("\n- a").count(), 20, "{section}");
        assert!(section.contains("- a19 (project)"), "{section}");
        assert!(!section.contains("- a20 "), "{section}");
        assert!(section.contains("30 artifacts in all"), "{section}");
    }

    #[test]
    fn long_descriptions_stop_the_list_at_the_byte_cap() {
        let long = "x".repeat(300);
        let all: Vec<_> = (0..20)
            .map(|i| artifact(&format!("a{i:02}"), ArtifactScope::Machine, Some(&long)))
            .collect();
        let section = artifacts_section(&all);
        assert!(section.len() <= SECTION_MAX, "{}", section.len());
        assert!(section.contains("20 artifacts in all"), "{section}");
        assert!(section.matches("- a").count() < 20, "{section}");
    }

    #[test]
    fn a_description_cannot_close_codex_s_literal_string() {
        let section = artifacts_section(&[artifact("a", ArtifactScope::Project, Some("it'''s"))]);
        assert!(!section.contains("'''"), "{section}");
    }

    #[test]
    fn no_artifacts_says_none() {
        assert!(artifacts_section(&[]).ends_with("none\n"));
    }
}
