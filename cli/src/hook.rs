//! What a coding agent's hook input says the agent waits on, in words, for `wheel pane report
//! needs_you --hook-input`.

use serde_json::Value;

/// The reason in Claude Code's hook input on stdin: for `PermissionRequest`, the tool and what it
/// would act on ("Bash: make test", "asks: Red or blue?"); for `Notification`, its message. None
/// for input that names neither.
pub fn reason(input: &Value) -> Option<String> {
    let text = match input["hook_event_name"].as_str()? {
        "PermissionRequest" => {
            let tool = input["tool_name"].as_str()?;
            let args = &input["tool_input"];
            if tool == "AskUserQuestion" {
                match args["questions"][0]["question"].as_str() {
                    Some(q) => format!("asks: {q}"),
                    None => "asks a question".to_string(),
                }
            } else {
                // The one field that says what the call acts on, for the tools that have one.
                let what = [
                    "command",
                    "file_path",
                    "notebook_path",
                    "url",
                    "path",
                    "pattern",
                    "query",
                ]
                .iter()
                .find_map(|k| args[*k].as_str())
                .filter(|s| !s.trim().is_empty());
                match what {
                    Some(what) => format!("{tool}: {what}"),
                    None => tool.to_string(),
                }
            }
        }
        "Notification" => input["message"].as_str()?.to_string(),
        _ => return None,
    };
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_permission_request_names_the_tool_and_what_it_acts_on() {
        let bash = json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash",
            "tool_input": {"command": "touch made-by-probe", "description": "Create a file"}});
        assert_eq!(reason(&bash).as_deref(), Some("Bash: touch made-by-probe"));
        let edit = json!({"hook_event_name": "PermissionRequest", "tool_name": "Edit",
            "tool_input": {"file_path": "/a/b.rs", "old_string": "x", "new_string": "y"}});
        assert_eq!(reason(&edit).as_deref(), Some("Edit: /a/b.rs"));
        let bare = json!({"hook_event_name": "PermissionRequest", "tool_name": "mcp__x__y",
            "tool_input": {"n": 1}});
        assert_eq!(reason(&bare).as_deref(), Some("mcp__x__y"));
    }

    #[test]
    fn a_question_is_its_first_question() {
        // The shape Claude Code 2.1.316 sent for an AskUserQuestion call.
        let ask = json!({"hook_event_name": "PermissionRequest", "tool_name": "AskUserQuestion",
            "tool_input": {"questions": [{"question": "Red or blue?", "header": "Color"}]}});
        assert_eq!(reason(&ask).as_deref(), Some("asks: Red or blue?"));
    }

    #[test]
    fn a_notification_is_its_message_and_anything_else_has_none() {
        let n = json!({"hook_event_name": "Notification", "message": "Claude needs your input",
            "notification_type": "elicitation_dialog"});
        assert_eq!(reason(&n).as_deref(), Some("Claude needs your input"));
        assert_eq!(reason(&json!({"hook_event_name": "Stop"})), None);
        assert_eq!(reason(&json!({})), None);
        assert_eq!(
            reason(&json!({"hook_event_name": "Notification", "message": "  "})),
            None
        );
    }
}
