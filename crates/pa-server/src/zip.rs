//! Zip archives (mirrors `zip.ts` + the yazl streaming of `/api/download`): extraction with
//! zip-slip protection, a size guard and Turkish CP857 / Info-ZIP Unicode names, and a streaming
//! store-only writer (data descriptors, zip64 when needed).

use crate::paths::{normalize_rel, sanitize_name};
use bytes::Bytes;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const CP857_HIGH: &str = concat!(
    "ÇüéâäàåçêëèïîıÄÅ",
    "ÉæÆôöòûùİÖÜø£ØŞş",
    "áíóúñÑĞğ¿®¬½¼¡«»",
    "░▒▓│┤ÁÂÀ©╣║╗╝¢¥┐",
    "└┴┬├─┼ãÃ╚╔╩╦╠═╬¤",
    "ºªÊËÈ€ÍÎÏ┘┌█▄¦Ì▀",
    "ÓßÔÒõÕµ_×ÚÛÙìÿ¯´",
    "\u{ad}±_¾¶§÷¸°¨·¹³²■\u{a0}"
);

fn decode_cp857(raw: &[u8]) -> String {
    let high: Vec<char> = CP857_HIGH.chars().collect();
    raw.iter()
        .map(|&b| {
            if b < 0x80 {
                b as char
            } else {
                high.get((b - 0x80) as usize).copied().unwrap_or('_')
            }
        })
        .collect()
}

/// Name of an entry: Info-ZIP Unicode Path extra field, UTF-8 flag, valid UTF-8, else CP857.
pub fn decode_zip_name(raw: &[u8], flags: u16, extra: &[(u16, Vec<u8>)]) -> String {
    if let Some((_, d)) = extra.iter().find(|(id, _)| *id == 0x7075) {
        if d.len() > 5 && d[0] == 1 {
            return String::from_utf8_lossy(&d[5..]).into_owned();
        }
    }
    if flags & 0x800 != 0 {
        return String::from_utf8_lossy(raw).into_owned();
    }
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => decode_cp857(raw),
    }
}

/// Extraction result.
#[derive(Debug, Default)]
pub struct ExtractResult {
    /// posix rel paths of written files
    pub files: Vec<String>,
    pub bytes: u64,
}

struct CdEntry {
    name: String,
    flags: u16,
    method: u16,
    csize: u64,
    usize_: u64,
    offset: u64,
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn u64le(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or([0; 8]))
}

fn err(m: impl Into<String>) -> std::io::Error {
    std::io::Error::other(m.into())
}

fn read_central(f: &mut std::fs::File) -> std::io::Result<Vec<CdEntry>> {
    let len = f.seek(SeekFrom::End(0))?;
    let tail = len.min(22 + 65_535);
    f.seek(SeekFrom::Start(len - tail))?;
    let mut buf = vec![0u8; tail as usize];
    f.read_exact(&mut buf)?;
    let eocd = (0..buf.len().saturating_sub(21)).rev().find(|&i| u32le(&buf, i) == 0x0605_4b50).ok_or_else(|| err("End of central directory record signature not found. Either not a zip file, or file is truncated."))?;
    let mut count = u16le(&buf, eocd + 10) as u64;
    let mut cd_size = u32le(&buf, eocd + 12) as u64;
    let mut cd_off = u32le(&buf, eocd + 16) as u64;
    if (count == 0xffff || cd_off == 0xffff_ffff || cd_size == 0xffff_ffff)
        && eocd >= 20
        && u32le(&buf, eocd - 20) == 0x0706_4b50
    {
        let z64 = u64le(&buf, eocd - 20 + 8);
        f.seek(SeekFrom::Start(z64))?;
        let mut h = [0u8; 56];
        f.read_exact(&mut h)?;
        if u32le(&h, 0) != 0x0606_4b50 {
            return Err(err(
                "invalid zip64 end of central directory record signature",
            ));
        }
        count = u64le(&h, 32);
        cd_size = u64le(&h, 40);
        cd_off = u64le(&h, 48);
    }
    f.seek(SeekFrom::Start(cd_off))?;
    let mut cd = vec![0u8; cd_size.min(len) as usize];
    f.read_exact(&mut cd)?;
    let mut out = Vec::new();
    let mut i = 0usize;
    for _ in 0..count {
        if i + 46 > cd.len() {
            return Err(err("unexpected EOF"));
        }
        if u32le(&cd, i) != 0x0201_4b50 {
            return Err(err(format!(
                "invalid central directory file header signature: 0x{:x}",
                u32le(&cd, i)
            )));
        }
        let flags = u16le(&cd, i + 8);
        let method = u16le(&cd, i + 10);
        let mut csize = u32le(&cd, i + 20) as u64;
        let mut usize_ = u32le(&cd, i + 24) as u64;
        let nlen = u16le(&cd, i + 28) as usize;
        let elen = u16le(&cd, i + 30) as usize;
        let clen = u16le(&cd, i + 32) as usize;
        let mut offset = u32le(&cd, i + 42) as u64;
        let name_raw = cd
            .get(i + 46..i + 46 + nlen)
            .ok_or_else(|| err("invalid file name"))?
            .to_vec();
        let extra_raw = cd
            .get(i + 46 + nlen..i + 46 + nlen + elen)
            .unwrap_or(&[])
            .to_vec();
        let mut extras = Vec::new();
        let mut j = 0;
        while j + 4 <= extra_raw.len() {
            let id = u16le(&extra_raw, j);
            let sz = u16le(&extra_raw, j + 2) as usize;
            let data = extra_raw.get(j + 4..j + 4 + sz).unwrap_or(&[]).to_vec();
            if id == 0x0001 {
                let mut k = 0;
                if usize_ == 0xffff_ffff && k + 8 <= data.len() {
                    usize_ = u64le(&data, k);
                    k += 8;
                }
                if csize == 0xffff_ffff && k + 8 <= data.len() {
                    csize = u64le(&data, k);
                    k += 8;
                }
                if offset == 0xffff_ffff && k + 8 <= data.len() {
                    offset = u64le(&data, k);
                }
            }
            extras.push((id, data));
            j += 4 + sz;
        }
        out.push(CdEntry {
            name: decode_zip_name(&name_raw, flags, &extras),
            flags,
            method,
            csize,
            usize_,
            offset,
        });
        i += 46 + nlen + elen + clen;
    }
    Ok(out)
}

fn skip_rel(rel: &str) -> bool {
    rel.starts_with("__MACOSX/")
        || rel == ".DS_Store"
        || rel.ends_with("/.DS_Store")
        || rel.eq_ignore_ascii_case("thumbs.db")
        || rel.to_ascii_lowercase().ends_with("/thumbs.db")
}

/// Extract a zip into `dest` (blocking). Entries with absolute or ../ paths are skipped.
pub fn extract_zip(file: &Path, dest: &Path, max_bytes: u64) -> std::io::Result<ExtractResult> {
    let mut f = std::fs::File::open(file)?;
    let entries = read_central(&mut f)?;
    std::fs::create_dir_all(dest)?;
    let root = crate::paths::resolve(dest);
    let mut result = ExtractResult::default();
    for e in entries {
        let name = &e.name;
        let Ok(rel) = normalize_rel(Some(name)) else {
            continue;
        };
        let b = name.as_bytes();
        let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
        if rel.is_empty() || skip_rel(&rel) || drive || name.starts_with('/') {
            continue;
        }
        let rel = rel
            .split('/')
            .map(sanitize_name)
            .collect::<Vec<_>>()
            .join("/");
        let target = rel.split('/').fold(root.clone(), |p, s| p.join(s));
        let target = crate::paths::normalize(&target);
        if !target.starts_with(&root) || target == root {
            continue;
        }
        if name.ends_with('/') {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if result.bytes + e.usize_ > max_bytes {
            return Err(err("Archive too large"));
        }
        if e.flags & 1 != 0 {
            return Err(err(
                "entry is encrypted, and options.decodeFileData !== false",
            ));
        }
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p)?;
        }
        // local header → data
        f.seek(SeekFrom::Start(e.offset))?;
        let mut lh = [0u8; 30];
        f.read_exact(&mut lh)?;
        if u32le(&lh, 0) != 0x0403_4b50 {
            return Err(err(format!(
                "invalid local file header signature: 0x{:x}",
                u32le(&lh, 0)
            )));
        }
        let skip = u16le(&lh, 26) as u64 + u16le(&lh, 28) as u64;
        f.seek(SeekFrom::Current(skip as i64))?;
        let data = (&mut f).take(e.csize);
        let mut reader: Box<dyn Read> = match e.method {
            0 => Box::new(data),
            8 => Box::new(flate2::read::DeflateDecoder::new(data)),
            m => return Err(err(format!("unsupported compression method: {m}"))),
        };
        let mut out = std::fs::File::create(&target)?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut written = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            result.bytes += n as u64;
            if result.bytes > max_bytes || written > e.usize_ + 1024 {
                return Err(err("Archive too large or corrupt"));
            }
            std::io::Write::write_all(&mut out, &buf[..n])?;
        }
        result.files.push(rel);
    }
    Ok(result)
}

/// default size guard (200 GiB)
pub const MAX_EXTRACT: u64 = 200 * 1024 * 1024 * 1024;

// ───────────────────────── writer ─────────────────────────

/// Source of a zip entry.
pub enum ZipSource {
    /// a local file (`addFile`: mode + mtime from stat)
    File(PathBuf),
    /// bytes produced by a stream (phone / MEGA; `addReadStream` with `size`: mode 0o100664, mtime now)
    Stream(
        futures_util::stream::BoxStream<'static, std::io::Result<Bytes>>,
        Option<u64>,
    ),
}

/// One entry of [`zip_stream`].
pub struct ZipEntry {
    pub name: String,
    pub source: ZipSource,
}

/// yazl's `dateToDosDateTime` (local time, clamped to 1980-01-01 … 2107-12-31 23:59:58)
fn dos_time(ms: i64) -> (u16, u16) {
    let tz = crate::db::system_time_zone();
    let min = pa_core::local_to_instant(pa_core::js::date_utc(1980, 0, 1, 0, 0, 0, 0), &tz);
    let max = pa_core::local_to_instant(pa_core::js::date_utc(2107, 11, 31, 23, 59, 58, 0), &tz);
    let ms = ms.clamp(min, max);
    let local = pa_core::instant_to_local(ms, &tz);
    let p = pa_core::js::utc_parts(local);
    let date = (((p.year - 1980) as u16 & 0x7f) << 9)
        | (((p.month0 as u16 + 1) & 0xf) << 5)
        | (p.day as u16 & 0x1f);
    let time =
        ((p.hour as u16 & 0x1f) << 11) | ((p.minute as u16 & 0x3f) << 5) | (p.second as u16 / 2);
    (date, time)
}

/// Stream a store-only zip of `entries`, byte-identical to yazl 3 (`compress: false`): data
/// descriptors, Info-ZIP "UT" timestamps in the central directory, zip64 when needed. An entry
/// failing ends the stream with an error.
pub fn zip_stream(
    entries: Vec<ZipEntry>,
) -> futures_util::stream::BoxStream<'static, std::io::Result<Bytes>> {
    use futures_util::StreamExt;
    let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<Bytes>>(8);
    tokio::spawn(async move {
        let r = write_zip(entries, &tx).await;
        if let Err(e) = r {
            let _ = tx.send(Err(e)).await;
        }
    });
    tokio_stream_from(rx).boxed()
}

fn tokio_stream_from(
    mut rx: tokio::sync::mpsc::Receiver<std::io::Result<Bytes>>,
) -> impl futures_util::Stream<Item = std::io::Result<Bytes>> {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

struct Written {
    name: Vec<u8>,
    crc: u32,
    size: u64,
    offset: u64,
    date: u16,
    time: u16,
    mtime_s: i64,
    mode: u32,
}

const ZIP64_LIMIT: u64 = 0xffff_fffe;

async fn write_zip(
    entries: Vec<ZipEntry>,
    tx: &tokio::sync::mpsc::Sender<std::io::Result<Bytes>>,
) -> std::io::Result<()> {
    use futures_util::StreamExt;
    use tokio::io::AsyncReadExt;
    let send = |b: Vec<u8>| async move {
        tx.send(Ok(Bytes::from(b)))
            .await
            .map_err(|_| err("client gone"))
    };
    // entries are created (and files stat'ed) up front, like the addFile / addReadStream calls
    let mut prepared = Vec::with_capacity(entries.len());
    for e in entries {
        let (mtime, mode, known) = match &e.source {
            ZipSource::File(p) => {
                let md = tokio::fs::metadata(p).await?;
                if !md.is_file() {
                    return Err(err(format!("not a file: {}", p.display())));
                }
                #[cfg(unix)]
                let mode = std::os::unix::fs::PermissionsExt::mode(&md.permissions()) & 0xffff;
                #[cfg(not(unix))]
                let mode = if md.permissions().readonly() {
                    0o100444
                } else {
                    0o100666
                };
                (crate::scanner::mtime_ms(&md), mode, Some(md.len()))
            }
            ZipSource::Stream(_, size) => (crate::events::now_ms(), 0o100664, *size),
        };
        prepared.push((e, mtime, mode, known));
    }
    let mut off: u64 = 0;
    let mut done: Vec<Written> = Vec::new();
    for (e, mtime, mode, known) in prepared {
        let name = e.name.into_bytes();
        let (date, time) = dos_time(mtime);
        let mut lh = Vec::with_capacity(30 + name.len());
        lh.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        lh.extend_from_slice(&20u16.to_le_bytes());
        lh.extend_from_slice(&(0x0808u16).to_le_bytes());
        lh.extend_from_slice(&0u16.to_le_bytes());
        lh.extend_from_slice(&time.to_le_bytes());
        lh.extend_from_slice(&date.to_le_bytes());
        lh.extend_from_slice(&[0u8; 12]);
        lh.extend_from_slice(&(name.len() as u16).to_le_bytes());
        lh.extend_from_slice(&0u16.to_le_bytes());
        lh.extend_from_slice(&name);
        let start = off;
        off += lh.len() as u64;
        send(lh).await?;
        let mut crc = crc32fast::Hasher::new();
        let mut size = 0u64;
        match e.source {
            ZipSource::File(p) => {
                let mut f = tokio::fs::File::open(&p).await?;
                loop {
                    let mut buf = vec![0u8; 64 * 1024];
                    let n = f.read(&mut buf).await?;
                    if n == 0 {
                        break;
                    }
                    buf.truncate(n);
                    crc.update(&buf);
                    size += n as u64;
                    send(buf).await?;
                }
            }
            ZipSource::Stream(mut s, _) => {
                while let Some(c) = s.next().await {
                    let c = c?;
                    crc.update(&c);
                    size += c.len() as u64;
                    tx.send(Ok(c)).await.map_err(|_| err("client gone"))?;
                }
            }
        }
        if known.is_some_and(|k| k != size) {
            return Err(err("file data stream has unexpected number of bytes"));
        }
        off += size;
        let crc = crc.finalize();
        let zip64 = size > ZIP64_LIMIT || start > ZIP64_LIMIT;
        let mut dd = Vec::new();
        dd.extend_from_slice(&0x0807_4b50u32.to_le_bytes());
        dd.extend_from_slice(&crc.to_le_bytes());
        if zip64 {
            dd.extend_from_slice(&size.to_le_bytes());
            dd.extend_from_slice(&size.to_le_bytes());
        } else {
            dd.extend_from_slice(&(size as u32).to_le_bytes());
            dd.extend_from_slice(&(size as u32).to_le_bytes());
        }
        off += dd.len() as u64;
        send(dd).await?;
        done.push(Written {
            name,
            crc,
            size,
            offset: start,
            date,
            time,
            mtime_s: mtime.div_euclid(1000),
            mode,
        });
    }
    let cd_start = off;
    let mut cd = Vec::new();
    for w in &done {
        let big = w.size > ZIP64_LIMIT || w.offset > ZIP64_LIMIT;
        let mut extra = Vec::new();
        // Info-ZIP universal timestamp (mtime + the atime flag, like Info-ZIP)
        extra.extend_from_slice(&0x5455u16.to_le_bytes());
        extra.extend_from_slice(&5u16.to_le_bytes());
        extra.push(3);
        extra.extend_from_slice(
            &(w.mtime_s.clamp(-0x8000_0000, 0x7fff_ffff) as i32 as u32).to_le_bytes(),
        );
        if big {
            extra.extend_from_slice(&1u16.to_le_bytes());
            extra.extend_from_slice(&24u16.to_le_bytes());
            extra.extend_from_slice(&w.size.to_le_bytes());
            extra.extend_from_slice(&w.size.to_le_bytes());
            extra.extend_from_slice(&w.offset.to_le_bytes());
        }
        let clamp = |v: u64| if big { 0xffff_ffffu32 } else { v as u32 };
        cd.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        cd.extend_from_slice(&(0x033fu16).to_le_bytes());
        cd.extend_from_slice(&(if big { 45u16 } else { 20u16 }).to_le_bytes());
        cd.extend_from_slice(&(0x0808u16).to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&w.time.to_le_bytes());
        cd.extend_from_slice(&w.date.to_le_bytes());
        cd.extend_from_slice(&w.crc.to_le_bytes());
        cd.extend_from_slice(&clamp(w.size).to_le_bytes());
        cd.extend_from_slice(&clamp(w.size).to_le_bytes());
        cd.extend_from_slice(&(w.name.len() as u16).to_le_bytes());
        cd.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&(w.mode << 16).to_le_bytes());
        cd.extend_from_slice(&clamp(w.offset).to_le_bytes());
        cd.extend_from_slice(&w.name);
        cd.extend_from_slice(&extra);
    }
    let cd_size = cd.len() as u64;
    let n = done.len() as u64;
    if n >= 0xffff || cd_start >= 0xffff_ffff || cd_size >= 0xffff_ffff {
        let z64 = cd_start + cd_size;
        cd.extend_from_slice(&0x0606_4b50u32.to_le_bytes());
        cd.extend_from_slice(&44u64.to_le_bytes());
        cd.extend_from_slice(&0x033fu16.to_le_bytes());
        cd.extend_from_slice(&45u16.to_le_bytes());
        cd.extend_from_slice(&0u32.to_le_bytes());
        cd.extend_from_slice(&0u32.to_le_bytes());
        cd.extend_from_slice(&n.to_le_bytes());
        cd.extend_from_slice(&n.to_le_bytes());
        cd.extend_from_slice(&cd_size.to_le_bytes());
        cd.extend_from_slice(&cd_start.to_le_bytes());
        cd.extend_from_slice(&0x0706_4b50u32.to_le_bytes());
        cd.extend_from_slice(&0u32.to_le_bytes());
        cd.extend_from_slice(&z64.to_le_bytes());
        cd.extend_from_slice(&1u32.to_le_bytes());
    }
    let c16 = |v: u64| if v >= 0xffff { 0xffff } else { v as u16 };
    let c32 = |v: u64| {
        if v >= 0xffff_ffff {
            0xffff_ffff
        } else {
            v as u32
        }
    };
    cd.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    cd.extend_from_slice(&[0u8; 4]);
    cd.extend_from_slice(&c16(n).to_le_bytes());
    cd.extend_from_slice(&c16(n).to_le_bytes());
    cd.extend_from_slice(&c32(cd_size).to_le_bytes());
    cd.extend_from_slice(&c32(cd_start).to_le_bytes());
    cd.extend_from_slice(&0u16.to_le_bytes());
    send(cd).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(
            decode_zip_name("Albüm/1.jpg".as_bytes(), 0x800, &[]),
            "Albüm/1.jpg"
        );
        // CP857: 0x81 = ü, 0x8D = ı, 0x9F = ş
        assert_eq!(
            decode_zip_name(
                &[b'g', 0x81, b'n', b' ', b'b', b'a', b't', 0x8d, b'm', 0x8d],
                0,
                &[]
            ),
            "gün batımı"
        );
        assert_eq!(decode_zip_name(&[0x9f], 0, &[]), "ş");
        let mut up = vec![1u8, 0, 0, 0, 0];
        up.extend_from_slice("yılbaşı.jpg".as_bytes());
        assert_eq!(decode_zip_name(b"x", 0, &[(0x7075, up)]), "yılbaşı.jpg");
    }
}
