//! The text layer property panel shared by the photo and the video editor (components/
//! TextLayerControls): the selected layer as an [`EdText`] and the panel's edits applied to a
//! [`TextLayer`].

use slint::Color;

use super::doc::{Align, Rgba, TextBackground, TextLayer, TextOutline, TextShadow};
use crate::EdText;

pub(crate) const DEFAULT_FAMILY: &str = "Bricolage Grotesque";

pub(crate) fn to_color(c: Rgba) -> Color {
    Color::from_argb_u8(c[3], c[0], c[1], c[2])
}

pub(crate) fn from_color(c: Color) -> Rgba {
    [c.red(), c.green(), c.blue(), c.alpha()]
}

fn nearest(weights: &[u16], w: f32) -> f32 {
    weights.iter().map(|x| *x as f32).min_by(|a, b| (a - w).abs().total_cmp(&(b - w).abs())).unwrap_or(w)
}

/// The weights a family offers and whether it has an italic.
fn family_info(family: &str) -> (Vec<u16>, bool) {
    crate::pages::fonts::picker()
        .into_iter()
        .find(|f| f.0 == family)
        .map(|f| (f.1, f.2))
        .unwrap_or_else(|| (vec![400, 700], true))
}

/// The panel's view of a layer; `max` is the size slider's top (outline and background
/// sliders scale with it).
pub(crate) fn props(l: &TextLayer, max: f64) -> EdText {
    let (weights, italic_ok) = family_info(&l.family);
    let stroke = l.stroke.unwrap_or(TextOutline { color: [0, 0, 0, 255], width: 0.0 });
    let shadow = l.shadow.unwrap_or(TextShadow { color: [0, 0, 0, 128], blur: 0.0, x: 0.0, y: 0.0 });
    let bg = l.background.unwrap_or(TextBackground { color: [0, 0, 0, 140], padding: 0.0, radius: 0.0 });
    EdText {
        id: l.id as i32,
        text: l.text.clone().into(),
        family: l.family.clone().into(),
        weight: l.weight,
        wmin: weights.first().copied().unwrap_or(400) as f32,
        wmax: weights.last().copied().unwrap_or(700) as f32,
        italic: l.italic,
        italic_ok,
        bold: l.weight >= 600.0,
        size: l.size.round() as f32,
        size_max: max as f32,
        color: to_color(l.color),
        align: match l.align {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
        }
        .into(),
        spacing: l.letter_spacing as f32,
        line: l.line_height as f32,
        rotation: l.rotation.round() as f32,
        opacity: (l.opacity * 100.0).round() as f32,
        stroke: l.stroke.is_some(),
        stroke_color: to_color(stroke.color),
        stroke_width: stroke.width as f32,
        stroke_max: (max / 12.0).round().max(20.0) as f32,
        shadow: l.shadow.is_some(),
        shadow_color: to_color(shadow.color),
        shadow_blur: shadow.blur as f32,
        shadow_x: shadow.x as f32,
        shadow_y: shadow.y as f32,
        bg: l.background.is_some(),
        bg_color: to_color(bg.color),
        bg_padding: bg.padding as f32,
        bg_radius: bg.radius as f32,
        bg_max: (max / 3.0).round().max(60.0) as f32,
    }
}

/// A slider of the panel.
pub(crate) fn set_num(l: &mut TextLayer, key: &str, v: f64) {
    match key {
        "size" => l.size = v.max(4.0),
        "weight" => l.weight = nearest(&family_info(&l.family).0, v as f32),
        "spacing" => l.letter_spacing = v,
        "line" => l.line_height = v,
        "rotation" => l.rotation = v,
        "opacity" => l.opacity = (v / 100.0).clamp(0.0, 1.0),
        "stroke.width" => {
            if let Some(o) = l.stroke.as_mut() {
                o.width = v.max(0.0);
            }
        }
        "shadow.blur" | "shadow.x" | "shadow.y" => {
            if let Some(sh) = l.shadow.as_mut() {
                match key {
                    "shadow.blur" => sh.blur = v.max(0.0),
                    "shadow.x" => sh.x = v,
                    _ => sh.y = v,
                }
            }
        }
        "bg.padding" | "bg.radius" => {
            if let Some(b) = l.background.as_mut() {
                if key == "bg.padding" {
                    b.padding = v.max(0.0);
                } else {
                    b.radius = v.max(0.0);
                }
            }
        }
        _ => {}
    }
}

/// Text, font family, alignment.
pub(crate) fn set_str(l: &mut TextLayer, key: &str, v: &str) {
    match key {
        "text" => l.text = v.to_string(),
        "family" => {
            let (weights, italic_ok) = family_info(v);
            l.weight = nearest(&weights, l.weight);
            l.italic &= italic_ok;
            l.family = v.to_string();
        }
        "align" => {
            l.align = match v {
                "left" => Align::Left,
                "right" => Align::Right,
                _ => Align::Center,
            }
        }
        _ => {}
    }
}

/// Shadow and background keep their alpha: the swatches only pick the hue.
pub(crate) fn set_color(l: &mut TextLayer, key: &str, c: Color) {
    let c = from_color(c);
    match key {
        "color" => l.color = c,
        "stroke.color" => {
            if let Some(o) = l.stroke.as_mut() {
                o.color = c;
            }
        }
        "shadow.color" => {
            if let Some(sh) = l.shadow.as_mut() {
                sh.color = [c[0], c[1], c[2], sh.color[3]];
            }
        }
        "bg.color" => {
            if let Some(b) = l.background.as_mut() {
                b.color = [c[0], c[1], c[2], b.color[3]];
            }
        }
        _ => {}
    }
}

/// Outline / shadow / background on or off (with their defaults), bold, italic.
pub(crate) fn toggle(l: &mut TextLayer, key: &str, on: bool) {
    match key {
        "stroke" => {
            l.stroke = on.then(|| TextOutline { color: [0, 0, 0, 255], width: (l.size / 24.0).round().max(2.0) });
        }
        "shadow" => l.shadow = on.then_some(TextShadow { color: [0, 0, 0, 128], blur: 16.0, x: 0.0, y: 4.0 }),
        "bg" => {
            l.background = on.then(|| TextBackground {
                color: [0, 0, 0, 140],
                padding: (l.size * 0.3).round(),
                radius: (l.size * 0.25).round(),
            })
        }
        "italic" => l.italic = on,
        "bold" => {
            let weights = family_info(&l.family).0;
            l.weight = if on {
                weights.iter().find(|w| **w >= 700).or(weights.last()).map_or(700.0, |w| *w as f32)
            } else {
                weights.iter().rev().find(|w| **w <= 500).or(weights.first()).map_or(400.0, |w| *w as f32)
            }
        }
        _ => {}
    }
}
