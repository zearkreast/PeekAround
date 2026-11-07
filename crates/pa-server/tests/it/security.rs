//! Mirrors security.test.ts + paths.test.ts + zip.test.ts

use crate::common::*;
use pa_server::paths::{
    is_inside, normalize_rel, resolve_host_path, resolve_in_root, sanitize_name, trim_chars,
    unique_edit_path, unique_path,
};
use pa_server::security::{host_name, is_allowed_host, is_allowed_origin};
use serde_json::json;
use std::path::{Path, PathBuf};

#[test]
fn security_helpers() {
    assert_eq!(host_name(Some("Example.com:8080")), "example.com");
    assert_eq!(host_name(Some("[::1]:8080")), "::1");
    assert_eq!(host_name(Some("192.168.1.5")), "192.168.1.5");
    for h in [
        Some("localhost:47213"),
        Some("127.0.0.1:8080"),
        Some("192.168.1.20:8080"),
        Some("[::1]:1"),
        Some("nas:8080"),
        Some("nas.local"),
        Some("pc.lan"),
        Some("box.home.arpa"),
        None,
    ] {
        assert!(is_allowed_host(h, &[]), "{h:?}");
    }
    for h in ["evil.example.com", "attacker.net:8080", "127.0.0.1.nip.io"] {
        assert!(!is_allowed_host(Some(h), &[]), "{h}");
    }
    assert!(is_allowed_host(
        Some("photos.example.com"),
        &["photos.example.com".into()]
    ));
    assert!(is_allowed_host(
        Some("a.example.com"),
        &["*.example.com".into()]
    ));
    assert!(is_allowed_host(Some("anything.com"), &["*".into()]));

    assert!(is_allowed_origin(None, Some("127.0.0.1:8080"), &[], false));
    assert!(is_allowed_origin(
        Some("http://127.0.0.1:8080"),
        Some("127.0.0.1:8080"),
        &[],
        false
    ));
    assert!(is_allowed_origin(
        Some("http://192.168.1.5:8080"),
        Some("192.168.1.5:8080"),
        &[],
        false
    ));
    assert!(is_allowed_origin(
        Some("http://localhost"),
        Some("192.168.1.5:8080"),
        &[],
        false
    ));
    assert!(!is_allowed_origin(
        Some("https://evil.example"),
        Some("127.0.0.1:8080"),
        &[],
        false
    ));
    assert!(!is_allowed_origin(
        Some("null"),
        Some("127.0.0.1:8080"),
        &[],
        false
    ));
    assert!(!is_allowed_origin(
        Some("http://localhost:5173"),
        Some("127.0.0.1:8787"),
        &[],
        false
    ));
    assert!(is_allowed_origin(
        Some("http://localhost:5173"),
        Some("127.0.0.1:8787"),
        &[],
        true
    ));
    assert!(is_allowed_origin(
        Some("https://my.app"),
        Some("127.0.0.1:8787"),
        &["https://my.app".into()],
        false
    ));
}

#[test]
fn path_helpers_are_linear_and_strict() {
    let t0 = std::time::Instant::now();
    let s = format!("{}a{}", "/".repeat(200_000), "/".repeat(200_000));
    assert_eq!(trim_chars(&s, "/"), "a");
    assert!(
        sanitize_name(&format!("x{}!", ". ".repeat(100_000)))
            .encode_utf16()
            .count()
            <= 200
    );
    assert_eq!(
        sanitize_name(&format!("name{}", " .".repeat(100_000))),
        "name"
    );
    assert!(t0.elapsed().as_millis() < 1000);
    assert!(is_inside("/a/b", "/a/b/c"));
    assert!(!is_inside("/a/b", "/a/bc"));
    assert!(!is_inside("/a/b", "/a/b/../c"));
    assert!(resolve_host_path("/etc", &[PathBuf::from("/home")]).is_err());
    assert_eq!(
        resolve_host_path("/home/x/../x/y", &[PathBuf::from("/home")]).unwrap(),
        PathBuf::from("/home/x/y")
    );
    assert!(resolve_host_path("/a\0b", &[]).is_err());
}

#[test]
fn paths() {
    let root = Tmp::new("paths");
    assert_eq!(normalize_rel(Some("/a//b/./c/")).unwrap(), "a/b/c");
    assert_eq!(normalize_rel(Some("a\\b")).unwrap(), "a/b");
    assert!(normalize_rel(Some("../etc")).is_err());
    assert!(normalize_rel(Some("a/../../b")).is_err());
    let r = root.0.to_string_lossy().into_owned();
    assert_eq!(
        resolve_in_root(&r, "x/y.jpg").unwrap(),
        root.join("x").join("y.jpg")
    );
    std::os::unix::fs::symlink(std::env::temp_dir(), root.join("escape")).unwrap();
    assert!(resolve_in_root(&r, "escape/whatever").is_err());
    assert_eq!(sanitize_name("a/b:c*?.jpg"), "a_b_c__.jpg");
    assert_eq!(sanitize_name("CON.txt"), "_CON.txt");
    assert_eq!(sanitize_name(".."), "untitled");
    assert_eq!(sanitize_name("İstanbul Boğazı.png"), "İstanbul Boğazı.png");
    std::fs::write(root.join("a.jpg"), "").unwrap();
    assert_eq!(
        unique_path(&root.0, "a.jpg").file_name().unwrap(),
        "a (1).jpg"
    );
    std::fs::write(root.join("a_edit.jpg"), "").unwrap();
    assert_eq!(
        unique_edit_path(&root.0, "a", "_edit", "jpg")
            .file_name()
            .unwrap(),
        "a_edit-2.jpg"
    );
}

#[test]
fn zip_extract_and_names() {
    let dir = Tmp::new("zip");
    // "../" names: write a same-length placeholder, patch the bytes afterwards (like the TS test)
    let mut z = zip_bytes(&[
        ("Düğün/kare 1.jpg", b"x"),
        ("XX/XX/evil.txt", b"bad"),
        ("__MACOSX/._junk", b"y"),
    ]);
    let (from, to) = (b"XX/XX/evil.txt".as_slice(), b"../../evil.txt".as_slice());
    let mut i = 0;
    while let Some(p) = z[i..].windows(from.len()).position(|w| w == from) {
        z[i + p..i + p + to.len()].copy_from_slice(to);
        i += p + 1;
    }
    let zip = dir.join("a.zip");
    std::fs::write(&zip, z).unwrap();
    let out = dir.join("out");
    let r = pa_server::zip::extract_zip(&zip, &out, pa_server::zip::MAX_EXTRACT).unwrap();
    assert_eq!(r.files, vec!["Düğün/kare 1.jpg"]);
    assert!(!dir.join("evil.txt").exists());
    assert!(out.join("Düğün").join("kare 1.jpg").exists());
    // CP857: ş=0x9F ğ=0xA7 İ=0x98 ı=0x8D
    assert_eq!(
        pa_server::zip::decode_zip_name(&[0x9f, 0xa7, 0x98, 0x8d, 0x2e, 0x6a, 0x70, 0x67], 0, &[]),
        "şğİı.jpg"
    );
    assert_eq!(
        pa_server::zip::decode_zip_name("ğüş.png".as_bytes(), 0x800, &[]),
        "ğüş.png"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn security_hooks() {
    let tmp = Tmp::new("sec");
    let lib = tmp.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let srv = start(&tmp, &lib).await;
    // refuses unknown Host names (DNS rebinding)
    assert_eq!(
        inj(&srv, "GET", "/api/info")
            .header("host", "evil.example.com")
            .send()
            .await
            .status,
        421
    );
    assert_eq!(
        inj(&srv, "GET", "/api/info")
            .header("host", "192.168.1.9:8080")
            .send()
            .await
            .status,
        200
    );
    // refuses cross-origin writes but allows same-origin and the app
    let mk = |name: &str| json!({"folderId": "1", "dir": "", "name": name});
    let bad = inj(&srv, "POST", "/api/fs/mkdir")
        .header("host", "127.0.0.1:8080")
        .header("origin", "https://evil.example")
        .json(mk("x"))
        .send()
        .await;
    assert_eq!(bad.status, 403);
    let sneaky = inj(&srv, "POST", "/api/fs/mkdir")
        .header("host", "127.0.0.1:8080")
        .header("sec-fetch-site", "cross-site")
        .json(mk("x"))
        .send()
        .await;
    assert_eq!(sneaky.status, 403);
    let ok = inj(&srv, "POST", "/api/fs/mkdir")
        .header("host", "127.0.0.1:8080")
        .header("origin", "http://127.0.0.1:8080")
        .json(mk("ok"))
        .send()
        .await;
    assert_eq!(ok.status, 200);
    let app = inj(&srv, "POST", "/api/fs/mkdir")
        .header("host", "192.168.1.9:8080")
        .header("origin", "http://localhost")
        .json(mk("ok2"))
        .send()
        .await;
    assert_eq!(app.status, 200);
    // only reflects CORS for allowed origins
    let good = inj(&srv, "GET", "/api/info")
        .header("host", "192.168.1.9:8080")
        .header("origin", "http://localhost")
        .send()
        .await;
    assert_eq!(
        good.h("access-control-allow-origin"),
        Some("http://localhost")
    );
    let bad = inj(&srv, "GET", "/api/info")
        .header("host", "192.168.1.9:8080")
        .header("origin", "https://evil.example")
        .send()
        .await;
    assert_eq!(bad.h("access-control-allow-origin"), None);
    // rate limits file-system routes
    let mut last = 0;
    for _ in 0..260 {
        last = inj(&srv, "GET", "/api/host/dirs")
            .header("host", "127.0.0.1:8080")
            .ip("10.9.9.9")
            .send()
            .await
            .status;
        if last == 429 {
            break;
        }
    }
    assert_eq!(last, 429);
    srv.close().await;
    let _ = Path::new("");
}
