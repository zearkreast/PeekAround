//! Dominant colour buckets from a thumbhash (mirrors `color.ts`).

use serde::{Deserialize, Serialize};

/// Colour bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorBucket {
    /// red
    Red,
    /// orange
    Orange,
    /// yellow
    Yellow,
    /// green
    Green,
    /// blue
    Blue,
    /// purple
    Purple,
    /// pink
    Pink,
    /// black & white / grey
    Bw,
}

/// stable order (also the numeric value stored in media.color; -1 = unknown)
pub const COLOR_BUCKETS: [ColorBucket; 8] = [
    ColorBucket::Red,
    ColorBucket::Orange,
    ColorBucket::Yellow,
    ColorBucket::Green,
    ColorBucket::Blue,
    ColorBucket::Purple,
    ColorBucket::Pink,
    ColorBucket::Bw,
];

impl ColorBucket {
    /// id string
    pub fn as_str(self) -> &'static str {
        match self {
            ColorBucket::Red => "red",
            ColorBucket::Orange => "orange",
            ColorBucket::Yellow => "yellow",
            ColorBucket::Green => "green",
            ColorBucket::Blue => "blue",
            ColorBucket::Purple => "purple",
            ColorBucket::Pink => "pink",
            ColorBucket::Bw => "bw",
        }
    }

    /// CSS swatch (UI chips)
    pub fn swatch(self) -> &'static str {
        match self {
            ColorBucket::Red => "#e5484d",
            ColorBucket::Orange => "#f76b15",
            ColorBucket::Yellow => "#ffc53d",
            ColorBucket::Green => "#46a758",
            ColorBucket::Blue => "#3e63dd",
            ColorBucket::Purple => "#8e4ec6",
            ColorBucket::Pink => "#e93d82",
            ColorBucket::Bw => "#8b8d98",
        }
    }

    /// index in [`COLOR_BUCKETS`]
    pub fn index(self) -> usize {
        COLOR_BUCKETS.iter().position(|b| *b == self).unwrap_or(0)
    }
}

/// a bucket id string → bucket
pub fn parse_color_bucket(s: &str) -> Option<ColorBucket> {
    COLOR_BUCKETS.iter().copied().find(|b| b.as_str() == s)
}

fn b64_value(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => (c - b'A') as u32,
        b'a'..=b'z' => (c - b'a') as u32 + 26,
        b'0'..=b'9' => (c - b'0') as u32 + 52,
        b'+' | b'-' => 62,
        b'/' | b'_' => 63,
        _ => return None,
    })
}

/// first `n` bytes of a base64 string (standard or url-safe alphabet)
pub fn base64_head(s: &str, n: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(n);
    let mut bits = 0u32;
    let mut acc = 0u32;
    for c in s.encode_utf16() {
        if out.len() >= n {
            break;
        }
        if c == 61 {
            break;
        }
        let v = if c < 128 {
            b64_value(c as u8)?
        } else {
            return None;
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
        acc &= 0xffff; // keep the accumulator small (only the low bits are ever read)
    }
    if out.len() == n {
        Some(out)
    } else {
        None
    }
}

/// RGBA colour (0..1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    /// red
    pub r: f64,
    /// green
    pub g: f64,
    /// blue
    pub b: f64,
    /// alpha
    pub a: f64,
}

/// average colour stored in a thumbhash (same maths as thumbhash's `thumbHashToAverageRGBA`)
pub fn thumbhash_average(hash: &[u8]) -> Option<Rgba> {
    if hash.len() < 6 {
        return None;
    }
    let header = hash[0] as u32 | (hash[1] as u32) << 8 | (hash[2] as u32) << 16;
    let l = (header & 63) as f64 / 63.0;
    let p = ((header >> 6) & 63) as f64 / 31.5 - 1.0;
    let q = ((header >> 12) & 63) as f64 / 31.5 - 1.0;
    let has_alpha = header >> 23;
    let a = if has_alpha != 0 {
        (hash[5] & 15) as f64 / 15.0
    } else {
        1.0
    };
    let b = l - 2.0 / 3.0 * p;
    let r = (3.0 * l - b + q) / 2.0;
    let g = r - q;
    let c = |v: f64| v.clamp(0.0, 1.0);
    Some(Rgba {
        r: c(r),
        g: c(g),
        b: c(b),
        a,
    })
}

/// HSL colour.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Hsl {
    /// hue 0..360
    pub h: f64,
    /// saturation 0..1
    pub s: f64,
    /// lightness 0..1
    pub l: f64,
}

/// rgb 0..1 → hue (0..360), saturation, lightness (0..1)
pub fn rgb_to_hsl(r: f64, g: f64, b: f64) -> Hsl {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return Hsl { h: 0.0, s: 0.0, l };
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let mut h = if max == r {
        ((g - b) / d) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h *= 60.0;
    if h < 0.0 {
        h += 360.0;
    }
    Hsl {
        h,
        s: s.min(1.0),
        l,
    }
}

/// Bucket of an (average) colour.
#[allow(clippy::manual_range_contains)] // exact comparisons of the TypeScript reference
pub fn color_bucket_of(r: f64, g: f64, b: f64) -> ColorBucket {
    let Hsl { h, s, l } = rgb_to_hsl(r, g, b);
    let chroma = r.max(g).max(b) - r.min(g).min(b);
    if l < 0.1 || l > 0.94 || chroma < 0.07 || s < 0.12 {
        return ColorBucket::Bw;
    }
    if !(12.0..345.0).contains(&h) {
        return ColorBucket::Red;
    }
    if h < 42.0 {
        ColorBucket::Orange
    } else if h < 68.0 {
        ColorBucket::Yellow
    } else if h < 165.0 {
        ColorBucket::Green
    } else if h < 255.0 {
        ColorBucket::Blue
    } else if h < 290.0 {
        ColorBucket::Purple
    } else {
        ColorBucket::Pink
    }
}

/// media.color value (index into [`COLOR_BUCKETS`]) of a base64 thumbhash; -1 when it can't be read
pub fn color_index_of_thumbhash(b64: Option<&str>) -> i32 {
    let Some(b64) = b64.filter(|s| !s.is_empty()) else {
        return -1;
    };
    match base64_head(b64, 6).and_then(|h| thumbhash_average(&h)) {
        Some(avg) => color_bucket_of(avg.r, avg.g, avg.b).index() as i32,
        None => -1,
    }
}
