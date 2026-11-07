//! Path helpers (mirrors `paths.ts`): client paths are normalised and confined to their root,
//! names are made safe for every OS, collisions get " (1)" suffixes.

use crate::error::{ApiResult, HttpError};
use std::path::{Component, Path, PathBuf};

/// Node's `path.resolve(p)`: absolute (against the cwd) and lexically normalised (no symlinks).
pub fn resolve(p: impl AsRef<Path>) -> PathBuf {
    let p = p.as_ref();
    let base = if p.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
    };
    normalize(&base.join(p))
}

/// `path.resolve(root, ...segments)`
pub fn resolve_join(root: &Path, segs: &[&str]) -> PathBuf {
    let mut p = resolve(root);
    for s in segs {
        p.push(s);
    }
    normalize(&p)
}

/// lexical normalisation ('.', '..', duplicate separators)
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Prefix(x) => out.push(x.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !out.has_root() {
                    out.push("..");
                }
            }
            Component::Normal(x) => out.push(x),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Normalize a client supplied relative path ("a/b", "a\\b", "/a/b/") → "a/b". Rejects traversal.
pub fn normalize_rel(rel: Option<&str>) -> ApiResult<String> {
    let Some(rel) = rel else {
        return Ok(String::new());
    };
    if rel.is_empty() {
        return Ok(String::new());
    }
    let s = rel.replace('\\', "/");
    let parts: Vec<&str> = s
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    for p in &parts {
        if *p == ".." || p.contains('\0') {
            return Err(HttpError::new(400, "Invalid path"));
        }
    }
    Ok(parts.join("/"))
}

/// `isInside(root, p)` after lexical resolution of both.
pub fn is_inside(root: impl AsRef<Path>, p: impl AsRef<Path>) -> bool {
    let r = resolve(root);
    let x = resolve(p);
    x == r || x.starts_with(&r)
}

/// Resolve `rel` inside `root` and guarantee the result stays inside it, also after following
/// symlinks of the existing part of the path.
pub fn resolve_in_root(root: &str, rel: &str) -> ApiResult<PathBuf> {
    let clean = normalize_rel(Some(rel))?;
    let root_abs = resolve(root);
    let segs: Vec<&str> = clean.split('/').filter(|s| !s.is_empty()).collect();
    let abs = resolve_join(&root_abs, &segs);
    if !is_inside(&root_abs, &abs) {
        return Err(HttpError::new(400, "Path escapes root"));
    }
    let mut probe = abs.clone();
    while !probe.exists() && probe != root_abs {
        match probe.parent() {
            Some(p) => probe = p.to_path_buf(),
            None => break,
        }
    }
    if let (Ok(real_root), Ok(real_probe)) = (
        std::fs::canonicalize(&root_abs),
        std::fs::canonicalize(&probe),
    ) {
        if !is_inside(&real_root, &real_probe) {
            return Err(HttpError::new(400, "Path escapes root"));
        }
    }
    Ok(abs)
}

/// Linear-time trim of any of `chars` from the end.
pub fn trim_end<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_end_matches(|c| chars.contains(c))
}

/// Linear-time trim of any of `chars` from both ends.
pub fn trim_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}

/// Resolve a user supplied absolute host path and verify it lies inside one of `roots`.
pub fn resolve_host_path(p: &str, roots: &[PathBuf]) -> ApiResult<PathBuf> {
    if p.is_empty() || p.contains('\0') {
        return Err(HttpError::new(400, "Invalid path"));
    }
    let abs = resolve(p);
    if !roots.is_empty() && !roots.iter().any(|r| is_inside(r, &abs)) {
        return Err(HttpError::new(
            403,
            "Path is outside of the allowed folders",
        ));
    }
    Ok(abs)
}

/// Node's `path.extname` of a single name.
pub fn extname(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if i > 0 && !base[..i].chars().all(|c| c == '.') => &base[i..],
        _ => "",
    }
}

fn is_win_reserved(base: &str) -> bool {
    let b = base.to_ascii_lowercase();
    if ["con", "prn", "aux", "nul"].contains(&b.as_str()) {
        return true;
    }
    for p in ["com", "lpt"] {
        if let Some(rest) = b.strip_prefix(p) {
            let mut cs = rest.chars();
            if let (Some(c), None) = (cs.next(), cs.next()) {
                if c.is_ascii_digit() {
                    return true;
                }
            }
        }
    }
    false
}

/// Make a single path segment safe for every OS.
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
    let mut n = pa_core::js::trim(trim_end(&replaced, ". ")).to_string();
    if n.is_empty() || n == "." || n == ".." {
        n = "untitled".into();
    }
    let base = n.split('.').next().unwrap_or("");
    if is_win_reserved(base) {
        n = format!("_{n}");
    }
    if pa_core::js::len16(&n) > 200 {
        let ext = extname(&n).to_string();
        let keep = 200usize.saturating_sub(pa_core::js::len16(&ext));
        n = format!("{}{ext}", pa_core::js::slice16_to(&n, keep));
    }
    n
}

/// Sanitize every segment of a relative path.
pub fn sanitize_rel(rel: &str) -> ApiResult<String> {
    Ok(normalize_rel(Some(rel))?
        .split('/')
        .filter(|s| !s.is_empty())
        .map(sanitize_name)
        .collect::<Vec<_>>()
        .join("/"))
}

/// `lstat` succeeds
pub fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// "a.jpg" → "a (1).jpg" … until free.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let ext = extname(name);
    let base = &name[..name.len() - ext.len()];
    let mut candidate = dir.join(name);
    let mut i = 1;
    while exists(&candidate) {
        candidate = dir.join(format!("{base} ({i}){ext}"));
        i += 1;
    }
    candidate
}

/// "a" + "_edit" + "jpg" → "a_edit.jpg", "a_edit-2.jpg" …
pub fn unique_edit_path(dir: &Path, base: &str, suffix: &str, ext: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{base}{suffix}.{ext}"));
    let mut i = 2;
    while exists(&candidate) {
        candidate = dir.join(format!("{base}{suffix}-{i}.{ext}"));
        i += 1;
    }
    candidate
}

/// '/' separated
pub fn to_posix(p: &Path) -> String {
    let s = p.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        s.into_owned()
    } else {
        s.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

/// `path.relative(root, abs)` as a posix path (both lexically resolved).
pub fn rel_of(root: &str, abs: &Path) -> String {
    let r = resolve(root);
    let a = resolve(abs);
    match a.strip_prefix(&r) {
        Ok(rest) => to_posix(rest),
        Err(_) => {
            // walk up like path.relative
            let rc: Vec<_> = r.components().collect();
            let ac: Vec<_> = a.components().collect();
            let mut i = 0;
            while i < rc.len() && i < ac.len() && rc[i] == ac[i] {
                i += 1;
            }
            let mut parts: Vec<String> = vec!["..".into(); rc.len() - i];
            parts.extend(
                ac[i..]
                    .iter()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned()),
            );
            parts.join("/")
        }
    }
}

/// dot files and NAS / Windows system folders
pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
        || name == "@eaDir"
        || name == "$RECYCLE.BIN"
        || name == "System Volume Information"
        || name == "#recycle"
}

/// `path.posix.dirname(rel)` with '.' → ''
pub fn dir_of_rel(rel: &str) -> &str {
    match rel.rfind('/') {
        Some(0) => "/",
        Some(i) => &rel[..i],
        None => "",
    }
}

/// `path.posix.basename(rel)`
pub fn base_of_rel(rel: &str) -> &str {
    let t = rel.trim_end_matches('/');
    t.rsplit('/').next().unwrap_or(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(normalize_rel(Some("/a\\b/./c/")).unwrap(), "a/b/c");
        assert!(normalize_rel(Some("a/../b")).is_err());
        assert!(is_inside("/a/b", "/a/b/c"));
        assert!(!is_inside("/a/b", "/a/bc"));
        assert!(!is_inside("/a/b", "/a/b/../c"));
        assert_eq!(
            trim_chars(
                &format!("{}a{}", "/".repeat(200000), "/".repeat(200000)),
                "/"
            ),
            "a"
        );
        assert!(sanitize_name(&format!("x{}!", ". ".repeat(100000))).len() <= 200);
        assert_eq!(
            sanitize_name(&format!("name{}", " .".repeat(100000))),
            "name"
        );
        assert_eq!(sanitize_name("con.txt"), "_con.txt");
        assert_eq!(sanitize_name("a:b?.jpg"), "a_b_.jpg");
        assert_eq!(sanitize_name(".."), "untitled");
        assert!(resolve_host_path("/etc", &[PathBuf::from("/home")]).is_err());
        assert_eq!(
            resolve_host_path("/home/x/../x/y", &[PathBuf::from("/home")]).unwrap(),
            PathBuf::from("/home/x/y")
        );
        assert!(resolve_host_path("/a\0b", &[]).is_err());
        assert_eq!(extname("a.b.jpg"), ".jpg");
        assert_eq!(extname(".hidden"), "");
        assert_eq!(extname("a."), ".");
        assert_eq!(rel_of("/a/b", Path::new("/a/b/c/d")), "c/d");
        assert_eq!(dir_of_rel("a/b"), "a");
        assert_eq!(dir_of_rel("b"), "");
    }
}
