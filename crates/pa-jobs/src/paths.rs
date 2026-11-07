//! Path helpers of `paths.ts` the job / transfer code needs (shared with pa-link and pa-remote).

use crate::error::HttpError;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// `normalizeRel`: `\` → `/`, drop empty / `.` segments; `..` or NUL → 400 'Invalid path'.
pub fn normalize_rel(rel: &str) -> Result<String, HttpError> {
    let s = rel.replace('\\', "/");
    let parts: Vec<&str> = s
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    for p in &parts {
        if *p == ".." || p.contains('\0') {
            return Err(HttpError::bad("Invalid path"));
        }
    }
    Ok(parts.join("/"))
}

static WIN_RESERVED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(con|prn|aux|nul|com\d|lpt\d)$").unwrap());

/// `sanitizeName`: one path segment that is safe on every OS.
pub fn sanitize_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if (c as u32) < 0x20 || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = replaced.trim_end_matches(['.', ' ']);
    let mut n = pa_core::js::trim(trimmed).to_string();
    if n.is_empty() || n == "." || n == ".." {
        n = "untitled".into();
    }
    let base = n.split('.').next().unwrap_or("");
    if WIN_RESERVED.is_match(base) {
        n = format!("_{n}");
    }
    if pa_core::js::len16(&n) > 200 {
        let ext = extname(&n).to_string();
        let keep = 200 - pa_core::js::len16(&ext).min(200);
        n = format!("{}{}", pa_core::js::slice16_to(&n, keep), ext);
    }
    n
}

/// `path.extname` of a file name ("" for ".bashrc" / "name").
pub fn extname(name: &str) -> &str {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if i > 0 => &base[i..],
        _ => "",
    }
}

/// `exists` (lstat)
pub async fn exists(p: &Path) -> bool {
    tokio::fs::symlink_metadata(p).await.is_ok()
}

/// `uniquePath`: "a.jpg" → "a (1).jpg" … until free.
pub async fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let ext = extname(name);
    let base = &name[..name.len() - ext.len()];
    let mut candidate = dir.join(name);
    let mut i = 1;
    while exists(&candidate).await {
        candidate = dir.join(format!("{base} ({i}){ext}"));
        i += 1;
    }
    candidate
}

/// `uniqueEditPath`: "a" + "_edit" + "mp4" → "a_edit.mp4", "a_edit-2.mp4" …
pub async fn unique_edit_path(dir: &Path, base: &str, suffix: &str, ext: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{base}{suffix}.{ext}"));
    let mut i = 2;
    while exists(&candidate).await {
        candidate = dir.join(format!("{base}{suffix}-{i}.{ext}"));
        i += 1;
    }
    candidate
}

/// `path.parse(name).name`
pub fn stem(name: &str) -> &str {
    &name[..name.len() - extname(name).len()]
}

/// `dir ? dir/name : name`
pub fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// `path.posix.dirname` with "." → ""
pub fn posix_dirname(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[..i],
        None => "",
    }
}

/// `path.posix.basename`
pub fn posix_basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// `isInside`
pub fn is_inside(root: &Path, p: &Path) -> bool {
    p == root || p.starts_with(root)
}

/// `resolveInRoot`: `rel` inside `root`, also after following the symlinks of the existing part.
pub fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf, HttpError> {
    let clean = normalize_rel(rel)?;
    let mut abs = root.to_path_buf();
    for s in clean.split('/').filter(|s| !s.is_empty()) {
        abs.push(s);
    }
    if !is_inside(root, &abs) {
        return Err(HttpError::bad("Path escapes root"));
    }
    let mut probe = abs.clone();
    while !probe.exists() && probe != root {
        if !probe.pop() {
            break;
        }
    }
    if let (Ok(rr), Ok(rp)) = (std::fs::canonicalize(root), std::fs::canonicalize(&probe)) {
        if !is_inside(&rr, &rp) {
            return Err(HttpError::bad("Path escapes root"));
        }
    }
    Ok(abs)
}

/// `toPosix(path.relative(root, p))`
pub fn rel_posix(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map(|r| {
            r.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default()
}

/// Set a file's mtime (ms epoch) and atime = now; errors ignored.
pub async fn set_mtime(p: &Path, mtime_ms: i64) {
    let p = p.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || {
        let f = std::fs::OpenOptions::new().write(true).open(&p)?;
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(mtime_ms.max(0) as u64);
        f.set_times(
            std::fs::FileTimes::new()
                .set_modified(t)
                .set_accessed(std::time::SystemTime::now()),
        )
    })
    .await;
}

/// 8 random hex chars (`crypto.randomBytes(4).toString('hex')`)
pub fn rand_hex4() -> String {
    format!("{:08x}", rand::random::<u32>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(sanitize_name("a/b:c?.jpg"), "a_b_c_.jpg");
        assert_eq!(sanitize_name("  .. "), "untitled");
        assert_eq!(sanitize_name("con.txt"), "_con.txt");
        assert_eq!(sanitize_name("x. . "), "x");
        assert_eq!(normalize_rel("a\\b/./c//").unwrap(), "a/b/c");
        assert!(normalize_rel("a/../b").is_err());
        assert_eq!(extname("a.tar.gz"), ".gz");
        assert_eq!(extname(".bashrc"), "");
        assert_eq!(stem("clip.mp4"), "clip");
        assert_eq!(posix_dirname("a/b/c"), "a/b");
        assert_eq!(posix_dirname("c"), "");
    }
}
