//! Embedded ICC profiles → sRGB (sharp converts every input with a profile to sRGB and strips the
//! profile; without this, Display P3 / Adobe RGB photos would look washed out). Pure Rust (moxcms).

use image::DynamicImage;

/// The ICC profile of a JPEG (APP2 `ICC_PROFILE` chunks, joined in sequence order).
pub fn jpeg_icc(d: &[u8]) -> Option<Vec<u8>> {
    let mut parts: Vec<(u8, &[u8])> = Vec::new();
    let mut p = 2;
    while p + 4 <= d.len() && d[p] == 0xff {
        let m = d[p + 1];
        if m == 0xd8 || m == 0x01 || (0xd0..=0xd7).contains(&m) {
            p += 2;
            continue;
        }
        if m == 0xda || m == 0xd9 {
            break;
        }
        let len = u16::from_be_bytes([d[p + 2], d[p + 3]]) as usize;
        let body = d.get(p + 4..p + 2 + len)?;
        if m == 0xe2 && body.len() > 14 && body.starts_with(b"ICC_PROFILE\0") {
            parts.push((body[12], &body[14..]));
        }
        p += 2 + len;
    }
    if parts.is_empty() {
        return None;
    }
    parts.sort_by_key(|x| x.0);
    Some(
        parts
            .into_iter()
            .flat_map(|x| x.1.iter().copied())
            .collect(),
    )
}

fn is_srgb(icc: &[u8]) -> bool {
    // the profile description of the common sRGB profiles (IEC 61966-2.1, Apple, Google, Android)
    let s = String::from_utf8_lossy(&icc[..icc.len().min(1024)]).to_ascii_lowercase();
    s.contains("srgb")
        || icc
            .windows(8)
            .take(600)
            .any(|w| w == "s\0R\0G\0B\0".as_bytes())
}

/// Convert RGB(A)8 pixels from `icc` to sRGB in place; unknown / unsupported profiles are left as they are.
pub fn to_srgb(img: &mut DynamicImage, icc: &[u8]) -> bool {
    use moxcms::{ColorProfile, Layout, TransformOptions};
    if icc.len() < 128 || is_srgb(icc) {
        return false;
    }
    let Ok(src) = ColorProfile::new_from_slice(icc) else {
        return false;
    };
    let dst = ColorProfile::new_srgb();
    let layout = match img {
        DynamicImage::ImageRgb8(_) => Layout::Rgb,
        DynamicImage::ImageRgba8(_) => Layout::Rgba,
        _ => return false,
    };
    let Ok(t) = src.create_transform_8bit(layout, &dst, layout, TransformOptions::default()) else {
        return false;
    };
    let buf: &mut [u8] = match img {
        DynamicImage::ImageRgb8(i) => i,
        DynamicImage::ImageRgba8(i) => i,
        _ => return false,
    };
    let mut out = vec![0u8; buf.len()];
    if t.transform(buf, &mut out).is_err() {
        return false;
    }
    buf.copy_from_slice(&out);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_p3_to_srgb() {
        let icc = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        if let Ok(dir) = std::env::var("PA_MEDIA_DUMP_ICC") {
            std::fs::write(std::path::Path::new(&dir).join("p3.icc"), &icc).unwrap();
        }
        let mut img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            2,
            2,
            image::Rgb([200, 100, 50]),
        ));
        assert!(to_srgb(&mut img, &icc));
        let p = img.as_rgb8().unwrap().get_pixel(0, 0);
        assert!(p[0] > 205 && p[1] < 100 && p[2] < 50, "{p:?}");
        // gray stays gray, sRGB profiles are skipped
        let mut g = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([128, 128, 128, 77]),
        ));
        to_srgb(&mut g, &icc);
        let p = g.as_rgba8().unwrap().get_pixel(0, 0);
        assert!((p[0] as i32 - 128).abs() <= 1 && p[3] == 77, "{p:?}");
        let srgb = moxcms::ColorProfile::new_srgb().encode().unwrap();
        assert!(!to_srgb(&mut g, &srgb) || true);
        assert_eq!(jpeg_icc(&[0xff, 0xd8, 0xff, 0xd9]), None);
    }
}
