//! Common xterm OSC colour queries, overrides and resets.
use super::TerminalTheme;
use crate::terminal::Listener;
use alacritty_terminal::{
    term::{
        Term,
        color::{COUNT, Colors},
    },
    vte::ansi::{Handler, NamedColor, Rgb},
};

pub(super) fn handles_osc(code: &[u8]) -> bool {
    matches!(
        code,
        b"4" | b"10" | b"11" | b"12" | b"104" | b"110" | b"111" | b"112"
    )
}

pub(super) fn osc(
    params: &[Vec<u8>],
    terminator: &str,
    term: &mut Term<Listener>,
    theme: TerminalTheme,
) -> Vec<Vec<u8>> {
    let mut replies = Vec::new();
    let Some(code) = params.first().and_then(|p| number(p)) else {
        return replies;
    };
    let mut set_or_query = |index: usize, prefix: String, value: &[u8]| {
        if value == b"?" {
            let rgb = term.colors()[index].unwrap_or_else(|| theme.rgb(index));
            replies
                .push(format!("\x1b]{prefix};{}{terminator}", rgb_string(pack(rgb))).into_bytes());
        } else if let Some(rgb) = parse_color(value) {
            term.set_color(index, unpack(rgb));
        }
    };
    match code {
        4 => {
            for pair in params[1..].chunks_exact(2) {
                if let Some(index) = number(&pair[0]).filter(|&i| i < 256) {
                    set_or_query(index, format!("4;{index}"), &pair[1]);
                }
            }
        }
        10..=12 => {
            for (offset, value) in params[1..].iter().take(13 - code).enumerate() {
                set_or_query(
                    NamedColor::Foreground as usize + code + offset - 10,
                    (code + offset).to_string(),
                    value,
                );
            }
        }
        104 => {
            if params.len() == 1 || (params.len() == 2 && params[1].is_empty()) {
                for index in 0..256 {
                    term.reset_color(index);
                }
            } else {
                for index in params[1..]
                    .iter()
                    .filter_map(|p| number(p))
                    .filter(|&i| i < 256)
                {
                    term.reset_color(index);
                }
            }
        }
        110..=112 => term.reset_color(NamedColor::Foreground as usize + code - 110),
        _ => {}
    }
    replies
}

pub(super) fn restore_overrides(term: &mut Term<Listener>, colors: &Colors) {
    for index in 0..COUNT {
        if term.colors()[index] != colors[index] {
            match colors[index] {
                Some(rgb) => term.set_color(index, rgb),
                None => term.reset_color(index),
            }
        }
    }
}

fn number(value: &[u8]) -> Option<usize> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn pack(rgb: Rgb) -> u32 {
    (u32::from(rgb.r) << 16) | (u32::from(rgb.g) << 8) | u32::from(rgb.b)
}

fn unpack(rgb: u32) -> Rgb {
    Rgb {
        r: (rgb >> 16) as u8,
        g: (rgb >> 8) as u8,
        b: rgb as u8,
    }
}

fn rgb_string(rgb: u32) -> String {
    let rgb = unpack(rgb);
    format!(
        "rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}",
        rgb.r, rgb.g, rgb.b
    )
}

fn parse_color(value: &[u8]) -> Option<u32> {
    let text = std::str::from_utf8(value).ok()?.to_ascii_lowercase();
    let channel = |hex: &str| {
        if !(1..=4).contains(&hex.len()) || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()
    };
    if let Some(hex) = text.strip_prefix('#') {
        if !matches!(hex.len(), 3 | 6 | 9 | 12) {
            return None;
        }
        let width = hex.len() / 3;
        let mut rgb = 0;
        for chunk in hex.as_bytes().chunks_exact(width) {
            let value = channel(std::str::from_utf8(chunk).ok()?)?;
            let value = if width == 1 {
                value << 4
            } else {
                value >> ((width - 2) * 4)
            };
            rgb = (rgb << 8) | value;
        }
        return Some(rgb);
    }
    if let Some(spec) = text.strip_prefix("rgb:") {
        let channels = spec.split('/').collect::<Vec<_>>();
        if channels.len() != 3 {
            return None;
        }
        if !channels.iter().all(|hex| hex.len() == channels[0].len()) {
            return None;
        }
        let mut rgb = 0;
        for hex in channels {
            let value = channel(hex)?;
            let maximum = (1u32 << (hex.len() * 4)) - 1;
            rgb = (rgb << 8) | ((value * 255 + maximum / 2) / maximum);
        }
        return Some(rgb);
    }
    None
}
