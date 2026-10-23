//! The `keybind` setting: key bindings over the Mac app's own, written the way Ghostty writes its
//! `keybind` lines, comma separated. The daemon checks each entry and stores it in one spelling, so
//! a client reads only that spelling and every alias is decided here.
//!
//! ```text
//! clear                       drop every built-in binding before this entry
//! cmd+shift+d=split-down      bind a trigger to an action
//! cmd+d=unbind                free a trigger
//! ```
//!
//! Which actions exist is the app's to say: it reports an action it does not have beside the
//! setting, since the daemon has no list of them.

/// Modifiers in the order macOS prints them (⌃⌥⇧⌘), which is the order they are stored in.
const MODIFIERS: [(&str, &[&str]); 4] = [
    ("ctrl", &["ctrl", "control"]),
    ("opt", &["opt", "option", "alt"]),
    ("shift", &["shift"]),
    ("cmd", &["cmd", "command", "super"]),
];

/// Named keys, each with the spellings it is written as; the first is the stored one.
const NAMED: [&[&str]; 23] = [
    &["enter", "return"],
    &["tab"],
    &["space"],
    &["escape", "esc"],
    &["backspace"],
    &["delete"],
    &["arrow_up", "up"],
    &["arrow_down", "down"],
    &["arrow_left", "left"],
    &["arrow_right", "right"],
    &["home"],
    &["end"],
    &["page_up"],
    &["page_down"],
    &["comma"],
    &["period", "."],
    &["slash", "/"],
    &["semicolon", ";"],
    &["quote", "'"],
    &["backquote", "`"],
    &["minus", "-"],
    &["equal"],
    &["backslash", "\\"],
];

/// Brackets are named apart: their stored names match Ghostty's.
const BRACKETS: [(&str, &str); 2] = [("bracket_left", "["), ("bracket_right", "]")];

/// The setting's value as it is stored, or why it is refused.
pub(crate) fn take(value: &str) -> Result<String, String> {
    let mut out = Vec::new();
    for entry in value.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        out.push(self::entry(entry).map_err(|why| format!("{entry:?}: {why}"))?);
    }
    Ok(out.join(","))
}

fn entry(entry: &str) -> Result<String, String> {
    if entry.eq_ignore_ascii_case("clear") {
        return Ok("clear".into());
    }
    let Some((trigger, action)) = entry.split_once('=') else {
        return Err("write trigger=action, such as cmd+d=split-right, or clear".into());
    };
    Ok(format!(
        "{}={}",
        self::trigger(trigger)?,
        self::action(action)?
    ))
}

/// `cmd+shift+d`: modifiers, then one key.
fn trigger(trigger: &str) -> Result<String, String> {
    let parts: Vec<String> = trigger
        .split('+')
        .map(|p| p.trim().to_ascii_lowercase())
        .collect();
    let (key, mods) = parts.split_last().expect("split gives at least one part");
    let mut held = [false; MODIFIERS.len()];
    for m in mods {
        let Some(i) = MODIFIERS
            .iter()
            .position(|(_, names)| names.contains(&m.as_str()))
        else {
            return Err(format!(
                "{m:?} is not a modifier: use cmd, ctrl, opt or shift"
            ));
        };
        if std::mem::replace(&mut held[i], true) {
            return Err(format!("{m:?} is given twice"));
        }
    }
    let mut out: Vec<&str> = MODIFIERS
        .iter()
        .zip(held)
        .filter(|(_, on)| *on)
        .map(|((name, _), _)| *name)
        .collect();
    let key = self::key(key)?;
    out.push(&key);
    Ok(out.join("+"))
}

fn key(key: &str) -> Result<String, String> {
    if key.is_empty() {
        return Err("the trigger names no key: end it with one, such as d or comma".into());
    }
    if key.len() == 1 && key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok(key.into());
    }
    if let Some(digit) = key
        .strip_prefix("digit_")
        .filter(|d| d.len() == 1 && d.chars().all(|c| c.is_ascii_digit()))
    {
        return Ok(digit.into());
    }
    if let Some(n) = key
        .strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .filter(|n| (1..=12).contains(n))
    {
        return Ok(format!("f{n}"));
    }
    if let Some((name, _)) = BRACKETS
        .iter()
        .find(|(name, lit)| key == *name || key == *lit)
    {
        return Ok((*name).into());
    }
    if let Some(names) = NAMED.iter().find(|names| names.contains(&key)) {
        return Ok(names[0].into());
    }
    Err(format!(
        "{key:?} is not a key: use a letter, a digit, f1–f12, or a name such as enter, tab, escape, arrow_left, comma, equal, minus or bracket_left"
    ))
}

/// An action id, `split-right`; Ghostty's underscores are taken as dashes.
fn action(action: &str) -> Result<String, String> {
    let action = action.trim().to_ascii_lowercase().replace('_', "-");
    match !action.is_empty()
        && action
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        true => Ok(action),
        false => Err(format!(
            "{action:?} is not an action: Settings › Keys lists them, such as split-right, or unbind"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every alias is stored in one spelling, modifiers in ⌃⌥⇧⌘ order, so the app parses one form.
    #[test]
    fn bindings_are_stored_in_one_spelling() {
        assert_eq!(
            take(" Super+Shift+D = split_down , cmd+d=unbind,CLEAR, alt+command+Left=focus-left ")
                .unwrap(),
            "shift+cmd+d=split-down,cmd+d=unbind,clear,opt+cmd+arrow_left=focus-left"
        );
        assert_eq!(
            take("cmd+[=focus-previous,cmd+shift+bracket_right=next-space,ctrl+cmd+equal=equalize-splits,cmd+digit_1=space-1,cmd+return=x,cmd+f12=y")
                .unwrap(),
            "cmd+bracket_left=focus-previous,shift+cmd+bracket_right=next-space,ctrl+cmd+equal=equalize-splits,cmd+1=space-1,cmd+enter=x,cmd+f12=y"
        );
        assert_eq!(take("").unwrap(), "");
    }

    #[test]
    fn a_malformed_entry_is_refused_naming_it() {
        for (bad, says) in [
            ("cmd+d", "trigger=action"),
            ("hyper+d=x", "not a modifier"),
            ("cmd+cmd+d=x", "given twice"),
            ("cmd+=x", "names no key"),
            ("cmd+dd=x", "not a key"),
            ("cmd+f13=x", "not a key"),
            ("cmd+d=split right", "not an action"),
            ("cmd+d=", "not an action"),
            ("cmd+d=new_split:right", "not an action"),
        ] {
            let e = take(&format!("cmd+k=ok,{bad}")).unwrap_err();
            assert!(e.contains(says) && e.contains(bad), "{bad:?}: {e}");
        }
    }
}
