//! Library-side traits shared by the phone link (pa-link) and MEGA folders (pa-remote): the media
//! rows filled from a listing, and the server's thumbnail service.

use crate::error::HttpError;
use async_trait::async_trait;
use pa_core::MediaKind;
use pa_meta::ExtractedMeta;

/// One file from a `list` reply, ready for the media table (`meta_state = 1`). The server computes
/// the grouping columns with `pa_meta::group_columns(&RowInfo{name,dir,ext,kind,mtime}, &meta, geo, tz)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedFile {
    pub dir: String,
    pub name: String,
    pub ext: String,
    pub kind: MediaKind,
    pub size: i64,
    pub mtime: i64,
    /// width / height / duration / playback / instant (taken) filled from the listing
    pub meta: ExtractedMeta,
    /// 1 = final (phones), 3 = MEGA rows waiting for background metadata (`META_REMOTE_PENDING`)
    pub meta_state: i64,
}

/// Scope of a (re)listing: the folder, or one sub tree of it (`dir = ? OR dir LIKE 'dir/%'`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListScope {
    pub folder_id: i64,
    /// "" = the whole folder
    pub dir: String,
}

/// Media rows of a listed folder (devices and MEGA links) + the scanner's skip rules.
pub trait MediaRowSink: Send + Sync + 'static {
    /// `scanner.skipName(name)` (trash dir, hidden names unless shown)
    fn skip_name(&self, name: &str) -> bool;
    /// `scanner.skipFile(folderId, rel, size)` (hidden segments, library exclusions, min size)
    fn skip_file(&self, folder_id: i64, rel: &str, size: i64) -> bool;
    /// `scanner.skipDir(folderId, rel)`
    fn skip_dir(&self, folder_id: i64, rel: &str) -> bool;
    /// `UPDATE media SET seen = 0 WHERE folder_id = ? <scope>`; returns `max(id)` of the media table
    fn list_begin(&self, scope: &ListScope) -> i64;
    /// one transaction: per file `UPDATE … SET seen = 1 WHERE same dir/name/size/mtime`, else the
    /// INSERT … ON CONFLICT DO UPDATE of hub.ts `upsertListed`. Returns how many rows were
    /// inserted or changed.
    fn list_upsert(&self, folder_id: i64, files: &[ListedFile]) -> usize;
    /// a failed listing: `UPDATE media SET seen = 1 WHERE folder_id = ? <scope>`
    fn list_abort(&self, scope: &ListScope);
    /// `DELETE … seen = 0 <scope>` (→ removed), replace `device_dirs` in scope with `dirs`, count rows
    /// with `id > max_before` (→ added). Returns (added, removed).
    fn list_finish(&self, scope: &ListScope, dirs: &[String], max_before: i64) -> (usize, usize);
}

/// A media row waiting for its thumbnail (`thumb_state = 0`).
#[derive(Debug, Clone, PartialEq)]
pub struct ThumbRow {
    pub id: i64,
    pub folder_id: i64,
    pub dir: String,
    pub name: String,
    pub kind: MediaKind,
    pub width: Option<f64>,
    pub height: Option<f64>,
    /// folder path (`device://<id>/<key>` / `remote://mega/<linkId>`)
    pub folder_path: String,
}

impl ThumbRow {
    /// `${fpath}/${dir ? dir + '/' : ''}${name}`
    pub fn abs(&self) -> String {
        if self.dir.is_empty() {
            format!("{}/{}", self.folder_path, self.name)
        } else {
            format!("{}/{}/{}", self.folder_path, self.dir, self.name)
        }
    }
}

/// `ThumbService.generateBase` result
#[derive(Debug, Clone, PartialEq)]
pub struct BaseThumb {
    pub thumbhash: Option<String>,
    pub width: u32,
    pub height: u32,
}

/// The server's thumbnail service (cache + renditions) and the thumb columns of the media table.
#[async_trait]
pub trait ThumbCache: Send + Sync + 'static {
    /// `thumbs.generateBase(abs, row, 'low')`: renders (through the tunnel / MEGA via the
    /// server's RemoteImageSource hookup) and caches the base thumbnails; None = cannot render.
    async fn generate_base(&self, abs: &str, media_id: i64)
        -> Result<Option<BaseThumb>, HttpError>;
    /// `UPDATE media SET thumbhash = ?, thumb_state = ?, width = COALESCE(width, ?), height = COALESCE(height, ?) WHERE id = ?`
    fn set_thumb(
        &self,
        media_id: i64,
        thumbhash: Option<&str>,
        state: i64,
        width: Option<u32>,
        height: Option<u32>,
    );
    /// rows with `thumb_state = 0 AND kind IN ('image','video') ORDER BY taken_local DESC LIMIT 16`
    /// of one device (`f.device_id = ?`) or of every MEGA folder (`f.remote IS NOT NULL`)
    fn pending_thumbs(&self, scope: ThumbScope<'_>) -> Vec<ThumbRow>;
}

/// Which folders [`ThumbCache::pending_thumbs`] looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbScope<'a> {
    Device(&'a str),
    Remote,
}

/// A response body (tunnel, MEGA read, cached file …).
pub type BodyStream =
    std::pin::Pin<Box<dyn futures_util::Stream<Item = std::io::Result<bytes::Bytes>> + Send>>;

/// A file response built by pa-link / pa-remote (`/api/media/:id/file`, pull GETs): send `status`,
/// the headers in order, then the body (None = empty body).
pub struct FileReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Option<BodyStream>,
}

impl FileReply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

impl std::fmt::Debug for FileReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileReply")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body", &self.body.is_some())
            .finish()
    }
}

/// `MIME` of devices/media.ts
pub fn mime_of(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" | "jfif" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/x-icon",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "3gp" => "video/3gpp",
        "ogv" => "video/ogg",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Accept exactly one byte range ("bytes=a-b", "bytes=a-", "bytes=-n"); anything else → whole file.
pub fn single_range(h: Option<&str>) -> Option<String> {
    let t = pa_core::js::trim(h?);
    let rest = t.strip_prefix("bytes=")?;
    let (a, b) = rest.split_once('-')?;
    let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    if !digits(a) || !digits(b) || (a.is_empty() && b.is_empty()) {
        return None;
    }
    Some(format!("bytes={a}-{b}"))
}

/// `"bytes=a-b"` (already validated by [`single_range`]) → Some(Ok((start, end))), Some(Err) =
/// unsatisfiable (416), None = whole file (`parseRange` of remote/folders.ts)
pub fn parse_range(h: Option<&str>, total: u64) -> Option<Result<(u64, u64), ()>> {
    let h = h?;
    let rest = h.strip_prefix("bytes=")?;
    let (a, b) = rest.split_once('-')?;
    let num = |s: &str| pa_core::js::number(s);
    let (start, end) = if a.is_empty() {
        let n = num(b);
        if n == 0.0 || n.is_nan() {
            return Some(Err(()));
        }
        ((total as f64 - n).max(0.0), total as f64 - 1.0)
    } else {
        let start = num(a);
        let end = if b.is_empty() {
            total as f64 - 1.0
        } else {
            num(b).min(total as f64 - 1.0)
        };
        (start, end)
    };
    if start >= total as f64 || start > end {
        return Some(Err(()));
    }
    Some(Ok((start as u64, end as u64)))
}

/// `Content-Disposition: attachment` with an ASCII fallback and the UTF-8 name
pub fn content_disposition(name: &str) -> String {
    // per UTF-16 unit like the JS regexp: astral characters become "__"
    let ascii: String = name
        .chars()
        .filter(|c| *c != '"')
        .map(|c| match c {
            ' '..='~' => c.to_string(),
            c if (c as u32) > 0xffff => "__".into(),
            _ => "_".into(),
        })
        .collect();
    let mut enc = String::new();
    for b in name.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || "-_.!~*'()".contains(c) {
            enc.push(c);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{enc}")
}

/// `new Date(ms).toUTCString()`
pub fn http_date(ms: i64) -> String {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms.max(0) as u64);
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = pa_core::js::civil_from_days(days);
    let wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(days.rem_euclid(7)) as usize];
    let mon = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(m - 1) as usize];
    format!(
        "{wd}, {d:02} {mon} {y} {:02}:{:02}:{:02} GMT",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// `dir ? dir/name : name` of a media row
pub fn rel_of_row(dir: &str, name: &str) -> String {
    crate::paths::join_rel(dir, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_helpers() {
        assert_eq!(http_date(1690000000000), "Sat, 22 Jul 2023 04:26:40 GMT");
        assert_eq!(
            single_range(Some(" bytes=0-9 ")).as_deref(),
            Some("bytes=0-9")
        );
        assert_eq!(single_range(Some("bytes=-")), None);
        assert_eq!(single_range(Some("bytes=0-1,5-6")), None);
        assert_eq!(parse_range(Some("bytes=-10"), 100), Some(Ok((90, 99))));
        assert_eq!(parse_range(Some("bytes=5000-"), 4096), Some(Err(())));
        assert_eq!(
            parse_range(Some("bytes=10-5000"), 4096),
            Some(Ok((10, 4095)))
        );
        assert_eq!(parse_range(Some("bytes=-0"), 4096), Some(Err(())));
        assert_eq!(
            content_disposition("Ağaç \"x\".jpg"),
            "attachment; filename=\"A_a_ x.jpg\"; filename*=UTF-8''A%C4%9Fa%C3%A7%20%22x%22.jpg"
        );
    }
}
