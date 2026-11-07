//! MEGA link parsing (mirrors `parseMegaUrl`): `/file/<id>#<key>`, `/folder/<id>#<key>` with an
//! optional `/file/<id>` or `/folder/<id>` sub node, and the legacy `#!id!key` / `#F!id!key[!node]`.

use crate::error::HttpError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkType {
    File,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubType {
    File,
    Folder,
    /// legacy `#F!id!key!node`
    Node,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubNode {
    #[serde(rename = "type")]
    pub kind: SubType,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MegaLink {
    #[serde(rename = "type")]
    pub kind: LinkType,
    pub id: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<SubNode>,
}

impl MegaLink {
    pub fn is_folder(&self) -> bool {
        self.kind == LinkType::Folder
    }
}

fn is_id(s: &str) -> bool {
    s.len() == 8
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn is_key(s: &str) -> bool {
    (16..=64).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

const HOSTS: [&str; 4] = ["mega.nz", "www.mega.nz", "mega.co.nz", "www.mega.co.nz"];

/// Parse a MEGA link; anything else → HttpError 400.
pub fn parse_mega_url(input: &str) -> Result<MegaLink, HttpError> {
    let bad = |why: &str| HttpError::new(400, why);
    let not_mega = || bad("Not a MEGA link");
    let invalid = || bad("Invalid MEGA link (id or key missing)");
    if input.len() > 2048 {
        return Err(not_mega());
    }
    let url = url::Url::parse(input.trim()).map_err(|_| not_mega())?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err(not_mega());
    }
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    if !HOSTS.contains(&host.as_str()) || !url.username().is_empty() || url.password().is_some() {
        return Err(not_mega());
    }
    // WHATWG `url.port` is '' for the scheme's default port
    if let Some(p) = url.port() {
        if p != 443 {
            return Err(not_mega());
        }
    }
    let hash = percent_encoding::percent_decode_str(url.fragment().unwrap_or(""))
        .decode_utf8()
        .map_err(|_| not_mega())?
        .into_owned();
    let parts: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();

    if parts.len() == 2 && (parts[0] == "file" || parts[0] == "folder") {
        let kind = if parts[0] == "file" {
            LinkType::File
        } else {
            LinkType::Folder
        };
        let id = parts[1];
        let segs: Vec<&str> = hash.split('/').collect();
        let key = segs[0];
        if !is_id(id) || !is_key(key) {
            return Err(invalid());
        }
        if segs.len() == 1 {
            return Ok(MegaLink {
                kind,
                id: id.into(),
                key: key.into(),
                sub: None,
            });
        }
        let (sub_type, sub_id) = (segs[1], segs.get(2).copied().unwrap_or(""));
        if kind != LinkType::Folder
            || segs.len() > 3
            || (sub_type != "file" && sub_type != "folder")
            || !is_id(sub_id)
        {
            return Err(not_mega());
        }
        let sk = if sub_type == "file" {
            SubType::File
        } else {
            SubType::Folder
        };
        return Ok(MegaLink {
            kind,
            id: id.into(),
            key: key.into(),
            sub: Some(SubNode {
                kind: sk,
                id: sub_id.into(),
            }),
        });
    }

    if parts.is_empty() && hash.starts_with('!') {
        let segs: Vec<&str> = hash.split('!').collect();
        let (id, key) = (
            segs.get(1).copied().unwrap_or(""),
            segs.get(2).copied().unwrap_or(""),
        );
        if segs.len() > 3 || !is_id(id) || !is_key(key) {
            return Err(invalid());
        }
        return Ok(MegaLink {
            kind: LinkType::File,
            id: id.into(),
            key: key.into(),
            sub: None,
        });
    }
    if parts.is_empty() && hash.starts_with("F!") {
        let segs: Vec<&str> = hash.split('!').collect();
        let (id, key) = (
            segs.get(1).copied().unwrap_or(""),
            segs.get(2).copied().unwrap_or(""),
        );
        if segs.len() > 4 || !is_id(id) || !is_key(key) {
            return Err(invalid());
        }
        let sub = segs.get(3).copied().unwrap_or("");
        if sub.is_empty() {
            return Ok(MegaLink {
                kind: LinkType::Folder,
                id: id.into(),
                key: key.into(),
                sub: None,
            });
        }
        if !is_id(sub) {
            return Err(not_mega());
        }
        return Ok(MegaLink {
            kind: LinkType::Folder,
            id: id.into(),
            key: key.into(),
            sub: Some(SubNode {
                kind: SubType::Node,
                id: sub.into(),
            }),
        });
    }
    Err(not_mega())
}

#[cfg(test)]
mod tests {
    use super::*;

    const K22: &str = "AbCdEfGhIjKlMnOpQrStUv";
    const K43: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCd";

    fn link(kind: LinkType, id: &str, key: &str, sub: Option<(SubType, &str)>) -> MegaLink {
        MegaLink {
            kind,
            id: id.into(),
            key: key.into(),
            sub: sub.map(|(kind, id)| SubNode {
                kind,
                id: id.into(),
            }),
        }
    }

    #[test]
    fn accepts_every_mega_link_format() {
        use LinkType::*;
        assert_eq!(
            parse_mega_url(&format!("https://mega.nz/file/AbCd1234#{K43}")).unwrap(),
            link(File, "AbCd1234", K43, None)
        );
        assert_eq!(
            parse_mega_url(&format!("https://mega.nz/folder/AbCd1234#{K22}")).unwrap(),
            link(Folder, "AbCd1234", K22, None)
        );
        assert_eq!(
            parse_mega_url(&format!(
                "https://mega.nz/folder/AbCd1234#{K22}/file/Zz9_-xYw"
            ))
            .unwrap(),
            link(Folder, "AbCd1234", K22, Some((SubType::File, "Zz9_-xYw")))
        );
        assert_eq!(
            parse_mega_url(&format!(
                "https://mega.nz/folder/AbCd1234#{K22}/folder/Qq1Ww2Ee"
            ))
            .unwrap(),
            link(Folder, "AbCd1234", K22, Some((SubType::Folder, "Qq1Ww2Ee")))
        );
        assert_eq!(
            parse_mega_url(&format!("https://mega.nz/#!AbCd1234!{K43}")).unwrap(),
            link(File, "AbCd1234", K43, None)
        );
        assert_eq!(
            parse_mega_url(&format!("https://mega.nz/#F!AbCd1234!{K22}")).unwrap(),
            link(Folder, "AbCd1234", K22, None)
        );
        assert_eq!(
            parse_mega_url(&format!("https://mega.nz/#F!AbCd1234!{K22}!Qq1Ww2Ee")).unwrap(),
            link(Folder, "AbCd1234", K22, Some((SubType::Node, "Qq1Ww2Ee")))
        );
        assert_eq!(
            parse_mega_url(&format!("https://mega.co.nz/#!AbCd1234!{K43}"))
                .unwrap()
                .kind,
            File
        );
        assert_eq!(
            parse_mega_url(&format!("https://www.mega.nz/file/AbCd1234#{K43}"))
                .unwrap()
                .kind,
            File
        );
        assert_eq!(
            parse_mega_url(&format!("  https://mega.nz/file/AbCd1234#{K43}  "))
                .unwrap()
                .id,
            "AbCd1234"
        );
        assert_eq!(
            parse_mega_url(&format!("https://MEGA.NZ:443/file/AbCd1234#{K43}"))
                .unwrap()
                .id,
            "AbCd1234"
        );
        let j = serde_json::to_value(
            parse_mega_url(&format!(
                "https://mega.nz/folder/AbCd1234#{K22}/file/Zz9_-xYw"
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            j,
            serde_json::json!({ "type": "folder", "id": "AbCd1234", "key": K22, "sub": { "type": "file", "id": "Zz9_-xYw" } })
        );
    }

    #[test]
    fn rejects_everything_else_with_400() {
        let bad = [
            String::new(),
            "mega.nz/file/AbCd1234".into(),
            format!("https://evil.com/file/AbCd1234#{K43}"),
            format!("https://mega.nz.evil.com/file/AbCd1234#{K43}"),
            format!("https://user:pw@mega.nz/file/AbCd1234#{K43}"),
            format!("https://mega.nz:8443/file/AbCd1234#{K43}"),
            format!("ftp://mega.nz/file/AbCd1234#{K43}"),
            format!("javascript:alert(1)//mega.nz/file/AbCd1234#{K43}"),
            "https://mega.nz/file/AbCd1234".into(),
            format!("https://mega.nz/file/short#{K43}"),
            "https://mega.nz/file/AbCd1234#bad key!".into(),
            format!("https://mega.nz/file/AbCd1234#{K43}/file/Zz9_-xYw"),
            format!("https://mega.nz/folder/AbCd1234#{K22}/file/../etc"),
            format!("https://mega.nz/folder/AbCd1234#{K22}/x/Zz9_-xYw"),
            format!("https://mega.nz/embed/AbCd1234#{K43}"),
            "https://mega.nz/#!AbCd1234".into(),
            format!("https://mega.nz/#F!AbCd1234!{K22}!a!b"),
            "https://mega.nz/".into(),
            format!("https://mega.nz/file/AbCd1234#{K43}{}", "x".repeat(2048)),
        ];
        for u in bad {
            assert_eq!(parse_mega_url(&u).map_err(|e| e.status), Err(400), "{u}");
        }
    }
}
