//! Thumbnail pipeline: decode off the UI thread (WebP / JPEG / PNG → RGBA), an LRU of ready
//! `slint::Image`s on the UI thread (bounded by decoded bytes), thumbhash placeholders.

use std::num::NonZeroUsize;

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::backend::ThumbData;

/// Decoded RGBA thumbnail (Send: crosses from the decoder thread to the UI thread).
pub type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// Decode a backend thumbnail (blocking: call from a worker thread).
pub fn decode(data: ThumbData) -> Option<Pixels> {
    match data {
        ThumbData::Encoded(bytes) => {
            let img = image::load_from_memory(&bytes).ok()?.into_rgba8();
            let (w, h) = img.dimensions();
            Some(SharedPixelBuffer::clone_from_slice(img.as_raw(), w, h))
        }
        ThumbData::Rgba {
            width,
            height,
            pixels,
        } => {
            if pixels.len() < (width * height * 4) as usize {
                return None;
            }
            Some(SharedPixelBuffer::clone_from_slice(&pixels, width, height))
        }
    }
}

/// Decode a base64 thumbhash to a small RGBA placeholder (≤ 32 px).
pub fn thumbhash_pixels(b64: &str) -> Option<Pixels> {
    use base64::Engine;
    let hash = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let (w, h, rgba) = thumbhash::thumb_hash_to_rgba(&hash).ok()?;
    Some(SharedPixelBuffer::clone_from_slice(
        &rgba, w as u32, h as u32,
    ))
}

/// LRU of decoded thumbnails, keyed by (media id, size), bounded by decoded bytes.
pub struct ThumbCache {
    lru: lru::LruCache<(String, u32), (Image, usize)>,
    bytes: usize,
    max_bytes: usize,
    /// lookups / hits (stats)
    pub lookups: u64,
    /// hits
    pub hits: u64,
}

impl ThumbCache {
    /// `max_bytes` of decoded RGBA
    pub fn new(max_bytes: usize) -> ThumbCache {
        ThumbCache {
            lru: lru::LruCache::unbounded(),
            bytes: 0,
            max_bytes,
            lookups: 0,
            hits: 0,
        }
    }

    /// Cached image of `id` at `size`, or a bigger cached size (never a smaller one).
    pub fn get(&mut self, id: &str, size: u32) -> Option<Image> {
        self.lookups += 1;
        for s in [size, 640, 1600] {
            if s < size {
                continue;
            }
            if let Some((img, _)) = self.lru.get(&(id.to_string(), s)) {
                self.hits += 1;
                return Some(img.clone());
            }
        }
        None
    }

    /// Insert (evicts the least recently used beyond the byte budget).
    pub fn put(&mut self, id: String, size: u32, px: Pixels) -> Image {
        let n = (px.width() * px.height() * 4) as usize;
        let img = Image::from_rgba8(px);
        if let Some((_, old)) = self.lru.put((id, size), (img.clone(), n)) {
            self.bytes -= old;
        }
        self.bytes += n;
        while self.bytes > self.max_bytes && self.lru.len() > 1 {
            if let Some((_, (_, b))) = self.lru.pop_lru() {
                self.bytes -= b;
            }
        }
        img
    }

    /// decoded bytes held
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// entries
    pub fn len(&self) -> usize {
        self.lru.len()
    }

    /// empty?
    pub fn is_empty(&self) -> bool {
        self.lru.is_empty()
    }
}

/// Small LRU of thumbhash placeholders (per media id).
pub struct PlaceholderCache {
    lru: lru::LruCache<String, Image>,
}

impl PlaceholderCache {
    /// `n` entries
    pub fn new(n: usize) -> PlaceholderCache {
        PlaceholderCache {
            lru: lru::LruCache::new(NonZeroUsize::new(n.max(1)).unwrap()),
        }
    }

    /// image for `id`, built from `px` once
    pub fn get_or_insert(&mut self, id: &str, px: &Pixels) -> Image {
        if let Some(i) = self.lru.get(id) {
            return i.clone();
        }
        let img = Image::from_rgba8(px.clone());
        self.lru.put(id.to_string(), img.clone());
        img
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_png_and_thumbhash() {
        let mut png = Vec::new();
        let img = image::RgbaImage::from_pixel(8, 4, image::Rgba([10, 20, 30, 255]));
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let px = decode(ThumbData::Encoded(png.into())).unwrap();
        assert_eq!((px.width(), px.height()), (8, 4));
        assert_eq!(
            px.as_slice()[0],
            Rgba8Pixel {
                r: 10,
                g: 20,
                b: 30,
                a: 255
            }
        );
        assert!(decode(ThumbData::Encoded(bytes::Bytes::from_static(b"nope"))).is_none());
        // a real hash from the fixture library (TS server)
        let th = thumbhash_pixels("ihcCBID9a57tY2qGyDkDFtq/fA==").unwrap();
        assert!(th.width() > 0 && th.width() <= 32 && th.height() <= 32);
        assert!(thumbhash_pixels("!!").is_none());
    }

    #[test]
    fn cache_evicts_by_bytes() {
        let mut c = ThumbCache::new(3 * 100 * 100 * 4);
        for i in 0..5 {
            c.put(format!("{i}"), 320, SharedPixelBuffer::new(100, 100));
        }
        assert_eq!(c.len(), 3);
        assert!(c.get("0", 320).is_none());
        assert!(c.get("4", 320).is_some());
        // a bigger cached size serves a smaller request, not the other way round
        c.put("big".into(), 640, SharedPixelBuffer::new(10, 10));
        assert!(c.get("big", 320).is_some());
        c.put("small".into(), 320, SharedPixelBuffer::new(10, 10));
        assert!(c.get("small", 640).is_none());
    }
}
