//! Band colors: the named palette, hex parsing and text contrast.

use std::fmt;

/// Named colors, as (name, background, text).
pub const PALETTE: &[(&str, &str, &str)] = &[
    ("orange", "#ff6a00", "#1a120c"),
    ("amber", "#f5b942", "#1a120c"),
    ("blue", "#3b82f6", "#f8fafc"),
    ("green", "#22c55e", "#052e16"),
    ("red", "#ef4444", "#1a0a0a"),
    ("purple", "#a855f7", "#faf5ff"),
    ("yellow", "#eab308", "#1a1504"),
    ("pink", "#ec4899", "#1a0a12"),
    ("teal", "#14b8a6", "#042f2e"),
];

const DARK_TEXT: &str = "#111111";
const LIGHT_TEXT: &str = "#f8fafc";

/// A resolved band color. `bg` and `fg` are always `#rrggbb`, lowercase.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Color {
    /// Palette name, or the hex value when the color is not in the palette.
    pub label: String,
    pub bg: String,
    pub fg: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ColorError(pub String);

impl fmt::Display for ColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown color: {:?} (use {}, random, #rgb or #rrggbb)",
            self.0,
            names().join(", ")
        )
    }
}

impl std::error::Error for ColorError {}

pub fn names() -> Vec<&'static str> {
    PALETTE.iter().map(|(name, _, _)| *name).collect()
}

pub fn named(name: &str) -> Option<Color> {
    let key = name.trim().to_ascii_lowercase();
    PALETTE
        .iter()
        .find(|(n, _, _)| *n == key)
        .map(|(n, bg, fg)| Color {
            label: (*n).to_string(),
            bg: (*bg).to_string(),
            fg: (*fg).to_string(),
        })
}

/// Parse a palette name or a hex color. "random" is handled by the caller,
/// since it depends on which colors are already in use.
pub fn parse(spec: &str) -> Result<Color, ColorError> {
    if let Some(color) = named(spec) {
        return Ok(color);
    }
    let bg = parse_hex(spec).ok_or_else(|| ColorError(spec.to_string()))?;
    // A hex value that matches a palette entry gets the palette's text color.
    if let Some((name, _, fg)) = PALETTE.iter().find(|(_, pbg, _)| *pbg == bg) {
        return Ok(Color {
            label: (*name).to_string(),
            bg,
            fg: (*fg).to_string(),
        });
    }
    let fg = contrast_text(&bg).to_string();
    Ok(Color {
        label: bg.clone(),
        bg,
        fg,
    })
}

pub fn is_random(spec: &str) -> bool {
    spec.trim().eq_ignore_ascii_case("random")
}

/// Normalize `#rgb`, `#rrggbb` (the `#` is optional) to lowercase `#rrggbb`.
pub fn parse_hex(spec: &str) -> Option<String> {
    let s = spec.trim();
    let digits = s.strip_prefix('#').unwrap_or(s);
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let full: String = match digits.len() {
        3 => digits.chars().flat_map(|c| [c, c]).collect(),
        6 => digits.to_string(),
        _ => return None,
    };
    Some(format!("#{}", full.to_ascii_lowercase()))
}

fn rgb(hex: &str) -> (f64, f64, f64) {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    let c = |shift: u32| f64::from((v >> shift) & 0xff) / 255.0;
    (c(16), c(8), c(0))
}

/// WCAG relative luminance of a `#rrggbb` color.
pub fn luminance(hex: &str) -> f64 {
    let lin = |c: f64| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = rgb(hex);
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

pub fn contrast_ratio(a: &str, b: &str) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Dark or light text, whichever reads better on `bg`.
pub fn contrast_text(bg: &str) -> &'static str {
    if contrast_ratio(bg, DARK_TEXT) >= contrast_ratio(bg, LIGHT_TEXT) {
        DARK_TEXT
    } else {
        LIGHT_TEXT
    }
}

/// Pick a palette color, preferring ones no other band is using.
/// `seed` is any varying number; the caller supplies it so this stays pure.
pub fn pick_random(in_use: &[String], seed: u64) -> Color {
    let free: Vec<_> = PALETTE
        .iter()
        .filter(|(_, bg, _)| !in_use.iter().any(|u| u.eq_ignore_ascii_case(bg)))
        .collect();
    let pool: Vec<_> = if free.is_empty() {
        PALETTE.iter().collect()
    } else {
        free
    };
    let (name, bg, fg) = pool[(seed % pool.len() as u64) as usize];
    Color {
        label: (*name).to_string(),
        bg: (*bg).to_string(),
        fg: (*fg).to_string(),
    }
}

/// A seed for `pick_random` from the kernel, falling back to the clock.
pub fn random_seed() -> u64 {
    let mut buf = [0u8; 8];
    // SAFETY: buf is valid for 8 writable bytes.
    let n = unsafe { libc::getrandom(buf.as_mut_ptr().cast(), buf.len(), 0) };
    if n == 8 {
        return u64::from_ne_bytes(buf);
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_colors_are_case_insensitive() {
        let c = parse("Orange").unwrap();
        assert_eq!(c.label, "orange");
        assert_eq!(c.bg, "#ff6a00");
        assert_eq!(c.fg, "#1a120c");
        assert_eq!(parse("  TEAL ").unwrap().label, "teal");
    }

    #[test]
    fn hex_forms() {
        assert_eq!(parse_hex("#ABC").as_deref(), Some("#aabbcc"));
        assert_eq!(parse_hex("abc").as_deref(), Some("#aabbcc"));
        assert_eq!(parse_hex("#A1b2C3").as_deref(), Some("#a1b2c3"));
        assert_eq!(parse_hex("a1b2c3").as_deref(), Some("#a1b2c3"));
    }

    #[test]
    fn bad_hex_is_rejected() {
        for bad in [
            "", "#", "#zzzzzz", "#12345", "#1234567", "#12", "red;}", "#ff 000", "#ff00",
        ] {
            assert!(parse_hex(bad).is_none(), "{bad:?} should be rejected");
            assert!(parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn css_injection_is_impossible() {
        // The old version accepted any 7-character string starting with '#',
        // which went straight into the stylesheet.
        assert!(parse("#};*{a").is_err());
    }

    #[test]
    fn hex_matching_palette_gets_its_name() {
        let c = parse("#FF6A00").unwrap();
        assert_eq!(c.label, "orange");
        assert_eq!(c.fg, "#1a120c");
    }

    #[test]
    fn custom_hex_gets_readable_text() {
        assert_eq!(parse("#000000").unwrap().fg, LIGHT_TEXT);
        assert_eq!(parse("#ffffff").unwrap().fg, DARK_TEXT);
        assert_eq!(parse("#003366").unwrap().fg, LIGHT_TEXT);
        assert_eq!(parse("#ffee88").unwrap().fg, DARK_TEXT);
        let c = parse("#123456").unwrap();
        assert_eq!(c.label, "#123456");
    }

    #[test]
    fn palette_text_is_readable() {
        for (name, bg, fg) in PALETTE {
            let ratio = contrast_ratio(bg, fg);
            assert!(ratio >= 3.0, "{name}: contrast {ratio:.2} is too low");
        }
    }

    #[test]
    fn palette_entries_are_valid_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for (name, bg, fg) in PALETTE {
            assert_eq!(parse_hex(bg).as_deref(), Some(*bg), "{name} bg");
            assert_eq!(parse_hex(fg).as_deref(), Some(*fg), "{name} fg");
            assert!(seen.insert(*bg), "duplicate background {bg}");
        }
    }

    #[test]
    fn random_prefers_unused_colors() {
        let in_use: Vec<String> = PALETTE[..PALETTE.len() - 1]
            .iter()
            .map(|(_, bg, _)| bg.to_uppercase())
            .collect();
        for seed in 0..20 {
            assert_eq!(pick_random(&in_use, seed).label, PALETTE.last().unwrap().0);
        }
    }

    #[test]
    fn random_falls_back_to_whole_palette() {
        let in_use: Vec<String> = PALETTE.iter().map(|(_, bg, _)| bg.to_string()).collect();
        let seen: std::collections::HashSet<_> = (0..PALETTE.len() as u64)
            .map(|s| pick_random(&in_use, s).label)
            .collect();
        assert_eq!(seen.len(), PALETTE.len());
    }

    #[test]
    fn random_keyword() {
        assert!(is_random("random"));
        assert!(is_random(" RANDOM "));
        assert!(!is_random("rand"));
    }

    #[test]
    fn error_lists_names() {
        let msg = parse("chartreuse").unwrap_err().to_string();
        assert!(msg.contains("chartreuse"));
        assert!(msg.contains("orange"));
        assert!(msg.contains("#rrggbb"));
    }
}
