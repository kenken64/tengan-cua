use anyhow::{Result, anyhow, bail};
use enigo::{Direction, Enigo, Key, Keyboard};
use serde::{Deserialize, Serialize};

use crate::platform::primary_modifier;

/// A keyboard instruction from the model: a named key ("enter"), an OS-aware
/// semantic shortcut ("select_all", "copy"), or a modifier combo joined with
/// '+' ("cmd+shift+t"). Validated at deserialization time so an invalid key
/// fails the whole plan before any action executes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct KeySpec(String);

impl KeySpec {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn chord(&self) -> Result<KeyChord> {
        parse_key_spec(&self.0)
    }

    pub(crate) fn press(&self, enigo: &mut Enigo) -> Result<()> {
        let chord = self.chord()?;
        press_chord(enigo, &chord.modifiers, chord.key)
    }
}

impl TryFrom<String> for KeySpec {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self> {
        parse_key_spec(&value)?;
        Ok(Self(value))
    }
}

impl From<KeySpec> for String {
    fn from(spec: KeySpec) -> Self {
        spec.0
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct KeyChord {
    pub(crate) modifiers: Vec<Key>,
    pub(crate) key: Key,
}

pub(crate) fn press_chord(enigo: &mut Enigo, modifiers: &[Key], key: Key) -> Result<()> {
    let mut pressed = Vec::with_capacity(modifiers.len());
    let mut result = Ok(());

    for modifier in modifiers {
        match enigo.key(*modifier, Direction::Press) {
            Ok(()) => pressed.push(*modifier),
            Err(err) => {
                result = Err(anyhow!("{err}"));
                break;
            }
        }
    }

    if result.is_ok() {
        result = enigo
            .key(key, Direction::Click)
            .map_err(|err| anyhow!("{err}"));
    }

    for modifier in pressed.iter().rev() {
        if let Err(err) = enigo.key(*modifier, Direction::Release)
            && result.is_ok()
        {
            result = Err(anyhow!("{err}"));
        }
    }

    result
}

fn parse_key_spec(spec: &str) -> Result<KeyChord> {
    let normalized = spec.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        bail!("key spec is empty");
    }

    if !normalized.contains('+') {
        if let Some(chord) = semantic_shortcut(&normalized) {
            return Ok(chord);
        }

        return Ok(KeyChord {
            modifiers: Vec::new(),
            key: named_key(spec, &normalized)?,
        });
    }

    let mut tokens = normalized
        .split('+')
        .map(str::trim)
        .collect::<Vec<&str>>();
    // "shift++" means shift plus the literal '+' key.
    let key_token = match tokens.pop() {
        Some("") if tokens.len() > 1 && tokens.last() == Some(&"") => {
            tokens.pop();
            "+"
        }
        Some("") => "+",
        Some(token) => token,
        None => bail!("key spec {spec:?} is empty"),
    };

    if tokens.is_empty() {
        bail!("key spec {spec:?} has no modifier before '+'");
    }

    let mut modifiers = Vec::with_capacity(tokens.len());
    for token in tokens {
        let modifier = modifier_key(token)
            .ok_or_else(|| anyhow!("unknown modifier {token:?} in key spec {spec:?}"))?;
        if !modifiers.contains(&modifier) {
            modifiers.push(modifier);
        }
    }

    Ok(KeyChord {
        modifiers,
        key: named_key(spec, key_token)?,
    })
}

/// OS-aware semantic shortcuts. `primary_modifier()` is Command on macOS and
/// Control on Windows/Linux, so "select_all" lands correctly everywhere. The
/// control_* values stay literal Control for terminal line editing.
fn semantic_shortcut(token: &str) -> Option<KeyChord> {
    let primary = primary_modifier();
    let chord = |modifiers: Vec<Key>, ch: char| {
        Some(KeyChord {
            modifiers,
            key: Key::Unicode(ch),
        })
    };

    match token {
        "select_all" => chord(vec![primary], 'a'),
        "copy" => chord(vec![primary], 'c'),
        "paste" => chord(vec![primary], 'v'),
        "cut" => chord(vec![primary], 'x'),
        "undo" => chord(vec![primary], 'z'),
        "redo" => {
            if cfg!(target_os = "macos") {
                chord(vec![Key::Meta, Key::Shift], 'z')
            } else {
                chord(vec![Key::Control], 'y')
            }
        }
        "save" => chord(vec![primary], 's'),
        "find" => chord(vec![primary], 'f'),
        "new_tab" => chord(vec![primary], 't'),
        "close_tab" => chord(vec![primary], 'w'),
        "control_a" | "ctrl_a" => chord(vec![Key::Control], 'a'),
        "control_k" | "ctrl_k" => chord(vec![Key::Control], 'k'),
        "control_u" | "ctrl_u" => chord(vec![Key::Control], 'u'),
        _ => None,
    }
}

fn modifier_key(token: &str) -> Option<Key> {
    match token {
        // "cmd" follows user intent rather than the literal key: tasks written
        // for macOS say cmd+c but mean ctrl+c on Windows/Linux.
        "cmd" | "command" | "primary" => Some(primary_modifier()),
        "ctrl" | "control" => Some(Key::Control),
        "alt" | "option" | "opt" => Some(Key::Alt),
        "shift" => Some(Key::Shift),
        "meta" | "super" | "win" | "windows" => Some(Key::Meta),
        _ => None,
    }
}

fn named_key(spec: &str, token: &str) -> Result<Key> {
    let token = token.replace([' ', '-'], "_");
    let key = match token.as_str() {
        "enter" | "return" => Key::Return,
        "escape" | "esc" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "space" => Key::Space,
        "arrow_up" | "up" => Key::UpArrow,
        "arrow_down" | "down" => Key::DownArrow,
        "arrow_left" | "left" => Key::LeftArrow,
        "arrow_right" | "right" => Key::RightArrow,
        "home" => Key::Home,
        "end" => Key::End,
        "page_up" | "pageup" | "pgup" => Key::PageUp,
        "page_down" | "pagedown" | "pgdn" => Key::PageDown,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        other => {
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Key::Unicode(ch),
                _ => bail!(
                    "unknown key {other:?} in key spec {spec:?}; use a named key, an OS shortcut like select_all, or a combo like cmd+shift+t"
                ),
            }
        }
    };

    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::primary_modifier;

    fn chord(spec: &str) -> KeyChord {
        parse_key_spec(spec).unwrap()
    }

    #[test]
    fn parses_legacy_named_keys() {
        assert_eq!(chord("enter").key, Key::Return);
        assert_eq!(chord("return").key, Key::Return);
        assert_eq!(chord("arrow_down").key, Key::DownArrow);
        assert_eq!(chord("page_down").key, Key::PageDown);
        assert_eq!(chord("f5").key, Key::F5);
        assert!(chord("enter").modifiers.is_empty());
    }

    #[test]
    fn legacy_control_shortcuts_stay_literal_control() {
        let parsed = chord("control_a");
        assert_eq!(parsed.modifiers, vec![Key::Control]);
        assert_eq!(parsed.key, Key::Unicode('a'));
        assert_eq!(chord("ctrl_k").modifiers, vec![Key::Control]);
    }

    #[test]
    fn select_all_uses_platform_primary_modifier() {
        let parsed = chord("select_all");
        assert_eq!(parsed.modifiers, vec![primary_modifier()]);
        assert_eq!(parsed.key, Key::Unicode('a'));
    }

    #[test]
    fn semantic_shortcuts_use_platform_primary_modifier() {
        for (spec, ch) in [("copy", 'c'), ("paste", 'v'), ("cut", 'x'), ("save", 's')] {
            let parsed = chord(spec);
            assert_eq!(parsed.modifiers, vec![primary_modifier()]);
            assert_eq!(parsed.key, Key::Unicode(ch));
        }
    }

    #[test]
    fn cmd_combo_maps_to_primary_modifier_per_os() {
        let parsed = chord("cmd+l");
        assert_eq!(parsed.modifiers, vec![primary_modifier()]);
        assert_eq!(parsed.key, Key::Unicode('l'));

        if cfg!(target_os = "macos") {
            assert_eq!(parsed.modifiers, vec![Key::Meta]);
        } else {
            assert_eq!(parsed.modifiers, vec![Key::Control]);
        }
    }

    #[test]
    fn ctrl_combo_stays_literal_control_on_every_os() {
        let parsed = chord("ctrl+c");
        assert_eq!(parsed.modifiers, vec![Key::Control]);
        assert_eq!(parsed.key, Key::Unicode('c'));
    }

    #[test]
    fn parses_multi_modifier_combos_and_named_combo_keys() {
        let parsed = chord("ctrl+shift+t");
        assert_eq!(parsed.modifiers, vec![Key::Control, Key::Shift]);
        assert_eq!(parsed.key, Key::Unicode('t'));

        let parsed = chord("alt+f4");
        assert_eq!(parsed.modifiers, vec![Key::Alt]);
        assert_eq!(parsed.key, Key::F4);

        let parsed = chord("CMD+Shift+Enter");
        assert_eq!(parsed.modifiers, vec![primary_modifier(), Key::Shift]);
        assert_eq!(parsed.key, Key::Return);
    }

    #[test]
    fn parses_shift_plus_literal_plus_key() {
        let parsed = chord("shift++");
        assert_eq!(parsed.modifiers, vec![Key::Shift]);
        assert_eq!(parsed.key, Key::Unicode('+'));
    }

    #[test]
    fn rejects_unknown_keys_and_modifiers() {
        assert!(parse_key_spec("").is_err());
        assert!(parse_key_spec("bogus_key").is_err());
        assert!(parse_key_spec("hyper+a").is_err());
        assert!(parse_key_spec("a+b").is_err());
    }

    #[test]
    fn key_spec_deserialization_validates_value() {
        assert!(serde_json::from_str::<KeySpec>("\"cmd+shift+t\"").is_ok());
        assert!(serde_json::from_str::<KeySpec>("\"select_all\"").is_ok());
        assert!(serde_json::from_str::<KeySpec>("\"not_a_real_key\"").is_err());
    }
}
