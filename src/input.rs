use alacritty_terminal::term::TermMode;
use gpui_kit::{Keystroke, Modifiers, MouseButton};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseReport {
    Press(MouseButton),
    Release(MouseButton),
    Motion(Option<MouseButton>),
    WheelUp,
    WheelDown,
}

pub fn mouse_reporting(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

fn mouse_modifiers(modifiers: &Modifiers) -> u8 {
    4 * u8::from(modifiers.shift) | 8 * u8::from(modifiers.alt) | 16 * u8::from(modifiers.control)
}

fn mouse_button(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Navigate(_) => None,
    }
}

/// Encode a cell-based mouse event using the protocol selected by the application.
/// `row` and `col` are zero-based visible viewport coordinates.
pub fn encode_mouse(
    report: MouseReport,
    row: usize,
    col: usize,
    modifiers: &Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !mouse_reporting(mode) {
        return None;
    }

    let (button, release, motion) = match report {
        MouseReport::Press(button) => (mouse_button(button)?, false, false),
        MouseReport::Release(button) => (mouse_button(button)?, true, false),
        MouseReport::Motion(button) => {
            let enabled = mode.contains(TermMode::MOUSE_MOTION)
                || (button.is_some() && mode.contains(TermMode::MOUSE_DRAG));
            if !enabled {
                return None;
            }
            (
                match button {
                    Some(button) => mouse_button(button)?,
                    None => 3,
                },
                false,
                true,
            )
        }
        MouseReport::WheelUp => (64, false, false),
        MouseReport::WheelDown => (65, false, false),
    };
    let code = button | mouse_modifiers(modifiers) | if motion { 32 } else { 0 };
    let row = row + 1;
    let col = col + 1;

    if mode.contains(TermMode::SGR_MOUSE) {
        let action = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{col};{row}{action}").into_bytes());
    }

    // The original protocol reports every release as button 3. UTF-8 mode only
    // extends the coordinate range; the three values remain character encoded.
    let code = if release {
        3 | mouse_modifiers(modifiers)
    } else {
        code
    };
    let max_coordinate = if mode.contains(TermMode::UTF8_MOUSE) {
        2015
    } else {
        223
    };
    if row > max_coordinate || col > max_coordinate {
        return None;
    }
    let mut bytes = b"\x1b[M".to_vec();
    if mode.contains(TermMode::UTF8_MOUSE) {
        for value in [u32::from(code) + 32, col as u32 + 32, row as u32 + 32] {
            let value = char::from_u32(value)?;
            let mut encoded = [0; 4];
            bytes.extend_from_slice(value.encode_utf8(&mut encoded).as_bytes());
        }
    } else {
        bytes.extend_from_slice(&[code + 32, col as u8 + 32, row as u8 + 32]);
    }
    Some(bytes)
}

pub fn encode_focus(focused: bool, mode: TermMode) -> Option<Vec<u8>> {
    mode.contains(TermMode::FOCUS_IN_OUT)
        .then(|| if focused { b"\x1b[I" } else { b"\x1b[O" }.to_vec())
}

pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

/// Printable text is delivered by EntityInputHandler, so IME commits are not duplicated.
pub fn encode_key(key: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let modifiers = &key.modifiers;
    if modifiers.platform {
        return None;
    }
    let modifier = 1
        + u8::from(modifiers.shift)
        + 2 * u8::from(modifiers.alt)
        + 4 * u8::from(modifiers.control);
    if let Some(index) = key
        .key
        .strip_prefix('f')
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (1..=12).contains(n))
    {
        return Some(
            if index <= 4 {
                let suffix = ['P', 'Q', 'R', 'S'][index - 1];
                if modifier == 1 {
                    format!("\x1bO{suffix}")
                } else {
                    format!("\x1b[1;{modifier}{suffix}")
                }
            } else {
                let code = [15, 17, 18, 19, 20, 21, 23, 24][index - 5];
                if modifier == 1 {
                    format!("\x1b[{code}~")
                } else {
                    format!("\x1b[{code};{modifier}~")
                }
            }
            .into_bytes(),
        );
    }
    match key.key.as_str() {
        "delete" => {
            return Some(
                if modifier == 1 {
                    "\x1b[3~".into()
                } else {
                    format!("\x1b[3;{modifier}~")
                }
                .into_bytes(),
            );
        }
        "insert" => return (!modifiers.shift && !modifiers.control).then(|| b"\x1b[2~".to_vec()),
        "pageup" | "pagedown" => {
            if modifiers.shift {
                return None;
            } // Scrollback is handled by the view.
            let code = if key.key == "pageup" { 5 } else { 6 };
            return Some(
                if modifiers.control {
                    format!("\x1b[{code};{modifier}~")
                } else {
                    format!("\x1b[{code}~")
                }
                .into_bytes(),
            );
        }
        "tab" => {
            return Some(if modifiers.shift {
                b"\x1b[Z".to_vec()
            } else {
                vec![9]
            });
        }
        _ => {}
    }
    if modifiers.control && modifiers.shift && (key.key.len() == 1 || key.key == "space") {
        return None;
    } // Workspace shortcuts/IME.
    let suffix = match key.key.as_str() {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(suffix) = suffix {
        return Some(
            if modifier != 1 {
                format!("\x1b[1;{modifier}{suffix}")
            } else if mode.contains(TermMode::APP_CURSOR) {
                format!("\x1bO{suffix}")
            } else {
                format!("\x1b[{suffix}")
            }
            .into_bytes(),
        );
    }
    let bytes = match key.key.as_str() {
        "enter" => b"\r".to_vec(),
        "backspace" => vec![if modifiers.control { 8 } else { 127 }],
        "escape" => vec![27],
        "space" if modifiers.control => vec![0],
        value if modifiers.control && value.len() == 1 => {
            let c = value.as_bytes()[0].to_ascii_uppercase();
            match c {
                b'@'..=b'_' => vec![c & 0x1f],
                b'?' => vec![127],
                b'3'..=b'7' => vec![c - b'3' + 27],
                b'8' => vec![127],
                _ => return None,
            }
        }
        value if modifiers.alt => key
            .key_char
            .clone()
            .unwrap_or_else(|| value.to_owned())
            .into_bytes(),
        _ => return None,
    };
    if modifiers.alt {
        Some([vec![27], bytes].concat())
    } else {
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_keys_respect_application_mode_and_shortcuts() {
        assert_eq!(
            encode_key(&Keystroke::parse("up").unwrap(), TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key(&Keystroke::parse("ctrl-c").unwrap(), TermMode::empty()),
            Some(vec![3])
        );
        assert_eq!(
            encode_key(
                &Keystroke::parse("ctrl-shift-c").unwrap(),
                TermMode::empty()
            ),
            None
        );
        assert_eq!(
            encode_key(&Keystroke::parse("a").unwrap(), TermMode::empty()),
            None
        );
    }

    #[test]
    fn sgr_mouse_reports_buttons_motion_wheel_and_modifiers() {
        let mode = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        assert_eq!(
            encode_mouse(
                MouseReport::Press(MouseButton::Left),
                4,
                9,
                &Modifiers::default(),
                mode
            ),
            Some(b"\x1b[<0;10;5M".to_vec())
        );
        assert_eq!(
            encode_mouse(
                MouseReport::Motion(Some(MouseButton::Left)),
                4,
                10,
                &Modifiers::default(),
                mode
            ),
            Some(b"\x1b[<32;11;5M".to_vec())
        );
        assert_eq!(
            encode_mouse(
                MouseReport::Motion(None),
                4,
                10,
                &Modifiers::default(),
                mode
            ),
            None
        );
        assert_eq!(
            encode_mouse(
                MouseReport::WheelDown,
                4,
                9,
                &Modifiers {
                    control: true,
                    ..Default::default()
                },
                mode
            ),
            Some(b"\x1b[<81;10;5M".to_vec())
        );
        assert_eq!(
            encode_mouse(
                MouseReport::Release(MouseButton::Right),
                4,
                9,
                &Modifiers {
                    alt: true,
                    ..Default::default()
                },
                mode
            ),
            Some(b"\x1b[<10;10;5m".to_vec())
        );
    }

    #[test]
    fn legacy_and_utf8_mouse_encodings_respect_coordinate_limits() {
        let legacy = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            encode_mouse(
                MouseReport::Press(MouseButton::Middle),
                1,
                2,
                &Modifiers::default(),
                legacy
            ),
            Some(vec![0x1b, b'[', b'M', 33, 35, 34])
        );
        assert_eq!(
            encode_mouse(
                MouseReport::Release(MouseButton::Middle),
                1,
                2,
                &Modifiers::default(),
                legacy
            ),
            Some(vec![0x1b, b'[', b'M', 35, 35, 34])
        );
        assert!(
            encode_mouse(
                MouseReport::Press(MouseButton::Left),
                0,
                223,
                &Modifiers::default(),
                legacy
            )
            .is_none()
        );

        let utf8 = legacy | TermMode::UTF8_MOUSE;
        let report = encode_mouse(
            MouseReport::Press(MouseButton::Left),
            0,
            300,
            &Modifiers::default(),
            utf8,
        )
        .unwrap();
        assert_eq!(String::from_utf8(report).unwrap(), "\x1b[M \u{14d}!");
    }

    #[test]
    fn all_motion_and_focus_reports_follow_negotiated_modes() {
        let mode = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        assert_eq!(
            encode_mouse(MouseReport::Motion(None), 2, 3, &Modifiers::default(), mode),
            Some(b"\x1b[<35;4;3M".to_vec())
        );
        assert_eq!(encode_focus(true, TermMode::empty()), None);
        assert_eq!(
            encode_focus(true, TermMode::FOCUS_IN_OUT),
            Some(b"\x1b[I".to_vec())
        );
        assert_eq!(
            encode_focus(false, TermMode::FOCUS_IN_OUT),
            Some(b"\x1b[O".to_vec())
        );
    }
}
