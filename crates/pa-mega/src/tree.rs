//! Tree helpers: the breadth-first walk behind `POST /api/remote/inspect` and the import job
//! (`flatten` / `inspectTree` of mega.ts), the remote-folder path map (`buildTree` of
//! folders.ts) and the selection-relative paths of copies (import.ts).

use crate::client::MegaNode;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};

pub const MAX_INSPECT_NODES: usize = 20_000;
pub const MAX_IMPORT_NODES: usize = 200_000;
pub const MAX_REMOTE_NODES: usize = 200_000;

const IMAGE_EXT: [&str; 14] = [
    "jpg", "jpeg", "png", "gif", "webp", "avif", "bmp", "svg", "heic", "heif", "tif", "tiff",
    "jfif", "ico",
];
const VIDEO_EXT: [&str; 15] = [
    "mp4", "m4v", "mov", "webm", "mkv", "avi", "wmv", "flv", "mpg", "mpeg", "3gp", "ts", "mts",
    "m2ts", "ogv",
];
const AUDIO_EXT: [&str; 8] = ["mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "flac"];

/// lower-case extension without the dot ('' when none) — shared `extOf`
pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 || name.len() > 1 => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// 'image' | 'video' | 'audio' | 'archive' — shared `kindOf`. TODO(unify): use `pa_core::kind_of`.
pub fn kind_of(name: &str) -> Option<&'static str> {
    let e = ext_of(name);
    let e = e.as_str();
    if IMAGE_EXT.contains(&e) {
        Some("image")
    } else if VIDEO_EXT.contains(&e) {
        Some("video")
    } else if AUDIO_EXT.contains(&e) {
        Some("audio")
    } else if e == "zip" {
        Some("archive")
    } else {
        None
    }
}

/// Approximation of `a.localeCompare(b, undefined, { numeric: true })`: digit runs compare as
/// numbers, letters case-insensitively first (lower case before upper on ties), then by code point.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    fn runs(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let d = c.is_ascii_digit();
            match out.last_mut() {
                Some((ld, r)) if *ld == d => r.push(c),
                _ => out.push((d, c.to_string())),
            }
        }
        out
    }
    let (ra, rb) = (runs(a), runs(b));
    for (x, y) in ra.iter().zip(rb.iter()) {
        let o = match (x.0, y.0) {
            (true, true) => {
                let (tx, ty) = (x.1.trim_start_matches('0'), y.1.trim_start_matches('0'));
                tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty))
            }
            _ => x.1.to_lowercase().cmp(&y.1.to_lowercase()),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    ra.len().cmp(&rb.len()).then_with(|| {
        // tie on the primary level: lower case first (ICU), then code points
        let flip = |s: &str| {
            s.chars()
                .map(|c| {
                    if c.is_lowercase() {
                        c.to_uppercase().next().unwrap()
                    } else {
                        c.to_lowercase().next().unwrap()
                    }
                })
                .collect::<String>()
        };
        flip(a)
            .cmp(&flip(b))
            .reverse()
            .then_with(|| a.cmp(b))
            .reverse()
    })
}

#[derive(Debug, Clone)]
pub struct FlatNode<'a> {
    pub node: &'a MegaNode,
    /// path inside the shared tree ('' for the root)
    pub path: String,
    /// ids of the node and all of its ancestors (root first)
    pub chain: Vec<String>,
}

/// Breadth-first walk (root first, folders before files, natural name order), capped at `max`.
pub fn flatten(root: &MegaNode, max: usize) -> (Vec<FlatNode<'_>>, bool) {
    let mut out = Vec::new();
    let mut queue = VecDeque::from([FlatNode {
        node: root,
        path: String::new(),
        chain: vec![root.id.clone()],
    }]);
    while let Some(cur) = queue.pop_front() {
        if out.len() >= max {
            return (out, true);
        }
        if cur.node.directory {
            let mut kids: Vec<&MegaNode> = cur.node.children.iter().collect();
            kids.sort_by(|a, b| {
                b.directory
                    .cmp(&a.directory)
                    .then_with(|| natural_cmp(&a.name, &b.name))
            });
            for k in kids {
                let mut chain = cur.chain.clone();
                chain.push(k.id.clone());
                queue.push_back(FlatNode {
                    node: k,
                    path: if cur.path.is_empty() {
                        k.name.clone()
                    } else {
                        format!("{}/{}", cur.path, k.name)
                    },
                    chain,
                });
            }
        }
        out.push(cur);
    }
    (out, false)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RemoteNode {
    pub id: String,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub dir: bool,
    pub kind: Option<&'static str>,
}

/// `RemoteInspect` of the shared API
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RemoteInspect {
    pub provider: &'static str,
    pub kind: &'static str,
    pub name: String,
    pub size: u64,
    pub nodes: Vec<RemoteNode>,
    pub truncated: bool,
}

pub fn inspect_tree(root: &MegaNode) -> RemoteInspect {
    let (nodes, truncated) = flatten(root, MAX_INSPECT_NODES);
    let mut dir_size: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for f in &nodes {
        if !f.node.directory {
            for id in &f.chain[..f.chain.len() - 1] {
                *dir_size.entry(id.as_str()).or_default() += f.node.size;
            }
        }
    }
    let size = nodes
        .iter()
        .filter(|f| !f.node.directory)
        .map(|f| f.node.size)
        .sum();
    let flat = nodes
        .iter()
        .map(|f| RemoteNode {
            id: f.node.id.clone(),
            path: f.path.clone(),
            name: f.node.name.clone(),
            size: if f.node.directory {
                dir_size.get(f.node.id.as_str()).copied().unwrap_or(0)
            } else {
                f.node.size
            },
            dir: f.node.directory,
            kind: if f.node.directory {
                None
            } else {
                kind_of(&f.node.name)
            },
        })
        .collect();
    RemoteInspect {
        provider: "mega",
        kind: if root.directory { "folder" } else { "file" },
        name: root.name.clone(),
        size,
        nodes: flat,
        truncated,
    }
}

/// a MEGA name as a single safe path segment
pub fn segment(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c == '\\' || c == '/' || (c as u32) < 0x20 {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim();
    if s.is_empty() || s == "." || s == ".." {
        return "_".into();
    }
    // JS slice(0, 255) counts UTF-16 units
    let mut units = 0;
    let mut end = s.len();
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > 255 {
            end = i;
            break;
        }
    }
    s[..end].to_string()
}

/// `name`, or `base (2).ext`, `base (3).ext`… — the first one not in `used` (and mark it used)
pub fn unique_in(used: &mut HashSet<String>, name: String) -> String {
    if used.insert(name.clone()) {
        return name;
    }
    let (base, ext) = match name.rfind('.') {
        Some(d) if d > 0 => (&name[..d], &name[d..]),
        _ => (name.as_str(), ""),
    };
    for i in 2.. {
        let n = format!("{base} ({i}){ext}");
        if used.insert(n.clone()) {
            return n;
        }
    }
    unreachable!()
}

fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// rel path → node maps of a remote folder (names made unique per directory like the rows)
#[derive(Debug, Default)]
pub struct RemotePaths<'a> {
    /// rel path → file node
    pub files: Vec<(String, &'a MegaNode)>,
    /// rel path → directory node ('' = the root of a folder link)
    pub dirs: Vec<(String, &'a MegaNode)>,
}

/// `buildTree` of folders.ts (breadth first, children by name then id in UTF-16 order, ≤ max nodes)
pub fn remote_paths(root: &MegaNode, max: usize) -> RemotePaths<'_> {
    let mut out = RemotePaths::default();
    if !root.directory {
        out.files.push((segment(&root.name), root));
        return out;
    }
    out.dirs.push((String::new(), root));
    let mut count = 0;
    let mut queue: VecDeque<(&MegaNode, String)> = VecDeque::from([(root, String::new())]);
    'outer: while let Some((node, rel)) = queue.pop_front() {
        if count >= max {
            break;
        }
        let mut used = HashSet::new();
        let mut kids: Vec<&MegaNode> = node.children.iter().collect();
        kids.sort_by(|a, b| {
            utf16_cmp(&a.name, &b.name).then_with(|| {
                if a.id < b.id {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            })
        });
        for k in kids {
            count += 1;
            if count > max {
                break 'outer;
            }
            let name = unique_in(&mut used, segment(&k.name));
            let r = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if k.directory {
                out.dirs.push((r.clone(), k));
                queue.push_back((k, r));
            } else {
                out.files.push((r, k));
            }
        }
    }
    out
}

/// Path of a file relative to the parent of its outermost selected ancestor (or itself):
/// selecting "Trip/2024" copies "2024/…", selecting a photo copies just the photo.
pub fn relative_to_selection(path: &str, chain: &[String], selected: &HashSet<String>) -> String {
    let Some(j) = chain.iter().position(|id| selected.contains(id)) else {
        return path.to_string();
    };
    if j == 0 {
        return path.to_string();
    }
    let segs: Vec<&str> = path.split('/').collect();
    segs[(j - 1).min(segs.len())..].join("/")
}

/// A single `Range: bytes=…` header against `total` bytes: None (no / unparseable header → whole
/// file), Some(Ok((start, end))) or Some(Err(())) → 416 (`parseRange` of folders.ts).
pub fn parse_range(h: Option<&str>, total: u64) -> Option<Result<(u64, u64), ()>> {
    let h = h?;
    let rest = h.strip_prefix("bytes=")?;
    let (a, b) = rest.split_once('-')?;
    if !a.bytes().all(|c| c.is_ascii_digit()) || !b.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().unwrap_or(0);
        (total.saturating_sub(n), total.wrapping_sub(1))
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() {
            total.wrapping_sub(1)
        } else {
            b.parse::<u64>().ok()?.min(total.wrapping_sub(1))
        };
        (s, e)
    };
    if total == 0 || start >= total || start > end {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(name: &str, dir: bool, size: u64, kids: Vec<MegaNode>) -> MegaNode {
        static ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let id = format!(
            "N{:07}",
            ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        MegaNode {
            id,
            name: name.into(),
            size,
            directory: dir,
            timestamp: Some(1_600_000_000_000),
            children: kids,
            fa: None,
            key: None,
        }
    }

    /// remote.test.ts "inspects a folder link" (the tree part)
    #[test]
    fn inspects_a_folder_tree() {
        let root = n(
            "Holiday",
            true,
            0,
            vec![
                n("a.jpg", false, 1000, vec![]),
                n(
                    "Sub",
                    true,
                    0,
                    vec![
                        n("b.mp4", false, 2000, vec![]),
                        n("notes.txt", false, 10, vec![]),
                    ],
                ),
                n("z.zip", false, 77, vec![]),
            ],
        );
        let ins = inspect_tree(&root);
        assert_eq!(
            (ins.kind, ins.name.as_str(), ins.truncated, ins.size),
            ("folder", "Holiday", false, 3087)
        );
        let got: Vec<(&str, bool, Option<&str>)> = ins
            .nodes
            .iter()
            .map(|x| (x.path.as_str(), x.dir, x.kind))
            .collect();
        assert_eq!(
            got,
            vec![
                ("", true, None),
                ("Sub", true, None),
                ("a.jpg", false, Some("image")),
                ("z.zip", false, Some("archive")),
                ("Sub/b.mp4", false, Some("video")),
                ("Sub/notes.txt", false, None)
            ]
        );
        assert_eq!(
            ins.nodes.iter().find(|x| x.path == "Sub").unwrap().size,
            2010
        );
        let one = inspect_tree(&n("clip.mp4", false, 500, vec![]));
        assert_eq!(
            (
                one.kind,
                one.size,
                one.nodes.len(),
                one.nodes[0].path.as_str()
            ),
            ("file", 500, 1, "")
        );
        let (f, t) = flatten(&root, 3);
        assert!(t);
        assert_eq!(f.len(), 3);
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["img10.jpg", "img2.jpg", "Img1.jpg", "b", "a", "A"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["a", "A", "b", "Img1.jpg", "img2.jpg", "img10.jpg"]);
    }

    #[test]
    fn remote_paths_unique_and_safe() {
        let root = n(
            "R",
            true,
            0,
            vec![
                n("x.jpg", false, 1, vec![]),
                n("x.jpg", false, 2, vec![]),
                n("a/b", true, 0, vec![n("..", false, 3, vec![])]),
            ],
        );
        let p = remote_paths(&root, 100);
        let files: Vec<&str> = p.files.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(files, vec!["x.jpg", "x (2).jpg", "a_b/_"]);
        assert_eq!(
            p.dirs.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(),
            vec!["", "a_b"]
        );
        assert_eq!(segment("  "), "_");
        assert_eq!(ext_of("A.JPG"), "jpg");
    }

    #[test]
    fn selection_relative_paths() {
        let sel: HashSet<String> = ["S".to_string()].into();
        assert_eq!(
            relative_to_selection(
                "Trip/2024/a.jpg",
                &["R".into(), "T".into(), "S".into(), "F".into()],
                &sel
            ),
            "2024/a.jpg"
        );
        assert_eq!(relative_to_selection("a.jpg", &["S".into()], &sel), "a.jpg");
    }

    /// remote-folders.test.ts "parses ranges like the browser sends them"
    #[test]
    fn parses_ranges() {
        assert_eq!(parse_range(None, 100), None);
        assert_eq!(parse_range(Some("bytes=0-"), 100), Some(Ok((0, 99))));
        assert_eq!(parse_range(Some("bytes=10-19"), 100), Some(Ok((10, 19))));
        assert_eq!(parse_range(Some("bytes=90-500"), 100), Some(Ok((90, 99))));
        assert_eq!(parse_range(Some("bytes=-10"), 100), Some(Ok((90, 99))));
        assert_eq!(parse_range(Some("bytes=100-"), 100), Some(Err(())));
        assert_eq!(parse_range(Some("bytes=5-2"), 100), Some(Err(())));
    }
}
