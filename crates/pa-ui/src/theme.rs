//! Theme helpers: accent partner colours (the web UI's `useTheme.ts` `mix()`: hue rotation in HSL).

/// "#rrggbb" → (r, g, b)
pub fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// (r, g, b) → "#rrggbb"
pub fn to_hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Rotate the hue of `hex` by `amount` degrees (gradient partner colours --accent-2 / --accent-3).
pub fn mix(hex: &str, amount: f64) -> String {
    let Some((r, g, b)) = parse_hex(hex) else {
        return hex.to_string();
    };
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d == 0.0 {
        0.0
    } else {
        d / (1.0 - (2.0 * l - 1.0).abs())
    };
    let mut h = 0.0;
    if d != 0.0 {
        // JS `%` keeps the sign of the dividend
        h = if max == r {
            ((g - b) / d) % 6.0
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        h *= 60.0;
    }
    h = (h + amount + 360.0) % 360.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let f = |v: f64| ((v + m) * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
    to_hex((f(r), f(g), f(b)))
}

/// slint colour of "#rrggbb"
pub fn color(hex: &str) -> slint::Color {
    let (r, g, b) = parse_hex(hex).unwrap_or((0x8b, 0x5c, 0xf6));
    slint::Color::from_rgb_u8(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hue_rotation_matches_web() {
        // node: mix('#8b5cf6', 38), mix('#8b5cf6', 75) with useTheme.ts
        assert_eq!(mix("#8b5cf6", 0.0), "#8b5cf6");
        assert_eq!(parse_hex("#10b981"), Some((0x10, 0xb9, 0x81)));
        assert_eq!(parse_hex("red"), None);
        assert_eq!(mix("#8b5cf6", 38.0), "#ed5cf6");
        assert_eq!(mix("#8b5cf6", 75.0), "#f65ca0");
        assert_eq!(mix("#10b981", 38.0), "#1086b9");
        assert_eq!(mix("#ef4444", 75.0), "#c4ef44");
        assert_eq!(mix("#808080", 90.0), "#808080");
    }
}
