//! The collage document (features/collage/model.ts, store.ts): canvas ratio, output size,
//! spacing, background, the nine slots with their framing, and the text layers on top.

use crate::editor::doc::TextLayer;

/// Design space: the long side of the collage is always this many units; the export scales it.
pub const DESIGN_LONG: f64 = 2000.0;

/// Most cells a layout has.
pub const MAX_CELLS: usize = 9;

/// Canvas ratios: key, width / height.
pub const ASPECTS: [(&str, f64); 6] = [
    ("1:1", 1.0),
    ("4:5", 4.0 / 5.0),
    ("9:16", 9.0 / 16.0),
    ("16:9", 16.0 / 9.0),
    ("3:2", 3.0 / 2.0),
    ("a4", std::f64::consts::FRAC_1_SQRT_2),
];

/// Output long sides (px).
pub const RESOLUTIONS: [u32; 3] = [2400, 3000, 4000];

pub type Rgb = [u8; 3];

pub struct GradientPreset {
    pub id: &'static str,
    /// CSS degrees (0 = towards the top, clockwise)
    pub angle: f64,
    pub stops: &'static [(f64, Rgb)],
}

pub const GRADIENTS: [GradientPreset; 8] = [
    GradientPreset { id: "aurora", angle: 135.0, stops: &[(0.0, [0x8b, 0x5c, 0xf6]), (0.55, [0xec, 0x48, 0x99]), (1.0, [0xf9, 0x73, 0x16])] },
    GradientPreset { id: "ocean", angle: 160.0, stops: &[(0.0, [0x0e, 0xa5, 0xe9]), (1.0, [0x1e, 0x3a, 0x8a])] },
    GradientPreset { id: "peach", angle: 120.0, stops: &[(0.0, [0xfd, 0xe6, 0x8a]), (1.0, [0xfb, 0x71, 0x85])] },
    GradientPreset { id: "mint", angle: 140.0, stops: &[(0.0, [0xd9, 0xf9, 0x9d]), (1.0, [0x14, 0xb8, 0xa6])] },
    GradientPreset { id: "night", angle: 180.0, stops: &[(0.0, [0x1e, 0x1b, 0x4b]), (1.0, [0x02, 0x06, 0x17])] },
    GradientPreset { id: "sand", angle: 150.0, stops: &[(0.0, [0xfe, 0xf3, 0xc7]), (1.0, [0xd6, 0xb4, 0x8a])] },
    GradientPreset { id: "berry", angle: 135.0, stops: &[(0.0, [0x4c, 0x1d, 0x95]), (1.0, [0xbe, 0x18, 0x5d])] },
    GradientPreset { id: "mist", angle: 180.0, stops: &[(0.0, [0xf8, 0xfa, 0xfc]), (1.0, [0xcb, 0xd5, 0xe1])] },
];

pub fn gradient(id: &str) -> &'static GradientPreset {
    GRADIENTS.iter().find(|g| g.id == id).unwrap_or(&GRADIENTS[0])
}

#[derive(Debug, Clone, PartialEq)]
pub enum Background {
    Solid(Rgb),
    Gradient(&'static str),
    /// blur radius in design units
    Blur(f64),
}

/// One cell's photo and its framing inside the cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub id: Option<String>,
    /// ≥ 1
    pub zoom: f64,
    /// −1…1 pan (1 = show the left / top edge)
    pub px: f64,
    pub py: f64,
}

impl Slot {
    pub fn new(id: Option<String>) -> Slot {
        Slot { id, zoom: 1.0, px: 0.0, py: 0.0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollageDoc {
    pub count: usize,
    pub template: &'static str,
    pub seed: u32,
    pub aspect: &'static str,
    pub long_side: u32,
    pub gap: f64,
    pub padding: f64,
    pub radius: f64,
    pub bg: Background,
    /// always MAX_CELLS long; the first `count` are shown
    pub slots: Vec<Slot>,
    /// design units
    pub layers: Vec<TextLayer>,
}

pub fn aspect_ratio(key: &str) -> f64 {
    ASPECTS.iter().find(|a| a.0 == key).map_or(1.0, |a| a.1)
}

pub fn aspect_key(key: &str) -> Option<&'static str> {
    ASPECTS.iter().find(|a| a.0 == key).map(|a| a.0)
}

/// The canvas in design units.
pub fn design_size(aspect: &str) -> (f64, f64) {
    let r = aspect_ratio(aspect);
    if r >= 1.0 {
        (DESIGN_LONG, (DESIGN_LONG / r).round())
    } else {
        ((DESIGN_LONG * r).round(), DESIGN_LONG)
    }
}

/// The output size in pixels.
pub fn output_size(d: &CollageDoc) -> (u32, u32) {
    let (w, h) = design_size(d.aspect);
    let k = d.long_side as f64 / DESIGN_LONG;
    ((w * k).round() as u32, (h * k).round() as u32)
}

impl CollageDoc {
    pub fn new(ids: &[String]) -> CollageDoc {
        let n = if ids.is_empty() { 4 } else { ids.len() };
        CollageDoc {
            count: n.clamp(2, MAX_CELLS),
            template: "grid",
            seed: 7,
            aspect: "4:5",
            long_side: 3000,
            gap: 24.0,
            padding: 24.0,
            radius: 12.0,
            bg: Background::Solid([0xff, 0xff, 0xff]),
            slots: (0..MAX_CELLS).map(|i| Slot::new(ids.get(i).cloned())).collect(),
            layers: Vec::new(),
        }
    }

    pub fn filled(&self) -> bool {
        self.slots[..self.count].iter().any(|s| s.id.is_some())
    }

    pub fn used(&self, id: &str) -> bool {
        self.slots[..self.count].iter().any(|s| s.id.as_deref() == Some(id))
    }

    /// Put a photo into cell `i`; a photo already shown elsewhere trades places with it.
    pub fn place(&self, i: usize, id: &str) -> CollageDoc {
        let mut d = self.clone();
        if i >= MAX_CELLS {
            return d;
        }
        if let Some(prev) = d.slots[..d.count].iter().position(|s| s.id.as_deref() == Some(id)) {
            if prev != i {
                d.slots[prev] = d.slots[i].clone();
            }
        }
        d.slots[i] = Slot::new(Some(id.to_string()));
        d
    }

    /// The pool's tap: into the selected cell, else the first empty one, else the first.
    pub fn place_from_pool(&self, selected: Option<usize>, id: &str) -> CollageDoc {
        let target = selected
            .filter(|c| *c < self.count)
            .or_else(|| self.slots[..self.count].iter().position(|s| s.id.is_none()))
            .unwrap_or(0);
        self.place(target, id)
    }

    pub fn swap(&self, a: usize, b: usize) -> CollageDoc {
        let mut d = self.clone();
        if a < MAX_CELLS && b < MAX_CELLS {
            d.slots.swap(a, b);
        }
        d
    }

    /// New photos (`first`: the pool was empty) go into empty cells first, then the layout
    /// grows to show the rest.
    pub fn add_photos(&self, fresh: &[String], first: bool) -> CollageDoc {
        let mut d = self.clone();
        let mut queue = fresh.iter();
        if first {
            for s in d.slots.iter_mut() {
                match queue.next() {
                    Some(id) => *s = Slot::new(Some(id.clone())),
                    None => break,
                }
            }
            d.count = fresh.len().clamp(2, MAX_CELLS);
            return d;
        }
        let mut rest: Vec<&String> = Vec::new();
        for s in d.slots[..d.count].iter_mut() {
            if s.id.is_none() {
                match queue.next() {
                    Some(id) => *s = Slot::new(Some(id.clone())),
                    None => break,
                }
            }
        }
        rest.extend(queue);
        for id in rest {
            if d.count >= MAX_CELLS {
                break;
            }
            d.slots[d.count] = Slot::new(Some(id.clone()));
            d.count += 1;
        }
        d
    }

    pub fn remove_photo(&self, id: &str) -> CollageDoc {
        let mut d = self.clone();
        for s in d.slots.iter_mut() {
            if s.id.as_deref() == Some(id) {
                *s = Slot::new(None);
            }
        }
        d
    }

    /// A new canvas ratio; text keeps its place proportionally.
    pub fn with_aspect(&self, aspect: &'static str) -> CollageDoc {
        let (aw, ah) = design_size(self.aspect);
        let (bw, bh) = design_size(aspect);
        let mut d = self.clone();
        d.aspect = aspect;
        for l in d.layers.iter_mut() {
            l.x = l.x / aw * bw;
            l.y = l.y / ah * bh;
        }
        d
    }

    /// The photo the blurred background uses: the first shown, else the pool's first.
    pub fn first_photo<'a>(&'a self, pool_first: Option<&'a str>) -> Option<&'a str> {
        self.slots[..self.count].iter().find_map(|s| s.id.as_deref()).or(pool_first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("p{i}")).collect()
    }

    #[test]
    fn design_size_keeps_the_long_side() {
        assert_eq!(design_size("4:5"), (1600.0, 2000.0));
        assert_eq!(design_size("16:9"), (2000.0, 1125.0));
        assert_eq!(design_size("a4"), (1414.0, 2000.0));
        assert_eq!(design_size("nope"), (2000.0, 2000.0));
        let d = CollageDoc::new(&[]);
        assert_eq!(output_size(&d), (2400, 3000));
    }

    #[test]
    fn new_doc_clamps_the_cell_count() {
        assert_eq!(CollageDoc::new(&[]).count, 4);
        assert_eq!(CollageDoc::new(&ids(1)).count, 2);
        assert_eq!(CollageDoc::new(&ids(12)).count, 9);
        assert_eq!(CollageDoc::new(&ids(3)).slots[2].id.as_deref(), Some("p2"));
        assert_eq!(CollageDoc::new(&ids(3)).slots.len(), MAX_CELLS);
    }

    #[test]
    fn placing_a_used_photo_swaps() {
        let d = CollageDoc::new(&ids(3));
        let n = d.place(0, "p2");
        assert_eq!(n.slots[0].id.as_deref(), Some("p2"));
        assert_eq!(n.slots[2].id.as_deref(), Some("p0"));
        let e = d.remove_photo("p1");
        assert!(e.slots[1].id.is_none());
        assert_eq!(e.place_from_pool(None, "x").slots[1].id.as_deref(), Some("x"));
        assert_eq!(d.place_from_pool(Some(2), "x").slots[2].id.as_deref(), Some("x"));
        assert_eq!(d.place_from_pool(None, "x").slots[0].id.as_deref(), Some("x"));
        let s = d.swap(0, 1);
        assert_eq!(s.slots[0].id.as_deref(), Some("p1"));
    }

    #[test]
    fn added_photos_fill_gaps_then_grow() {
        let d = CollageDoc::new(&[]).add_photos(&ids(3), true);
        assert_eq!(d.count, 3);
        let d = d.remove_photo("p1");
        let more: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let n = d.add_photos(&more, false);
        assert_eq!(n.slots[1].id.as_deref(), Some("a"));
        assert_eq!(n.count, 5);
        assert_eq!(n.slots[4].id.as_deref(), Some("c"));
        let full = CollageDoc::new(&ids(9)).add_photos(&more, false);
        assert_eq!(full.count, 9);
    }

    #[test]
    fn aspect_change_moves_text_proportionally() {
        let mut d = CollageDoc::new(&[]);
        d.layers.push(crate::editor::text::new_layer(1, "Hi".into(), false, crate::editor::doc::Rect { x: 0.0, y: 0.0, w: 1600.0, h: 2000.0 }, 0));
        let n = d.with_aspect("16:9");
        assert!((n.layers[0].x - 1000.0).abs() < 1e-9);
        assert!((n.layers[0].y - 562.5).abs() < 1e-9);
    }
}
