//! Slideshow model (mirrors packages/web/src/features/slideshow/show.ts): play order, durations,
//! per-slide transitions, Ken Burns moves, the show length and the export spec. Pure: the
//! builder, the player and the export all read it, so the preview matches the exported video.

use pa_core::{DirTarget, MediaItem, MediaKind, SlideTransition, SlideshowExportSpec, SlideshowItem, TimeRange};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Aspect {
    #[serde(rename = "16:9")]
    Wide,
    #[serde(rename = "9:16")]
    Tall,
    #[serde(rename = "1:1")]
    Square,
    #[serde(rename = "4:5")]
    Portrait,
}

impl Aspect {
    pub fn ratio(self) -> f64 {
        match self {
            Aspect::Wide => 16.0 / 9.0,
            Aspect::Tall => 9.0 / 16.0,
            Aspect::Square => 1.0,
            Aspect::Portrait => 4.0 / 5.0,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Aspect::Wide => "16:9",
            Aspect::Tall => "9:16",
            Aspect::Square => "1:1",
            Aspect::Portrait => "4:5",
        }
    }
    pub fn parse(s: &str) -> Option<Aspect> {
        [Aspect::Wide, Aspect::Tall, Aspect::Square, Aspect::Portrait]
            .into_iter()
            .find(|a| a.label() == s)
    }
}

/// One slide of the builder's list.
#[derive(Debug, Clone, PartialEq)]
pub struct SlideItem {
    /// unique within the list (the same photo may appear twice)
    pub key: String,
    pub media: MediaItem,
    /// seconds (images); None = the global duration
    pub duration: Option<f64>,
    /// video items: the part to play
    pub trim: Option<TimeRange>,
}

/// Show settings (persisted with the UI preferences, like 1.x's `peek.slideshow`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SlideSettings {
    pub duration: f64,
    pub transition: SlideTransition,
    pub transition_duration: f64,
    pub ken_burns: bool,
    pub music: Option<MediaItem>,
    /// short side of the export: 720 / 1080 / 2160
    pub res: u32,
    pub aspect: Aspect,
    pub background: String,
    pub shuffle: bool,
    #[serde(rename = "loop")]
    pub looping: bool,
    pub captions: bool,
}

impl Default for SlideSettings {
    fn default() -> Self {
        SlideSettings {
            duration: 4.0,
            transition: SlideTransition::Fade,
            transition_duration: 1.0,
            ken_burns: true,
            music: None,
            res: 1080,
            aspect: Aspect::Wide,
            background: "#000000".into(),
            shuffle: false,
            looping: false,
            captions: false,
        }
    }
}

/// Concrete transitions the player draws (`random` / `kenburns` resolve to one of these).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowTransition {
    None,
    Fade,
    Slide,
    Zoom,
    Blur,
    Flip,
    Circle,
}

pub const RANDOM_POOL: [ShowTransition; 6] = [
    ShowTransition::Fade,
    ShowTransition::Slide,
    ShowTransition::Zoom,
    ShowTransition::Blur,
    ShowTransition::Flip,
    ShowTransition::Circle,
];

/// One slide as played.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaySlide {
    pub key: String,
    pub media: MediaItem,
    /// seconds on screen (incl. the transition into the next one)
    pub dur: f64,
    pub trim: Option<TimeRange>,
    /// transition used when this slide comes in
    pub transition: ShowTransition,
    /// Ken Burns move index, None = no move
    pub kb: Option<usize>,
}

/// A small deterministic generator (shuffle / random transitions); 0..1.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }
    pub fn from_time() -> Rng {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Rng::new(t)
    }
    pub fn next(&mut self) -> f64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn even(n: f64) -> i64 {
    ((n / 2.0).round() * 2.0) as i64
}

/// Export size: `res` is the short side.
pub fn output_dims(res: u32, aspect: Aspect) -> (i64, i64) {
    let r = aspect.ratio();
    let res = res as f64;
    if r >= 1.0 {
        (even(res * r), res as i64)
    } else {
        (res as i64, even(res / r))
    }
}

pub fn item_duration(it: &SlideItem, s: &SlideSettings) -> f64 {
    if it.media.kind == MediaKind::Video {
        if let Some(t) = &it.trim {
            return (t.end - t.start).max(0.2);
        }
        return it.duration.or(it.media.duration).unwrap_or(s.duration).max(0.2);
    }
    it.duration.unwrap_or(s.duration)
}

pub fn shuffled<T: Clone>(v: &[T], rng: &mut Rng) -> Vec<T> {
    let mut a = v.to_vec();
    for i in (1..a.len()).rev() {
        let j = ((rng.next() * (i + 1) as f64) as usize).min(i);
        a.swap(i, j);
    }
    a
}

pub fn ken_burns_on(s: &SlideSettings) -> bool {
    s.ken_burns || s.transition == SlideTransition::Kenburns
}

/// Resolve the play order, durations, per-slide transitions (random picks) and Ken Burns moves.
pub fn playlist(items: &[SlideItem], s: &SlideSettings, rng: &mut Rng) -> Vec<PlaySlide> {
    let order = if s.shuffle { shuffled(items, rng) } else { items.to_vec() };
    let kb = ken_burns_on(s);
    let mut mv = 0usize;
    let mut prev: Option<ShowTransition> = None;
    order
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let transition = if i == 0 {
                ShowTransition::Fade
            } else {
                match s.transition {
                    SlideTransition::None => ShowTransition::None,
                    SlideTransition::Random => {
                        let pool: Vec<ShowTransition> =
                            RANDOM_POOL.iter().copied().filter(|x| Some(*x) != prev).collect();
                        pool[((rng.next() * pool.len() as f64) as usize) % pool.len()]
                    }
                    SlideTransition::Kenburns | SlideTransition::Fade => ShowTransition::Fade,
                    SlideTransition::Slide => ShowTransition::Slide,
                    SlideTransition::Zoom => ShowTransition::Zoom,
                    SlideTransition::Blur => ShowTransition::Blur,
                    SlideTransition::Flip => ShowTransition::Flip,
                    SlideTransition::Circle => ShowTransition::Circle,
                }
            };
            if i > 0 {
                prev = Some(transition);
            }
            let is_image = it.media.kind == MediaKind::Image;
            PlaySlide {
                key: it.key.clone(),
                media: it.media.clone(),
                dur: item_duration(it, s),
                trim: if it.media.kind == MediaKind::Video { it.trim } else { None },
                transition,
                kb: if kb && is_image {
                    let m = mv % KB_MOVES.len();
                    mv += 1;
                    Some(m)
                } else {
                    None
                },
            }
        })
        .collect()
}

/// Length of the show: slides overlap by the transition duration (same as the export).
pub fn show_length(slides: &[PlaySlide], td: f64) -> f64 {
    if slides.is_empty() {
        return 0.0;
    }
    let total: f64 = slides.iter().map(|x| x.dur).sum();
    let cuts: f64 = slides
        .windows(2)
        .map(|w| td.min(w[0].dur / 2.0).min(w[1].dur / 2.0))
        .sum();
    total - cuts
}

/// Effective cross-fade length into slide i (clamped like the server does).
pub fn cut_length(slides: &[PlaySlide], i: usize, td: f64) -> f64 {
    if i == 0 || i >= slides.len() || slides[i].transition == ShowTransition::None {
        return 0.0;
    }
    td.min(slides[i - 1].dur / 2.0).min(slides[i].dur / 2.0)
}

pub fn build_export_spec(
    items: &[SlideItem],
    s: &SlideSettings,
    target: Option<DirTarget>,
    name: &str,
    rng: &mut Rng,
) -> SlideshowExportSpec {
    let order = if s.shuffle { shuffled(items, rng) } else { items.to_vec() };
    let (w, h) = output_dims(s.res, s.aspect);
    SlideshowExportSpec {
        items: order
            .iter()
            .map(|it| {
                let video = it.media.kind == MediaKind::Video;
                SlideshowItem {
                    id: it.media.id.clone(),
                    duration: match (video, &it.trim) {
                        (false, _) | (true, None) => it.duration,
                        (true, Some(_)) => None,
                    },
                    trim: if video { it.trim } else { None },
                }
            })
            .collect(),
        duration: s.duration,
        transition: s.transition,
        transition_duration: if s.transition == SlideTransition::None {
            0.0
        } else {
            s.transition_duration
        },
        ken_burns: ken_burns_on(s),
        audio_id: Some(s.music.as_ref().map(|m| m.id.clone())),
        width: w,
        height: h,
        fps: 30,
        background: s.background.clone(),
        target,
        name: Some(name.trim().to_string()).filter(|n| !n.is_empty()),
    }
}

/// Ken Burns moves (scale from → to, x / y drift in % from → to), like the exporter.
pub const KB_MOVES: [([f64; 2], [f64; 2], [f64; 2]); 4] = [
    ([1.0, 1.15], [2.2, -2.2], [0.0, 0.0]),
    ([1.15, 1.0], [-2.2, 2.2], [1.2, -1.2]),
    ([1.0, 1.15], [0.0, 0.0], [-2.2, 2.2]),
    ([1.15, 1.0], [2.2, -2.2], [2.2, -2.2]),
];

/// (scale, x %, y %) of move `m` at progress `p`.
pub fn kb_at(m: usize, p: f64) -> (f64, f64, f64) {
    let (s, x, y) = KB_MOVES[m % KB_MOVES.len()];
    let t = p.clamp(0.0, 1.0);
    let l = |a: [f64; 2]| a[0] + (a[1] - a[0]) * t;
    (l(s), l(x), l(y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, kind: MediaKind, dur: Option<f64>) -> SlideItem {
        let media = MediaItem {
            id: id.into(),
            folder_id: "1".into(),
            dir: String::new(),
            name: format!("{id}.jpg"),
            ext: "jpg".into(),
            kind,
            size: 1,
            mtime: 0,
            width: None,
            height: None,
            duration: dur,
            taken_at: 0,
            local_at: 0,
            flags: 0,
            thumbhash: None,
            favorite: false,
            playback: pa_core::Playback::Direct,
        };
        SlideItem { key: id.into(), media, duration: None, trim: None }
    }

    #[test]
    fn dims() {
        assert_eq!(output_dims(1080, Aspect::Wide), (1920, 1080));
        assert_eq!(output_dims(1080, Aspect::Tall), (1080, 1920));
        assert_eq!(output_dims(720, Aspect::Portrait), (720, 900));
        assert_eq!(output_dims(2160, Aspect::Square), (2160, 2160));
    }

    #[test]
    fn durations_and_length() {
        let s = SlideSettings::default();
        let mut v = item("v", MediaKind::Video, Some(9.0));
        assert_eq!(item_duration(&v, &s), 9.0);
        v.trim = Some(TimeRange { start: 2.0, end: 5.0 });
        assert_eq!(item_duration(&v, &s), 3.0);
        let items = vec![item("a", MediaKind::Image, None), item("b", MediaKind::Image, None), v];
        let pl = playlist(&items, &s, &mut Rng::new(7));
        assert_eq!(pl.iter().map(|p| p.dur).collect::<Vec<_>>(), vec![4.0, 4.0, 3.0]);
        // 11 s minus a 1 s overlap at each of the two cuts
        assert!((show_length(&pl, 1.0) - 9.0).abs() < 1e-9);
        assert_eq!(cut_length(&pl, 0, 1.0), 0.0);
        assert_eq!(cut_length(&pl, 2, 4.0), 1.5);
    }

    #[test]
    fn transitions_and_ken_burns() {
        let items: Vec<SlideItem> = (0..8).map(|i| item(&i.to_string(), MediaKind::Image, None)).collect();
        let mut s = SlideSettings { transition: SlideTransition::Random, ..Default::default() };
        let pl = playlist(&items, &s, &mut Rng::new(42));
        assert_eq!(pl[0].transition, ShowTransition::Fade);
        for w in pl[1..].windows(2) {
            assert_ne!(w[0].transition, w[1].transition, "random never repeats the previous pick");
        }
        assert_eq!(pl.iter().map(|p| p.kb).collect::<Vec<_>>()[..5], [Some(0), Some(1), Some(2), Some(3), Some(0)]);
        s.transition = SlideTransition::None;
        s.ken_burns = false;
        let pl = playlist(&items, &s, &mut Rng::new(1));
        assert!(pl[1..].iter().all(|p| p.transition == ShowTransition::None && p.kb.is_none()));
        assert_eq!(kb_at(1, 0.5), (1.075, 0.0, 0.0));
    }

    #[test]
    fn export_spec() {
        let mut v = item("v", MediaKind::Video, Some(9.0));
        v.trim = Some(TimeRange { start: 1.0, end: 2.0 });
        let mut a = item("a", MediaKind::Image, None);
        a.duration = Some(6.0);
        let s = SlideSettings { transition: SlideTransition::None, ..Default::default() };
        let spec = build_export_spec(&[a, v], &s, None, "  Tatil ", &mut Rng::new(3));
        assert_eq!(spec.items[0].duration, Some(6.0));
        assert_eq!(spec.items[1].duration, None);
        assert_eq!(spec.items[1].trim, Some(TimeRange { start: 1.0, end: 2.0 }));
        assert_eq!(spec.transition_duration, 0.0);
        assert_eq!((spec.width, spec.height), (1920, 1080));
        assert_eq!(spec.name.as_deref(), Some("Tatil"));
        assert_eq!(spec.audio_id, Some(None));
    }
}
