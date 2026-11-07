//! Media type classification + camera names + messaging apps (mirrors `classify.ts`).

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::js;
use crate::types::media_flag;

/// Input of [`classify`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClassifyInput {
    /// file name
    pub name: String,
    /// folder-relative dir ('/' separated)
    #[serde(default)]
    pub dir: Option<String>,
    /// extension
    pub ext: String,
    /// media kind ('image', 'video', …)
    pub kind: String,
    /// px
    #[serde(default)]
    pub width: Option<f64>,
    /// px
    #[serde(default)]
    pub height: Option<f64>,
    /// EXIF make
    #[serde(default)]
    pub make: Option<String>,
    /// EXIF model
    #[serde(default)]
    pub model: Option<String>,
    /// lens model
    #[serde(default)]
    pub lens: Option<String>,
    /// XMP GPano equirectangular / UsePanoramaViewer
    #[serde(default)]
    pub gpano: Option<bool>,
    /// frames
    #[serde(default)]
    pub pages: Option<f64>,
}

fn is_mark(c: char) -> bool {
    ('\u{300}'..='\u{36f}').contains(&c)
}

fn fold(s: &str) -> String {
    let pre: String = s
        .chars()
        .map(|c| {
            if matches!(c, 'İ' | 'I' | 'ı') {
                'i'
            } else {
                c
            }
        })
        .collect();
    let n: String = pre
        .nfkd()
        .filter(|c| !is_mark(*c))
        .map(|c| {
            if matches!(c, '’' | '‘' | '`' | '´') {
                '\''
            } else {
                c
            }
        })
        .collect();
    n.to_lowercase()
}

const SCREENSHOT_NAME: &[&str] = &[
    "screenshot",
    "screen shot",
    "screen_shot",
    "screen-shot",
    "screencap",
    "scrnshot",
    "ekran goruntusu",
    "ekran resmi",
    "ekran_goruntusu",
    "bildschirmfoto",
    "bildschirmaufnahme",
    "capture d'ecran",
    "capture_d_ecran",
    "captura de pantalla",
    "captura_de_pantalla",
    "schermafbeelding",
    "istantanea schermo",
    "zrzut ekranu",
    "снимок экрана",
    "скриншот",
    "スクリーンショット",
    "屏幕截图",
    "截屏",
    "스크린샷",
];
const SCREENSHOT_DIR: &[&str] = &[
    "screenshots",
    "screenshot",
    "ekran goruntuleri",
    "ekran goruntusu",
    "bildschirmfotos",
    "captures d'ecran",
    "capturas de pantalla",
    "скриншоты",
    "スクリーンショット",
];

/// file name / folder looks like a screenshot (many languages)
pub fn is_screenshot_name(name: &str, dir: &str) -> bool {
    let n = fold(name);
    if SCREENSHOT_NAME.iter().any(|p| n.contains(p)) {
        return true;
    }
    dir.split('/')
        .filter(|s| !s.is_empty())
        .map(fold)
        .any(|s| SCREENSHOT_DIR.contains(&s.as_str()))
}

fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// `/\bword\b/` (ASCII word boundaries, like a JS regex without the u flag)
fn has_word(hay: &str, word: &str) -> bool {
    let h = hay.as_bytes();
    let mut from = 0;
    while let Some(p) = hay[from..].find(word) {
        let s = from + p;
        let e = s + word.len();
        let before = s == 0 || !is_word_byte(h[s - 1]);
        let after = e == h.len() || !is_word_byte(h[e]);
        if before && after {
            return true;
        }
        from = s + 1;
        while !hay.is_char_boundary(from) {
            from += 1;
        }
    }
    false
}

/// MEDIA_FLAG bitmask of one item.
pub fn classify(i: &ClassifyInput) -> u32 {
    let mut f = 0;
    let ext = i.ext.to_lowercase();
    let w = i.width.unwrap_or(0.0);
    let h = i.height.unwrap_or(0.0);
    let has_camera = i.make.as_deref().is_some_and(|s| !s.is_empty())
        || i.model.as_deref().is_some_and(|s| !s.is_empty());
    let image = i.kind == "image";
    let dir = i.dir.as_deref().unwrap_or("");
    let lname = i.name.to_ascii_lowercase();

    if image {
        if is_screenshot_name(&i.name, dir) {
            f |= media_flag::SCREENSHOT;
        } else if ext == "png" && !has_camera && w > 0.0 && h > 0.0 {
            let a = w.max(h) / w.min(h);
            if (1.7..=2.45).contains(&a) && w.min(h) >= 640.0 {
                f |= media_flag::SCREENSHOT;
            }
        }
    }

    if image || i.kind == "video" {
        let lens = format!(
            "{} {}",
            i.lens.as_deref().unwrap_or(""),
            i.model.as_deref().unwrap_or("")
        )
        .to_lowercase();
        if has_word(&lens, "front") || lname.contains("selfie") {
            f |= media_flag::SELFIE;
        }
    }

    if image {
        if i.gpano == Some(true) {
            f |= media_flag::PANORAMA;
        } else if w > 0.0 && h > 0.0 && (has_camera || lname.contains("pano")) {
            let a = (w / h).max(h / w);
            if a >= 2.3 {
                f |= media_flag::PANORAMA;
            }
        }
        if ext == "gif"
            || (matches!(ext.as_str(), "webp" | "png" | "apng" | "avif")
                && i.pages.unwrap_or(1.0) > 1.0)
        {
            f |= media_flag::ANIMATED;
        }
    }
    if f & media_flag::SCREENSHOT != 0 {
        f &= !media_flag::PANORAMA;
    }
    f
}

// ───────────────────────── Camera names ─────────────────────────

/// `/^word\b/i` with ASCII case folding and word boundary
fn starts_word(m: &str, word: &str) -> bool {
    let b = m.as_bytes();
    b.len() >= word.len()
        && b[..word.len()].eq_ignore_ascii_case(word.as_bytes())
        && (b.len() == word.len() || !is_word_byte(b[word.len()]))
}

const MAKER_ALIASES: &[(&[&str], &str)] = &[
    (&["nikon"], "Nikon"),
    (&["canon"], "Canon"),
    (&["sony"], "Sony"),
    (&["samsung"], "Samsung"),
    (&["apple"], "Apple"),
    (&["olympus"], "Olympus"),
    (&["om digital solutions"], "OM System"),
    (&["fujifilm", "fuji photo film"], "Fujifilm"),
    (&["panasonic"], "Panasonic"),
    (&["pentax", "ricoh imaging"], "Pentax"),
    (&["ricoh"], "Ricoh"),
    (&["leica"], "Leica"),
    (&["hmd global"], "Nokia"),
    (&["huawei"], "Huawei"),
    (&["xiaomi"], "Xiaomi"),
    (&["oneplus"], "OnePlus"),
    (&["google"], "Google"),
    (&["lg electronics"], "LG"),
    (&["motorola"], "Motorola"),
    (&["gopro"], "GoPro"),
    (&["dji"], "DJI"),
    (&["htc"], "HTC"),
    (&["oppo"], "OPPO"),
    (&["vivo"], "vivo"),
    (&["realme"], "realme"),
    (&["asus"], "ASUS"),
    (&["zte"], "ZTE"),
    (&["tcl"], "TCL"),
];

const JS_SPACE_CLASS: &str = r"\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}";

/// ASCII-only case-insensitive version of a pattern (JS `/i` without the u flag never folds non-ASCII
/// letters onto ASCII ones, e.g. 'ſ' does not match 's')
fn ascii_ci(p: &str) -> String {
    p.chars()
        .map(|c| {
            if c.is_ascii_alphabetic() {
                format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
            } else {
                c.to_string()
            }
        })
        .collect()
}

static SUFFIXES: LazyLock<Regex> = LazyLock::new(|| {
    let alts = ascii_ci(
        r"(corporation|corp\.?|co\.?,?#*ltd\.?|company|ltd\.?|inc\.?|imaging|electronics|optical|digital|camera|gmbh|ag|s\.?a\.?)",
    );
    Regex::new(&format!(
        "[{JS_SPACE_CLASS},]+{}$",
        alts.replace("#", &format!("[{JS_SPACE_CLASS}]"))
    ))
    .expect("valid regex")
});

fn title_case(s: &str) -> String {
    // only touch SHOUTING names; keep "iPhone", "OnePlus", "EOS R5" as they are
    let shouting = {
        let b = s.as_bytes();
        let mut run = 0;
        let mut ok = false;
        for c in b {
            if c.is_ascii_uppercase() {
                run += 1;
                if run >= 3 {
                    ok = true;
                    break;
                }
            } else {
                run = 0;
            }
        }
        ok
    };
    if s != s.to_uppercase() || !shouting {
        return s.to_string();
    }
    let lower = s.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut prev: Option<char> = None;
    for c in lower.chars() {
        let at_word = prev.is_none_or(|p| js::is_space(p) || p == '-' || p == '/');
        if at_word && c.is_ascii_lowercase() {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c);
        }
        prev = Some(c);
    }
    out
}

fn strip_nul(s: &str) -> String {
    s.replace('\0', "")
}

/// Normalise an EXIF Make ("NIKON CORPORATION" → "Nikon").
pub fn normalize_make(make: Option<&str>) -> Option<String> {
    let raw = strip_nul(make.unwrap_or(""));
    let m0 = js::trim(js::slice16_to(js::trim(&raw), 80));
    if m0.is_empty() {
        return None;
    }
    let lm = m0.to_ascii_lowercase();
    for (prefixes, name) in MAKER_ALIASES {
        if prefixes.iter().any(|p| starts_word(m0, p)) {
            return Some((*name).to_string());
        }
    }
    if lm == "lge" {
        return Some("LG".into());
    }
    let mut m = m0.to_string();
    for _ in 0..3 {
        m = js::trim(&SUFFIXES.replace(&m, "")).to_string();
    }
    if m.is_empty() {
        None
    } else {
        Some(title_case(&m))
    }
}

/// Normalised camera.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Camera {
    /// make
    pub make: Option<String>,
    /// model
    pub model: Option<String>,
    /// [`camera_key`]
    pub key: Option<String>,
}

/// Normalise EXIF Make/Model: "NIKON CORPORATION" + "NIKON D750" → Nikon / D750.
pub fn normalize_camera(make: Option<&str>, model: Option<&str>) -> Camera {
    let mk = normalize_make(make);
    let mut md = js::trim(&strip_nul(model.unwrap_or(""))).to_string();
    if !md.is_empty() {
        let raw_make = js::trim(&strip_nul(make.unwrap_or(""))).to_string();
        let first_word = raw_make
            .split(|c: char| c == ',' || js::is_space(c))
            .next()
            .unwrap_or("")
            .to_string();
        let cands = [raw_make.clone(), mk.clone().unwrap_or_default(), first_word];
        for p in cands.iter().filter(|x| js::len16(x) >= 2) {
            if md.to_lowercase().starts_with(&p.to_lowercase()) && js::len16(&md) > js::len16(p) {
                md = js::slice16_from(&md, js::len16(p))
                    .trim_start_matches(|c: char| js::is_space(c) || c == '-' || c == '_')
                    .to_string();
                break;
            }
        }
    }
    if md.is_empty() {
        return Camera {
            make: mk,
            model: None,
            key: None,
        };
    }
    let key = camera_key(mk.as_deref(), &md);
    Camera {
        make: mk,
        model: Some(md),
        key: Some(key),
    }
}

/// normalised "make|model" key
pub fn camera_key(make: Option<&str>, model: &str) -> String {
    format!(
        "{}|{}",
        make.unwrap_or("").to_lowercase(),
        model.to_lowercase()
    )
}

// ───────────────────────── Messaging apps ─────────────────────────

static MSG_NAME: LazyLock<[Regex; 4]> = LazyLock::new(|| {
    [
        Regex::new(r"^(img|vid|aud|ptt|stk)-[0-9]{8}-wa[0-9]{3,}").expect("re"),
        Regex::new(r"^(photo|video)_[0-9]{4}-[0-9]{2}-[0-9]{2}_[0-9]{2}-[0-9]{2}-[0-9]{2}")
            .expect("re"),
        Regex::new(r"^signal-[0-9]{4}-[0-9]{2}-[0-9]{2}").expect("re"),
        Regex::new(r"^received_[0-9]{6,}").expect("re"),
    ]
});
static MSG_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|/)(whatsapp|telegram|signal|messenger)( |/|$)").expect("re"));

/// Files that came through messaging apps (WhatsApp, Telegram, Signal, Messenger …).
pub fn is_messaging_name(name: &str, dir: &str) -> bool {
    let n = name.to_lowercase();
    if MSG_NAME[0].is_match(&n)
        || n.starts_with("whatsapp ")
        || MSG_NAME[1].is_match(&n)
        || MSG_NAME[2].is_match(&n)
        || MSG_NAME[3].is_match(&n)
    {
        return true;
    }
    let d = dir.to_lowercase();
    MSG_DIR.is_match(&d) || d.contains("whatsapp images") || d.contains("telegram images")
}

/// SQL twin of [`is_messaging_name`] (columns name, dir).
pub const MESSAGING_SQL: &str = "(lower(name) GLOB '[iva][mit][gdo]-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]-wa[0-9]*' OR lower(name) LIKE 'whatsapp %'
  OR lower(name) GLOB '[pv][hi][od][te]*_[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]_*' OR lower(name) LIKE 'signal-%' OR lower(name) GLOB 'received_[0-9]*'
  OR lower(dir) LIKE '%whatsapp%' OR lower(dir) LIKE '%telegram%')";
