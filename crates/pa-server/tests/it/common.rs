//! Shared helpers of the integration tests (the vitest helpers + sharp / yazl / ffmpeg fixtures).

#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, Response};
use bytes::Bytes;
use http_body_util::BodyExt;
use pa_server::{RootSpec, Server, ServerOptions};
use serde_json::Value;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A temporary directory removed on drop.
pub struct Tmp(pub PathBuf);

static N: AtomicUsize = AtomicUsize::new(0);

impl Tmp {
    pub fn new(tag: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "peek-it-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("tmp dir");
        Tmp(p)
    }
    pub fn join(&self, p: impl AsRef<Path>) -> PathBuf {
        self.0.join(p)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `{ dataDir, roots: [{ path: lib, name }], port: 0, host: '127.0.0.1', mdns: false }`
pub fn opts(tmp: &Tmp, lib: &Path, name: &str) -> ServerOptions {
    let mut o = ServerOptions::new(tmp.join("data"));
    o.port = 0;
    o.host = "127.0.0.1".into();
    o.mdns = false;
    o.roots = vec![RootSpec {
        path: lib.to_string_lossy().into_owned(),
        name: Some(name.into()),
        split: false,
    }];
    o.security = Some(Default::default());
    o
}

pub async fn start(tmp: &Tmp, lib: &Path) -> Server {
    Server::start(opts(tmp, lib, "Lib"))
        .await
        .expect("server start")
}

/// An injected response.
pub struct R {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Bytes,
}

impl R {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not json ({e}): {}", String::from_utf8_lossy(&self.body)))
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn h(&self, k: &str) -> Option<&str> {
        self.headers.get(k).map(String::as_str)
    }
}

pub async fn collect(r: Response<Body>) -> R {
    let status = r.status().as_u16();
    let headers = r
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let body = r
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes())
        .unwrap_or_default();
    R {
        status,
        headers,
        body,
    }
}

/// Request builder for `inject`.
pub struct Inj<'a> {
    srv: &'a Server,
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<(Vec<u8>, String)>,
    ip: Option<IpAddr>,
    lan: bool,
}

pub fn inj<'a>(srv: &'a Server, method: &str, url: &str) -> Inj<'a> {
    Inj {
        srv,
        method: method.into(),
        url: url.into(),
        headers: Vec::new(),
        body: None,
        ip: None,
        lan: false,
    }
}

impl<'a> Inj<'a> {
    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn json(mut self, v: Value) -> Self {
        self.body = Some((v.to_string().into_bytes(), "application/json".into()));
        self
    }
    pub fn bytes(mut self, b: impl Into<Vec<u8>>) -> Self {
        self.body = Some((b.into(), "application/octet-stream".into()));
        self
    }
    pub fn ip(mut self, ip: &str) -> Self {
        self.ip = Some(ip.parse().expect("ip"));
        self
    }
    pub fn lan(mut self) -> Self {
        self.lan = true;
        self
    }
    pub async fn send(self) -> R {
        let mut b = Request::builder()
            .method(self.method.as_str())
            .uri(&self.url);
        let has_host = self
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("host"));
        if !has_host {
            b = b.header("host", "localhost:80");
        }
        for (k, v) in &self.headers {
            b = b.header(k.as_str(), v.as_str());
        }
        let req = match self.body {
            Some((data, ct)) => {
                let len = data.len();
                b.header("content-type", ct)
                    .header("content-length", len.to_string())
                    .body(Body::from(data))
                    .expect("request")
            }
            None => b.body(Body::empty()).expect("request"),
        };
        let mut conn = pa_server::http::ConnInfo::default();
        if let Some(ip) = self.ip {
            conn.ip = ip;
        }
        conn.lan = self.lan;
        collect(self.srv.inject_from(req, conn).await).await
    }
}

pub async fn get(srv: &Server, url: &str) -> R {
    inj(srv, "GET", url).send().await
}

/// GET that must answer 200 → JSON
pub async fn get_ok(srv: &Server, url: &str) -> Value {
    let r = get(srv, url).await;
    assert_eq!(r.status, 200, "{url}: {} {}", r.status, r.text());
    r.json()
}

pub async fn send(srv: &Server, method: &str, url: &str, body: Value) -> R {
    inj(srv, method, url).json(body).send().await
}

/// send that must not fail (status < 400) → JSON
pub async fn send_ok(srv: &Server, method: &str, url: &str, body: Value) -> Value {
    let r = send(srv, method, url, body).await;
    assert!(r.status < 400, "{method} {url} → {} {}", r.status, r.text());
    r.json()
}

pub async fn put(srv: &Server, url: &str, body: impl Into<Vec<u8>>) -> R {
    inj(srv, "PUT", url).bytes(body).send().await
}

pub async fn sleep(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// items of a browse result
pub fn items(v: &Value) -> Vec<Value> {
    v["items"].as_array().cloned().unwrap_or_default()
}

pub fn names(v: &Value) -> Vec<String> {
    items(v)
        .iter()
        .map(|i| i["name"].as_str().unwrap_or("").to_string())
        .collect()
}

pub fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        x => x.to_string(),
    }
}

// ───────────────────────── fixtures ─────────────────────────

/// EXIF tags of a generated JPEG (sharp's `withExif`)
#[derive(Default, Clone)]
pub struct Exif {
    pub date: Option<String>,
    pub offset: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub gps: Option<(f64, f64)>,
    /// GPS UTC instant (ms)
    pub gps_utc: Option<i64>,
    /// an XMP packet (APP1 "http://ns.adobe.com/xap/1.0/")
    pub xmp: Option<String>,
}

struct Ifd(Vec<(u16, u16, u32, Vec<u8>)>);

fn ascii(s: &str) -> (u16, u32, Vec<u8>) {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    (2, v.len() as u32, v)
}

fn rationals(r: &[(u32, u32)]) -> (u16, u32, Vec<u8>) {
    let mut v = Vec::new();
    for (a, b) in r {
        v.extend_from_slice(&a.to_le_bytes());
        v.extend_from_slice(&b.to_le_bytes());
    }
    (5, r.len() as u32, v)
}

fn long(x: u32) -> (u16, u32, Vec<u8>) {
    (4, 1, x.to_le_bytes().to_vec())
}

impl Ifd {
    fn add(&mut self, tag: u16, v: (u16, u32, Vec<u8>)) {
        self.0.push((tag, v.0, v.1, v.2));
    }
    fn size(&self) -> usize {
        let data: usize = self
            .0
            .iter()
            .map(|e| {
                if e.3.len() > 4 {
                    e.3.len() + e.3.len() % 2
                } else {
                    0
                }
            })
            .sum();
        2 + self.0.len() * 12 + 4 + data
    }
    fn write(&mut self, out: &mut Vec<u8>, start: usize) {
        self.0.sort_by_key(|e| e.0);
        let mut data = Vec::new();
        let data_start = start + 2 + self.0.len() * 12 + 4;
        out.extend_from_slice(&(self.0.len() as u16).to_le_bytes());
        for (tag, typ, count, val) in &self.0 {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            if val.len() <= 4 {
                let mut v = val.clone();
                v.resize(4, 0);
                out.extend_from_slice(&v);
            } else {
                out.extend_from_slice(&((data_start + data.len()) as u32).to_le_bytes());
                data.extend_from_slice(val);
                if data.len() % 2 == 1 {
                    data.push(0);
                }
            }
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&data);
    }
}

fn dms(v: f64) -> Vec<(u32, u32)> {
    let a = v.abs();
    let d = a.floor();
    let m = ((a - d) * 60.0).floor();
    let s = (((a - d) * 60.0 - m) * 60.0 * 100.0).round();
    vec![(d as u32, 1), (m as u32, 1), (s as u32, 100)]
}

/// TIFF block (little endian) of the tags
pub fn tiff(e: &Exif) -> Vec<u8> {
    let mut ifd0 = Ifd(Vec::new());
    let mut exif = Ifd(Vec::new());
    let mut gps = Ifd(Vec::new());
    if let Some(m) = &e.make {
        ifd0.add(0x010f, ascii(m));
    }
    if let Some(m) = &e.model {
        ifd0.add(0x0110, ascii(m));
    }
    if let Some(d) = &e.date {
        exif.add(0x9003, ascii(d));
    }
    if let Some(o) = &e.offset {
        exif.add(0x9011, ascii(o));
    }
    if let Some(l) = &e.lens {
        exif.add(0xa434, ascii(l));
    }
    if let Some((lat, lon)) = e.gps {
        gps.add(0x0001, ascii(if lat >= 0.0 { "N" } else { "S" }));
        gps.add(0x0002, rationals(&dms(lat)));
        gps.add(0x0003, ascii(if lon >= 0.0 { "E" } else { "W" }));
        gps.add(0x0004, rationals(&dms(lon)));
        if let Some(u) = e.gps_utc {
            let p = pa_core::js::utc_parts(u);
            gps.add(
                0x0007,
                rationals(&[
                    (p.hour as u32, 1),
                    (p.minute as u32, 1),
                    (p.second as u32, 1),
                ]),
            );
            gps.add(
                0x001d,
                ascii(&format!("{:04}:{:02}:{:02}", p.year, p.month0 + 1, p.day)),
            );
        }
    }
    // pointers (sizes are known up front: a LONG fits inline)
    let has_exif = !exif.0.is_empty();
    let has_gps = !gps.0.is_empty();
    if has_exif {
        ifd0.add(0x8769, long(0));
    }
    if has_gps {
        ifd0.add(0x8825, long(0));
    }
    let ifd0_size = ifd0.size();
    let exif_at = 8 + ifd0_size;
    let gps_at = exif_at + if has_exif { exif.size() } else { 0 };
    for e in ifd0.0.iter_mut() {
        if e.0 == 0x8769 {
            e.3 = (exif_at as u32).to_le_bytes().to_vec();
        }
        if e.0 == 0x8825 {
            e.3 = (gps_at as u32).to_le_bytes().to_vec();
        }
    }
    let mut out = b"II*\0".to_vec();
    out.extend_from_slice(&8u32.to_le_bytes());
    ifd0.write(&mut out, 8);
    if has_exif {
        exif.write(&mut out, exif_at);
    }
    if has_gps {
        gps.write(&mut out, gps_at);
    }
    out
}

fn rgb_of(hex: &str) -> [u8; 3] {
    let h = hex.trim_start_matches('#');
    let h = if h.len() == 3 {
        h.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        h.to_string()
    };
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
    [p(0), p(2), p(4)]
}

/// a solid JPEG (+ EXIF)
pub fn jpeg_bytes(w: u32, h: u32, color: &str, exif: &Exif, quality: u8) -> Vec<u8> {
    let c = rgb_of(color);
    let img = image::RgbImage::from_pixel(w, h, image::Rgb(c));
    encode_with_meta(&img, exif, quality)
}

/// distinct content per seed (only real copies are duplicates) — the TS test's raw buffer
pub fn seeded_jpeg_bytes(w: u32, h: u32, color: &str, seed: u64, exif: &Exif) -> Vec<u8> {
    let c = rgb_of(color);
    let mut i = 0u64;
    let img = image::RgbImage::from_fn(w, h, |_, _| {
        let j = ((i.wrapping_mul(2654435761).wrapping_add(seed * 97)) & 0xffff_ffff) % 9;
        i += 1;
        image::Rgb([
            (c[0] as u64 + j).min(255) as u8,
            (c[1] as u64 + j).min(255) as u8,
            (c[2] as u64 + j).min(255) as u8,
        ])
    });
    encode_with_meta(&img, exif, 80)
}

fn encode_with_meta(img: &image::RgbImage, exif: &Exif, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(img)
        .expect("jpeg");
    let mut segs: Vec<Vec<u8>> = Vec::new();
    let t = tiff(exif);
    if t.len() > 8 + 2 + 4 {
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&t);
        segs.push(app1);
    }
    if let Some(x) = &exif.xmp {
        let mut app1 = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        app1.extend_from_slice(x.as_bytes());
        segs.push(app1);
    }
    let mut res = Vec::with_capacity(out.len() + 1024);
    res.extend_from_slice(&out[..2]);
    for a in segs {
        res.extend_from_slice(&[0xff, 0xe1]);
        res.extend_from_slice(&((a.len() + 2) as u16).to_be_bytes());
        res.extend_from_slice(&a);
    }
    res.extend_from_slice(&out[2..]);
    res
}

pub fn jpeg(file: &Path, w: u32, h: u32, color: &str, exif: &Exif) {
    std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    std::fs::write(file, jpeg_bytes(w, h, color, exif, 80)).expect("write jpeg");
}

/// sharp `{ IFD2: { DateTimeOriginal } }` shortcut
pub fn jpeg_dated(file: &Path, w: u32, h: u32, color: &str, date: Option<&str>) {
    jpeg(
        file,
        w,
        h,
        color,
        &Exif {
            date: date.map(String::from),
            ..Default::default()
        },
    );
}

pub fn png(file: &Path, w: u32, h: u32, color: &str) {
    std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    let c = rgb_of(color);
    image::RgbImage::from_pixel(w, h, image::Rgb(c))
        .save_with_format(file, image::ImageFormat::Png)
        .expect("png");
}

pub fn png_bytes(w: u32, h: u32, color: &str) -> Vec<u8> {
    let c = rgb_of(color);
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(w, h, image::Rgb(c))
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("png");
    out.into_inner()
}

/// a noisy JPEG (well above a minimum size)
pub fn noise_jpeg(file: &Path, w: u32, h: u32) {
    std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    let mut i = 0u64;
    let img = image::RgbImage::from_fn(w, h, |_, _| {
        let mut px = [0u8; 3];
        for c in px.iter_mut() {
            *c = ((i * 7919) % 251) as u8;
            i += 1;
        }
        image::Rgb(px)
    });
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
        .encode_image(&img)
        .expect("jpeg");
    std::fs::write(file, out).expect("write");
}

pub fn set_mtime(file: &Path, ms: i64) {
    let t = std::time::UNIX_EPOCH + Duration::from_millis(ms as u64);
    let f = std::fs::File::options()
        .write(true)
        .open(file)
        .expect("open");
    f.set_times(std::fs::FileTimes::new().set_modified(t).set_accessed(t))
        .expect("utimes");
}

pub fn ffmpeg(args: &[&str]) {
    let st = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .expect("ffmpeg");
    assert!(st.success(), "ffmpeg {args:?}");
}

/// yazl `addBuffer` zip (deflate off: stored entries, data descriptors, UTF-8 names)
pub fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let off = out.len() as u32;
        let crc = crc32fast::hash(data);
        let n = name.as_bytes();
        let mut h = Vec::new();
        h.extend_from_slice(&0x04034b50u32.to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes());
        h.extend_from_slice(&0x0800u16.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(n.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(n);
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
        let mut c = Vec::new();
        c.extend_from_slice(&0x02014b50u32.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes());
        c.extend_from_slice(&0x0800u16.to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes());
        c.extend_from_slice(&0u32.to_le_bytes());
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(&(n.len() as u16).to_le_bytes());
        c.extend_from_slice(&[0u8; 12]);
        c.extend_from_slice(&off.to_le_bytes());
        c.extend_from_slice(n);
        central.extend_from_slice(&c);
    }
    let cd_off = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// image dimensions of encoded bytes
pub fn dims(b: &[u8]) -> (u32, u32) {
    let img = image::load_from_memory(b).expect("decodable image");
    (img.width(), img.height())
}

/// `Date.parse('<iso>Z')`
pub fn l(iso: &str) -> i64 {
    // YYYY-MM-DDTHH:MM:SS
    let n = |a: usize, b: usize| iso[a..b].parse::<i64>().expect("num");
    pa_core::js::date_utc(
        n(0, 4),
        n(5, 7) - 1,
        n(8, 10),
        n(11, 13),
        n(14, 16),
        n(17, 19),
        0,
    )
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn count(srv: &Server, sql: &str) -> i64 {
    srv.ctx.db.int(sql, vec![]).expect("sql").unwrap_or(0)
}
