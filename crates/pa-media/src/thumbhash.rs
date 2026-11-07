//! ThumbHash, ported line by line from the `thumbhash` npm package (0.1.1, Evan Wallace) that the
//! TypeScript server uses. Everything is computed in `f64` with JavaScript's `Math.round`, so the same
//! RGBA input gives byte-identical hashes (verified against the npm package, see tests/parity).

// a line-by-line port: index loops kept to mirror the JS reference
#![allow(clippy::needless_range_loop)]

use std::f64::consts::PI;

/// fdlibm-derived `cos` (musl / FreeBSD msun via the `libm` crate), the same algorithm V8 uses for
/// `Math.cos` — the platform libm may differ in the last bit, which shows on perfectly flat images
#[inline]
fn cos(x: f64) -> f64 {
    libm::cos(x)
}

/// `Math.round` (rounds .5 towards +∞, unlike `f64::round`)
#[inline]
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// Encode an RGBA image (not premultiplied, at most 100×100) to a ThumbHash.
pub fn rgba_to_thumb_hash(w: usize, h: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    if w > 100 || h > 100 {
        return Err(format!("{w}x{h} doesn't fit in 100x100"));
    }
    if rgba.len() < w * h * 4 {
        return Err("not enough pixel data".into());
    }
    if w == 0 || h == 0 {
        return Err("empty image".into());
    }
    let n = w * h;
    let (mut avg_r, mut avg_g, mut avg_b, mut avg_a) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        let j = i * 4;
        let alpha = rgba[j + 3] as f64 / 255.0;
        avg_r += alpha / 255.0 * rgba[j] as f64;
        avg_g += alpha / 255.0 * rgba[j + 1] as f64;
        avg_b += alpha / 255.0 * rgba[j + 2] as f64;
        avg_a += alpha;
    }
    if avg_a != 0.0 {
        avg_r /= avg_a;
        avg_g /= avg_a;
        avg_b /= avg_a;
    }

    let has_alpha = avg_a < n as f64;
    let l_limit = if has_alpha { 5.0 } else { 7.0 };
    let wf = w as f64;
    let hf = h as f64;
    let lx = 1f64.max(js_round(l_limit * wf / wf.max(hf))) as usize;
    let ly = 1f64.max(js_round(l_limit * hf / wf.max(hf))) as usize;
    let mut l = vec![0f64; n];
    let mut p = vec![0f64; n];
    let mut q = vec![0f64; n];
    let mut a = vec![0f64; n];
    for i in 0..n {
        let j = i * 4;
        let alpha = rgba[j + 3] as f64 / 255.0;
        let r = avg_r * (1.0 - alpha) + alpha / 255.0 * rgba[j] as f64;
        let g = avg_g * (1.0 - alpha) + alpha / 255.0 * rgba[j + 1] as f64;
        let b = avg_b * (1.0 - alpha) + alpha / 255.0 * rgba[j + 2] as f64;
        l[i] = (r + g + b) / 3.0;
        p[i] = (r + g) / 2.0 - b;
        q[i] = r - g;
        a[i] = alpha;
    }

    let encode_channel = |channel: &[f64], nx: usize, ny: usize| -> (f64, Vec<f64>, f64) {
        let mut dc = 0.0;
        let mut ac = Vec::new();
        let mut scale = 0.0f64;
        let mut fx = vec![0f64; w];
        for cy in 0..ny {
            let mut cx = 0;
            while cx * ny < nx * (ny - cy) {
                let mut f = 0.0;
                for (x, v) in fx.iter_mut().enumerate() {
                    *v = cos(PI / wf * cx as f64 * (x as f64 + 0.5));
                }
                for y in 0..h {
                    let fy = cos(PI / hf * cy as f64 * (y as f64 + 0.5));
                    for x in 0..w {
                        f += channel[x + y * w] * fx[x] * fy;
                    }
                }
                f /= (w * h) as f64;
                if cx != 0 || cy != 0 {
                    ac.push(f);
                    scale = scale.max(f.abs());
                } else {
                    dc = f;
                }
                cx += 1;
            }
        }
        if scale != 0.0 {
            for v in ac.iter_mut() {
                *v = 0.5 + 0.5 / scale * *v;
            }
        }
        (dc, ac, scale)
    };
    let (l_dc, l_ac, l_scale) = encode_channel(&l, lx.max(3), ly.max(3));
    let (p_dc, p_ac, p_scale) = encode_channel(&p, 3, 3);
    let (q_dc, q_ac, q_scale) = encode_channel(&q, 3, 3);
    let alpha = if has_alpha {
        Some(encode_channel(&a, 5, 5))
    } else {
        None
    };

    let is_landscape = w > h;
    let header24 = (js_round(63.0 * l_dc) as i64)
        | ((js_round(31.5 + 31.5 * p_dc) as i64) << 6)
        | ((js_round(31.5 + 31.5 * q_dc) as i64) << 12)
        | ((js_round(31.0 * l_scale) as i64) << 18)
        | ((has_alpha as i64) << 23);
    let header16 = (if is_landscape { ly } else { lx }) as i64
        | ((js_round(63.0 * p_scale) as i64) << 3)
        | ((js_round(63.0 * q_scale) as i64) << 9)
        | ((is_landscape as i64) << 15);
    let mut hash: Vec<u8> = vec![
        (header24 & 255) as u8,
        ((header24 >> 8) & 255) as u8,
        (header24 >> 16) as u8,
        (header16 & 255) as u8,
        (header16 >> 8) as u8,
    ];
    let ac_start = if has_alpha { 6 } else { 5 };
    let mut ac_index = 0usize;
    if let Some((a_dc, _, a_scale)) = &alpha {
        hash.push((js_round(15.0 * a_dc) as i64 | ((js_round(15.0 * a_scale) as i64) << 4)) as u8);
    }
    let mut channels: Vec<&Vec<f64>> = vec![&l_ac, &p_ac, &q_ac];
    if let Some((_, a_ac, _)) = &alpha {
        channels.push(a_ac);
    }
    for ac in channels {
        for f in ac {
            let idx = ac_start + (ac_index >> 1);
            if hash.len() <= idx {
                hash.resize(idx + 1, 0);
            }
            hash[idx] |= ((js_round(15.0 * f) as i64) << ((ac_index & 1) << 2)) as u8;
            ac_index += 1;
        }
    }
    Ok(hash)
}

/// Decoded placeholder (RGBA, not premultiplied), at most 32×32.
pub struct ThumbHashImage {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

fn byte(hash: &[u8], i: usize) -> i64 {
    hash.get(i).copied().unwrap_or(0) as i64
}

/// Approximate aspect ratio (width / height) of the original image.
pub fn thumb_hash_to_approximate_aspect_ratio(hash: &[u8]) -> f64 {
    let header = byte(hash, 3);
    let has_alpha = byte(hash, 2) & 0x80 != 0;
    let is_landscape = byte(hash, 4) & 0x80 != 0;
    let lx = if is_landscape {
        if has_alpha {
            5
        } else {
            7
        }
    } else {
        header & 7
    };
    let ly = if is_landscape {
        header & 7
    } else if has_alpha {
        5
    } else {
        7
    };
    lx as f64 / ly as f64
}

/// Average colour (RGBA, each 0..=1) stored in a ThumbHash.
pub fn thumb_hash_to_average_rgba(hash: &[u8]) -> [f64; 4] {
    let header = byte(hash, 0) | (byte(hash, 1) << 8) | (byte(hash, 2) << 16);
    let l = (header & 63) as f64 / 63.0;
    let p = ((header >> 6) & 63) as f64 / 31.5 - 1.0;
    let q = ((header >> 12) & 63) as f64 / 31.5 - 1.0;
    let has_alpha = header >> 23 != 0;
    let a = if has_alpha {
        (byte(hash, 5) & 15) as f64 / 15.0
    } else {
        1.0
    };
    let b = l - 2.0 / 3.0 * p;
    let r = (3.0 * l - b + q) / 2.0;
    let g = r - q;
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), a]
}

/// Render a ThumbHash back to a small RGBA image (mirrors `thumbHashToRGBA`).
pub fn thumb_hash_to_rgba(hash: &[u8]) -> ThumbHashImage {
    let header24 = byte(hash, 0) | (byte(hash, 1) << 8) | (byte(hash, 2) << 16);
    let header16 = byte(hash, 3) | (byte(hash, 4) << 8);
    let l_dc = (header24 & 63) as f64 / 63.0;
    let p_dc = ((header24 >> 6) & 63) as f64 / 31.5 - 1.0;
    let q_dc = ((header24 >> 12) & 63) as f64 / 31.5 - 1.0;
    let l_scale = ((header24 >> 18) & 31) as f64 / 31.0;
    let has_alpha = header24 >> 23 != 0;
    let p_scale = ((header16 >> 3) & 63) as f64 / 63.0;
    let q_scale = ((header16 >> 9) & 63) as f64 / 63.0;
    let is_landscape = header16 >> 15 != 0;
    let lx = 3.max(if is_landscape {
        if has_alpha {
            5
        } else {
            7
        }
    } else {
        header16 & 7
    }) as usize;
    let ly = 3.max(if is_landscape {
        header16 & 7
    } else if has_alpha {
        5
    } else {
        7
    }) as usize;
    let a_dc = if has_alpha {
        (byte(hash, 5) & 15) as f64 / 15.0
    } else {
        1.0
    };
    let a_scale = (byte(hash, 5) >> 4) as f64 / 15.0;

    let ac_start = if has_alpha { 6 } else { 5 };
    let mut ac_index = 0usize;
    let mut decode_channel = |nx: usize, ny: usize, scale: f64| -> Vec<f64> {
        let mut ac = Vec::new();
        for cy in 0..ny {
            let mut cx = if cy != 0 { 0 } else { 1 };
            while cx * ny < nx * (ny - cy) {
                let v = (byte(hash, ac_start + (ac_index >> 1)) >> ((ac_index & 1) << 2)) & 15;
                ac.push((v as f64 / 7.5 - 1.0) * scale);
                ac_index += 1;
                cx += 1;
            }
        }
        ac
    };
    let l_ac = decode_channel(lx, ly, l_scale);
    let p_ac = decode_channel(3, 3, p_scale * 1.25);
    let q_ac = decode_channel(3, 3, q_scale * 1.25);
    let a_ac = if has_alpha {
        decode_channel(5, 5, a_scale)
    } else {
        Vec::new()
    };

    let ratio = thumb_hash_to_approximate_aspect_ratio(hash);
    let w = js_round(if ratio > 1.0 { 32.0 } else { 32.0 * ratio }) as usize;
    let h = js_round(if ratio > 1.0 { 32.0 / ratio } else { 32.0 }) as usize;
    let mut rgba = vec![0u8; w * h * 4];
    let nfx = lx.max(if has_alpha { 5 } else { 3 });
    let nfy = ly.max(if has_alpha { 5 } else { 3 });
    let mut fx = vec![0f64; nfx];
    let mut fy = vec![0f64; nfy];
    let mut i = 0;
    for y in 0..h {
        for x in 0..w {
            let (mut l, mut p, mut q, mut a) = (l_dc, p_dc, q_dc, a_dc);
            for (cx, v) in fx.iter_mut().enumerate() {
                *v = cos(PI / w as f64 * (x as f64 + 0.5) * cx as f64);
            }
            for (cy, v) in fy.iter_mut().enumerate() {
                *v = cos(PI / h as f64 * (y as f64 + 0.5) * cy as f64);
            }
            let mut j = 0;
            for cy in 0..ly {
                let fy2 = fy[cy] * 2.0;
                let mut cx = if cy != 0 { 0 } else { 1 };
                while cx * ly < lx * (ly - cy) {
                    l += l_ac[j] * fx[cx] * fy2;
                    cx += 1;
                    j += 1;
                }
            }
            let mut j = 0;
            for cy in 0..3 {
                let fy2 = fy[cy] * 2.0;
                let mut cx = if cy != 0 { 0 } else { 1 };
                while cx < 3 - cy {
                    let f = fx[cx] * fy2;
                    p += p_ac[j] * f;
                    q += q_ac[j] * f;
                    cx += 1;
                    j += 1;
                }
            }
            if has_alpha {
                let mut j = 0;
                for cy in 0..5 {
                    let fy2 = fy[cy] * 2.0;
                    let mut cx = if cy != 0 { 0 } else { 1 };
                    while cx < 5 - cy {
                        a += a_ac[j] * fx[cx] * fy2;
                        cx += 1;
                        j += 1;
                    }
                }
            }
            let b = l - 2.0 / 3.0 * p;
            let r = (3.0 * l - b + q) / 2.0;
            let g = r - q;
            // Uint8Array assignment truncates towards zero
            rgba[i] = (255.0 * r.min(1.0)).max(0.0) as u8;
            rgba[i + 1] = (255.0 * g.min(1.0)).max(0.0) as u8;
            rgba[i + 2] = (255.0 * b.min(1.0)).max(0.0) as u8;
            rgba[i + 3] = (255.0 * a.min(1.0)).max(0.0) as u8;
            i += 4;
        }
    }
    ThumbHashImage { w, h, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_average_and_aspect() {
        let (w, h) = (80, 40);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                px[i] = (x * 3) as u8;
                px[i + 1] = 120;
                px[i + 2] = (y * 5) as u8;
                px[i + 3] = 255;
            }
        }
        let hash = rgba_to_thumb_hash(w, h, &px).unwrap();
        assert!(hash.len() >= 5);
        let ratio = thumb_hash_to_approximate_aspect_ratio(&hash);
        assert!((ratio - 2.0).abs() < 0.5, "{ratio}");
        let avg = thumb_hash_to_average_rgba(&hash);
        assert!((avg[1] - 120.0 / 255.0).abs() < 0.05, "{avg:?}");
        let img = thumb_hash_to_rgba(&hash);
        assert_eq!(img.w, 32);
        assert_eq!(img.rgba.len(), img.w * img.h * 4);
        assert!(rgba_to_thumb_hash(101, 10, &px).is_err());
    }

    /// vector produced by the npm package: rgbaToThumbHash(4, 3, …) of a fixed gradient with alpha
    #[test]
    fn matches_npm_vectors() {
        let mut px = Vec::new();
        for i in 0..12u32 {
            px.extend_from_slice(&[
                (i * 20) as u8,
                (255 - i * 15) as u8,
                (i * 7 + 30) as u8,
                if i % 3 == 0 { 128 } else { 255 },
            ]);
        }
        let hash = rgba_to_thumb_hash(4, 3, &px).unwrap();
        assert_eq!(
            hex::encode(&hash),
            include_str!("../tests/data/thumbhash-vector.txt").trim()
        );
    }
}
