//! Decoder smoke tests on generated images (orientation, alpha, animation, formats) and, when the
//! repository fixtures / camera samples are present, on real photos (HEIC included).

use image::{DynamicImage, Rgb, RgbImage};
use pa_media::render::{self, ThumbQuality};
use pa_media::{decode_bytes, image_info, DecodeOptions, Format};
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// 40×20 image, red top-left quadrant, encoded as JPEG with an EXIF orientation
fn oriented_jpeg(o: u16) -> Vec<u8> {
    let img = RgbImage::from_fn(40, 20, |x, y| {
        if x < 20 && y < 10 {
            Rgb([255, 0, 0])
        } else {
            Rgb([0, 0, 255])
        }
    });
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 95)
        .encode_image(&img)
        .unwrap();
    // TIFF block with IFD0 { 0x0112 = o }
    let mut t = b"MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01".to_vec();
    t.extend_from_slice(&o.to_be_bytes());
    t.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mut app1 = vec![0xff, 0xe1];
    app1.extend_from_slice(&((t.len() + 8) as u16).to_be_bytes());
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&t);
    let mut out = jpg[..2].to_vec();
    out.extend(app1);
    out.extend_from_slice(&jpg[2..]);
    out
}

fn red_corner(img: &DynamicImage) -> (bool, bool, bool, bool) {
    let i = img.to_rgb8();
    let (w, h) = (i.width() - 1, i.height() - 1);
    let red = |x, y| i.get_pixel(x, y)[0] > 200 && i.get_pixel(x, y)[2] < 60;
    (red(0, 0), red(w, 0), red(0, h), red(w, h))
}

#[test]
fn applies_every_exif_orientation() {
    // where the red quadrant ends up (tl, tr, bl, br) and the output size
    let want = [
        (1, (true, false, false, false), (40, 20)),
        (2, (false, true, false, false), (40, 20)),
        (3, (false, false, false, true), (40, 20)),
        (4, (false, false, true, false), (40, 20)),
        (5, (true, false, false, false), (20, 40)),
        (6, (false, true, false, false), (20, 40)),
        (7, (false, false, false, true), (20, 40)),
        (8, (false, false, true, false), (20, 40)),
    ];
    for (o, corner, size) in want {
        let data = oriented_jpeg(o);
        let info = image_info(&data, None).unwrap();
        assert_eq!(
            (info.format, info.width, info.height, info.orientation),
            (Format::Jpeg, 40, 20, Some(o as u8))
        );
        assert_eq!(info.display_size(), size);
        let img = decode_bytes(&data, DecodeOptions::default()).unwrap();
        assert_eq!((img.width(), img.height()), size, "orientation {o}");
        assert_eq!(red_corner(&img), corner, "orientation {o}");
    }
}

#[test]
fn formats_alpha_and_animation() {
    let rgba = image::RgbaImage::from_fn(30, 20, |x, _| {
        image::Rgba([200, 10, 10, if x < 15 { 255 } else { 0 }])
    });
    for f in [
        image::ImageFormat::Png,
        image::ImageFormat::Tiff,
        image::ImageFormat::Bmp,
        image::ImageFormat::Ico,
    ] {
        let mut b = Vec::new();
        DynamicImage::ImageRgba8(rgba.clone())
            .write_to(&mut Cursor::new(&mut b), f)
            .unwrap();
        let img = decode_bytes(&b, DecodeOptions::default()).unwrap();
        assert_eq!((img.width(), img.height()), (30, 20), "{f:?}");
        assert!(img.color().has_alpha(), "{f:?}");
        let r = render::render_base(&img, ThumbQuality::Normal).unwrap();
        assert_eq!((r.width, r.height), (30, 20));
    }
    // animated GIF: first frame, pages = frame count
    let mut gif = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut gif);
        for c in [10u8, 120, 240] {
            enc.encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                16,
                8,
                image::Rgba([c, c, c, 255]),
            )))
            .unwrap();
        }
    }
    let info = image_info(&gif, None).unwrap();
    assert_eq!(
        (info.format, info.pages, info.width),
        (Format::Gif, Some(3), 16)
    );
    let img = decode_bytes(&gif, DecodeOptions::default()).unwrap();
    assert!(img.to_rgb8().get_pixel(3, 3)[0] < 30);
    // SVG renders at the box
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200"><rect width="300" height="200" fill="#3366cc"/></svg>"##;
    assert_eq!(image_info(svg, Some("svg")).unwrap().width, 300);
    let img = decode_bytes(svg, DecodeOptions::default().ext("svg").fit(150, 150)).unwrap();
    assert_eq!((img.width(), img.height()), (150, 100));
    assert!(decode_bytes(b"definitely not an image", DecodeOptions::default()).is_err());
    // truncated JPEG still decodes (sharp failOn: 'none')
    let big = RgbImage::from_fn(400, 300, |x, y| {
        Rgb([
            (x * 7 % 256) as u8,
            (y * 13 % 256) as u8,
            ((x ^ y) % 256) as u8,
        ])
    });
    let mut j = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut j, 90)
        .encode_image(&big)
        .unwrap();
    let img = decode_bytes(&j[..j.len() * 2 / 3], DecodeOptions::default()).unwrap();
    assert_eq!((img.width(), img.height()), (400, 300));
}

fn samples() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for d in [
        "/tmp/claude-0/-home-user/0be13da1-d5a1-51d7-96bc-92ae259ef06d/scratchpad/samples",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/library/Fotoğraflar/Aile"
        ),
    ] {
        if let Ok(rd) = std::fs::read_dir(d) {
            v.extend(rd.filter_map(|e| e.ok()).map(|e| e.path()));
        }
    }
    v
}

#[test]
fn real_photos_when_available() {
    for p in samples() {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !["jpg", "heic"].contains(&ext.as_str()) {
            continue;
        }
        let info = pa_media::image_info_path(&p).unwrap();
        let img = render::decode_for_base(&p).unwrap();
        let b = render::render_base(&img, ThumbQuality::Normal).unwrap();
        let (dw, dh) = info.display_size();
        let expect = pa_media::resize::fit_inside(dw, dh, 640, 640, true);
        assert!(
            (b.width as i32 - expect.0 as i32).abs() <= 1
                && (b.height as i32 - expect.1 as i32).abs() <= 1,
            "{p:?}"
        );
        assert!(b.thumbhash.is_some());
        if ext == "heic" {
            assert_eq!(
                (info.format, info.width, info.height),
                (Format::Heif, 4032, 3024)
            );
        }
    }
}

/// Explicit HEIC / AVIF check for cross builds (the Windows exe under wine):
/// `PA_HEIF_SAMPLES="Z:\…\iphone.heic;Z:\…\sample.avif" wine decode-….exe heif_samples_decode`
#[test]
fn heif_samples_decode() {
    let Ok(list) = std::env::var("PA_HEIF_SAMPLES") else {
        return;
    };
    for p in list.split(';').filter(|p| !p.is_empty()) {
        let p = Path::new(p);
        let info = pa_media::image_info_path(p).unwrap_or_else(|e| panic!("{p:?}: {e}"));
        let img = render::decode_for_base(p).unwrap_or_else(|e| panic!("{p:?}: {e}"));
        let b = render::render_base(&img, ThumbQuality::Normal).unwrap();
        assert_eq!(info.format, Format::Heif, "{p:?}");
        assert!(
            img.width() > 100 && b.width > 0 && b.thumbhash.is_some(),
            "{p:?}"
        );
        eprintln!(
            "decoded {p:?}: {}x{} → thumb {}x{}",
            img.width(),
            img.height(),
            b.width,
            b.height
        );
    }
}
