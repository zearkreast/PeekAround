//! Mirrors packages/server/test/remote.test.ts "MEGA inspect + import (mocked client)": the import
//! job (`remote/import.ts`) and the inspect route against a mocked MEGA backend, like the TS test.

mod common;

use async_trait::async_trait;
use bytes::Bytes;
use common::*;
use pa_core::{JobStatus, JobType, ServerEvent};
use pa_mega::{ByteStream, MegaError, MegaLink, MegaNode};
use pa_remote::{routes, MegaBackend};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const K22: &str = "AbCdEfGhIjKlMnOpQrStUv";
const K43: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCd";

#[derive(Clone)]
enum Data {
    Bytes(Vec<u8>),
    Fail(MegaError),
    Slow,
}

enum Spec {
    File(&'static str, Data),
    Dir(&'static str, Vec<Spec>),
}

fn bytes_of(seed: u8, n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| seed.wrapping_mul(31).wrapping_add((i % 251) as u8))
        .collect()
}

struct Mock {
    trees: HashMap<String, MegaNode>,
    data: HashMap<String, Data>,
}

static NID: AtomicU32 = AtomicU32::new(0);

fn build(s: &Spec, data: &mut HashMap<String, Data>) -> MegaNode {
    let id = format!("F{:07}", NID.fetch_add(1, Ordering::SeqCst));
    match s {
        Spec::Dir(name, kids) => MegaNode {
            id,
            name: name.to_string(),
            size: 0,
            directory: true,
            timestamp: Some(1_700_000_000_000),
            children: kids.iter().map(|k| build(k, data)).collect(),
            fa: None,
            key: None,
        },
        Spec::File(name, d) => {
            let size = match d {
                Data::Bytes(b) => b.len() as u64,
                Data::Fail(_) => 10,
                Data::Slow => 50_000_000,
            };
            data.insert(id.clone(), d.clone());
            MegaNode {
                id,
                name: name.to_string(),
                size,
                directory: false,
                timestamp: Some(1_600_000_000_000),
                children: vec![],
                fa: None,
                key: None,
            }
        }
    }
}

fn find<'a>(n: &'a MegaNode, name: &str) -> &'a MegaNode {
    fn walk<'a>(n: &'a MegaNode, name: &str) -> Option<&'a MegaNode> {
        if n.name == name {
            return Some(n);
        }
        n.children.iter().find_map(|c| walk(c, name))
    }
    walk(n, name).unwrap()
}

#[async_trait]
impl MegaBackend for Mock {
    async fn load(&self, link: &MegaLink) -> Result<MegaNode, MegaError> {
        match link.id.as_str() {
            "Quota123" => Err(MegaError::Api(pa_mega::error::EOVERQUOTA)),
            "Gone1234" => Err(MegaError::Api(pa_mega::error::ENOENT)),
            id => self
                .trees
                .get(id)
                .cloned()
                .ok_or(MegaError::Api(pa_mega::error::ENOENT)),
        }
    }
    fn read(
        &self,
        _: &MegaLink,
        _: &MegaNode,
        _: u64,
        _: u64,
        _: CancellationToken,
    ) -> Result<ByteStream, MegaError> {
        Err(MegaError::Other("not used".into()))
    }
    fn download(
        &self,
        _link: &MegaLink,
        node: &MegaNode,
        cancel: CancellationToken,
    ) -> Result<ByteStream, MegaError> {
        match self.data.get(&node.id).cloned() {
            Some(Data::Bytes(b)) => Ok(Box::pin(futures_util::stream::iter(
                b.chunks(700)
                    .map(|c| Ok(Bytes::copy_from_slice(c)))
                    .collect::<Vec<_>>(),
            ))),
            Some(Data::Fail(e)) => Ok(Box::pin(futures_util::stream::once(async move { Err(e) }))),
            Some(Data::Slow) => Ok(Box::pin(futures_util::stream::unfold(
                cancel,
                |c| async move {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(20)) => Some((Ok(Bytes::from(vec![1u8; 1024])), c)),
                        _ = c.cancelled() => Some((Err(MegaError::Aborted), c)),
                    }
                },
            ))),
            None => Err(MegaError::NodeNotFound),
        }
    }
    async fn attr(
        &self,
        _: &MegaLink,
        _: &MegaNode,
        _: u32,
        _: &CancellationToken,
    ) -> Result<Option<Vec<u8>>, MegaError> {
        Ok(None)
    }
}

fn url(id: &str) -> String {
    format!("https://mega.nz/folder/{id}#{K22}")
}
fn file_url(id: &str) -> String {
    format!("https://mega.nz/file/{id}#{K43}")
}

fn parts_under(d: &Path, out: &mut Vec<String>) {
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        if e.path().is_dir() {
            parts_under(&e.path(), out);
        } else if e.file_name().to_string_lossy().ends_with(".part") {
            out.push(e.file_name().to_string_lossy().into());
        }
    }
}

fn ls(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into())
        .collect();
    v.sort();
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inspect_and_import() {
    let zip = write_zip(&[("inner/one.jpg", bytes_of(9, 300))]);
    let mut data = HashMap::new();
    let mut trees = HashMap::new();
    use Data::*;
    use Spec::*;
    trees.insert(
        "Holiday1".to_string(),
        build(
            &Dir(
                "Holiday",
                vec![
                    File("a.jpg", Bytes(bytes_of(1, 1000))),
                    Dir(
                        "Sub",
                        vec![
                            File("b.mp4", Bytes(bytes_of(2, 2000))),
                            File("notes.txt", Bytes(bytes_of(3, 10))),
                        ],
                    ),
                    File("z.zip", Bytes(zip.clone())),
                ],
            ),
            &mut data,
        ),
    );
    trees.insert(
        "Single12".to_string(),
        build(&File("clip.mp4", Bytes(bytes_of(4, 500))), &mut data),
    );
    trees.insert(
        "Slowwww1".to_string(),
        build(&Dir("Slow", vec![File("big.mp4", Slow)]), &mut data),
    );
    trees.insert(
        "Mixed123".to_string(),
        build(
            &Dir(
                "Mixed",
                vec![
                    File("ok.jpg", Bytes(bytes_of(6, 100))),
                    File("bad.zip", Bytes(b"this is not a zip file".to_vec())),
                    Dir(
                        "Deep",
                        vec![File(
                            "lost.jpg",
                            Fail(MegaError::Network("ECONNRESET".into())),
                        )],
                    ),
                    File("z.zip", Bytes(zip.clone())),
                ],
            ),
            &mut data,
        ),
    );
    trees.insert(
        "AllBad12".to_string(),
        build(
            &Dir(
                "AllBad",
                vec![
                    Dir(
                        "Sub",
                        vec![File("x.jpg", Fail(MegaError::Network("ECONNRESET".into())))],
                    ),
                    File("y.zip", Bytes(b"nope".to_vec())),
                ],
            ),
            &mut data,
        ),
    );
    trees.insert(
        "Broken12".to_string(),
        build(
            &Dir(
                "Broken",
                vec![File("q.jpg", Fail(MegaError::Quota { seconds: Some(600) }))],
            ),
            &mut data,
        ),
    );
    let mock = Arc::new(Mock { trees, data });
    let env = start("import", mock.clone()).await;
    let (lib, svc) = (env.lib.clone(), env.svc.clone());
    let root = env.tmp.join("lib");
    std::fs::create_dir_all(root.join("Target")).unwrap();
    let lf = lib.add_local("Lib", &root).to_string();
    let import = |body: Value| routes::import(&svc, &body);

    // ── inspects a folder link
    let ins = serde_json::to_value(
        routes::inspect(&svc, &json!({ "url": url("Holiday1") }))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (
            ins["provider"].as_str(),
            ins["kind"].as_str(),
            ins["name"].as_str(),
            ins["truncated"].as_bool()
        ),
        (Some("mega"), Some("folder"), Some("Holiday"), Some(false))
    );
    assert_eq!(ins["size"], 1000 + 2000 + 10 + zip.len());
    let shape: Vec<Value> = ins["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| json!([n["path"], n["dir"], n["kind"]]))
        .collect();
    assert_eq!(
        shape,
        vec![
            json!(["", true, null]),
            json!(["Sub", true, null]),
            json!(["a.jpg", false, "image"]),
            json!(["z.zip", false, "archive"]),
            json!(["Sub/b.mp4", false, "video"]),
            json!(["Sub/notes.txt", false, null])
        ]
    );
    assert_eq!(
        ins["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["path"] == "Sub")
            .unwrap()["size"],
        2010
    );
    let one = serde_json::to_value(
        routes::inspect(&svc, &json!({ "url": file_url("Single12") }))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (
            one["kind"].as_str(),
            one["name"].as_str(),
            one["size"].as_u64()
        ),
        (Some("file"), Some("clip.mp4"), Some(500))
    );
    assert_eq!(one["nodes"][0]["path"], "");

    // ── reports bad links and MEGA errors clearly
    assert_eq!(
        routes::inspect(&svc, &json!({ "url": "https://example.com/x" }))
            .await
            .unwrap_err()
            .status,
        400
    );
    let q = routes::inspect(&svc, &json!({ "url": url("Quota123") }))
        .await
        .unwrap_err();
    assert_eq!(q.status, 429);
    assert!(
        q.message
            .starts_with("MEGA bandwidth quota exceeded, try again later"),
        "{}",
        q.message
    );
    let g = routes::inspect(&svc, &json!({ "url": url("Gone1234") }))
        .await
        .unwrap_err();
    assert_eq!(
        (g.status, g.message.as_str()),
        (404, "MEGA link not found (removed, or the link is wrong)")
    );
    assert_eq!(
        import(json!({ "url": url("Holiday1"), "mode": "extract" }))
            .unwrap_err()
            .message,
        "folderId required"
    );
    assert_eq!(
        import(json!({ "url": url("Holiday1"), "mode": "weird", "folderId": "1" }))
            .unwrap_err()
            .message,
        "mode must be 'extract' or 'temp'"
    );
    assert_eq!(
        import(json!({ "url": url("Holiday1"), "mode": "extract", "folderId": lf, "nodes": [] }))
            .unwrap_err()
            .message,
        "Nothing selected"
    );
    assert_eq!(
        import(json!({ "url": 7, "mode": "temp" }))
            .unwrap_err()
            .message,
        "Not a MEGA link"
    );

    // ── imports selected nodes into a folder, preserving the tree, with progress
    let h = &mock.trees["Holiday1"];
    env.events.take();
    let job = import(json!({ "url": url("Holiday1"), "nodes": [find(h, "a.jpg").id, find(h, "Sub").id], "mode": "extract", "folderId": lf, "dir": "Target" })).unwrap();
    assert_eq!(job.kind, JobType::Transfer);
    let fin = env.jobs.wait(&job.id).await.unwrap();
    assert_eq!(fin.status, JobStatus::Done, "{:?}", fin.error);
    assert_eq!(fin.title, "Holiday");
    assert_eq!(fin.bytes.map(|b| (b.done, b.total)), Some((3010, 3010)));
    assert_eq!(fin.files.map(|b| (b.done, b.total)), Some((3, 3)));
    let res = fin.result.clone().unwrap();
    assert_eq!(
        (res.folder_id.as_str(), res.dir.as_str(), res.name.as_str()),
        (lf.as_str(), "Target", "Holiday")
    );
    assert!(env.events.snapshot().iter().any(|e| matches!(e, ServerEvent::Job { job } if job.status == JobStatus::Running && job.bytes.is_some_and(|b| b.total == 3010))));
    let base = root.join("Target/Holiday");
    assert_eq!(
        std::fs::read(base.join("a.jpg")).unwrap(),
        bytes_of(1, 1000)
    );
    assert_eq!(
        std::fs::read(base.join("Sub/b.mp4")).unwrap(),
        bytes_of(2, 2000)
    );
    assert!(base.join("Sub/notes.txt").exists());
    assert!(!base.join("z.zip").exists());
    let mtime = std::fs::metadata(base.join("a.jpg"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    assert_eq!(mtime, 1_600_000_000_000);
    let mut names: Vec<String> = lib
        .rows_of(lf.parse().unwrap())
        .iter()
        .filter(|r| r.dir.starts_with("Target/Holiday"))
        .map(|r| r.name.clone())
        .collect();
    names.sort();
    assert_eq!(names, vec!["a.jpg", "b.mp4"]);
    // a second import of the same folder lands next to it
    let again = import(json!({ "url": url("Holiday1"), "nodes": [find(h, "a.jpg").id], "mode": "extract", "folderId": lf, "dir": "Target" })).unwrap();
    assert_eq!(
        env.jobs.wait(&again.id).await.unwrap().result.unwrap().name,
        "Holiday (1)"
    );

    // ── handles zips inside the link per spec.zip (default from settings → keep)
    let zid = find(h, "z.zip").id.clone();
    let run = |zip_mode: Option<&'static str>, dir: &'static str| {
        let (svc, jobs, lf, root, zid) = (
            svc.clone(),
            env.jobs.clone(),
            lf.clone(),
            root.clone(),
            zid.clone(),
        );
        async move {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            let mut spec = json!({ "url": url("Holiday1"), "nodes": [zid], "mode": "extract", "folderId": lf, "dir": dir });
            if let Some(z) = zip_mode {
                spec["zip"] = json!(z);
            }
            let j = routes::import(&svc, &spec).unwrap();
            let fin = jobs.wait(&j.id).await.unwrap();
            assert_eq!(fin.status, JobStatus::Done, "{:?}", fin.error);
            root.join(dir).join("Holiday")
        }
    };
    let ex = run(Some("extract"), "Z1").await;
    assert_eq!(
        std::fs::read(ex.join("z/inner/one.jpg")).unwrap(),
        bytes_of(9, 300)
    );
    assert!(!ex.join("z.zip").exists());
    let keep = run(None, "Z2").await;
    assert_eq!(std::fs::read(keep.join("z.zip")).unwrap(), zip);
    let before = lib.temps().len();
    let t = run(Some("temp"), "Z3").await;
    assert!(!t.join("z.zip").exists());
    let temps = lib.temps();
    assert_eq!(temps.len(), before + 1);
    assert_eq!(temps.last().unwrap().source_name.as_deref(), Some("z.zip"));
    // the zipAction setting is the default
    *lib.zip_action.lock().unwrap() = "extract".into();
    let ex2 = run(None, "Z4").await;
    assert!(ex2.join("z/inner/one.jpg").exists());
    *lib.zip_action.lock().unwrap() = "keep".into();

    // ── temp mode creates a temp session named after the link
    let j = import(json!({ "url": file_url("Single12"), "mode": "temp" })).unwrap();
    let fin = env.jobs.wait(&j.id).await.unwrap();
    assert_eq!(fin.status, JobStatus::Done);
    let tid: i64 = fin.result.unwrap().folder_id.parse().unwrap();
    let t = lib.temps().into_iter().find(|f| f.id == tid).unwrap();
    assert_eq!(
        (t.name.as_str(), t.temp, t.source_name.as_deref()),
        ("clip.mp4", true, Some("clip.mp4"))
    );
    assert_eq!(
        std::fs::read(Path::new(&t.path).join("clip.mp4")).unwrap(),
        bytes_of(4, 500)
    );
    assert_eq!(
        lib.rows_of(tid)
            .iter()
            .map(|r| r.name.clone())
            .collect::<Vec<_>>(),
        vec!["clip.mp4"]
    );

    // ── cancels a running import (no .part left, empty temp session dropped)
    let temps_before = lib.temps().len();
    let j = import(json!({ "url": url("Slowwww1"), "mode": "temp" })).unwrap();
    until(
        || {
            env.jobs
                .get(&j.id)
                .and_then(|j| j.bytes)
                .filter(|b| b.done >= 4096)
                .map(|_| ())
        },
        5000,
    )
    .await;
    env.jobs.cancel(&j.id);
    let fin = env.jobs.wait(&j.id).await.unwrap();
    assert_eq!(fin.status, JobStatus::Canceled);
    assert_eq!(lib.temps().len(), temps_before);
    let mut parts = Vec::new();
    parts_under(&env.tmp, &mut parts);
    assert!(parts.is_empty(), "{parts:?}");

    // ── a transfer does not wait for a running video job (separate lane)
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let blocker = env.jobs.enqueue(
        JobType::Slideshow,
        "blocker",
        pa_jobs::runner(move |_jc| async move {
            let _ = rx.await;
            Ok(None)
        }),
    );
    std::fs::create_dir_all(root.join("Lane")).unwrap();
    let j = import(
        json!({ "url": file_url("Single12"), "mode": "extract", "folderId": lf, "dir": "Lane" }),
    )
    .unwrap();
    assert_eq!(env.jobs.wait(&j.id).await.unwrap().status, JobStatus::Done);
    assert_eq!(
        env.jobs.get(&blocker.id).unwrap().status,
        JobStatus::Running
    );
    let _ = tx.send(());
    env.jobs.wait(&blocker.id).await;

    // ── quota errors during a download fail the job with a friendly message
    let j = import(
        json!({ "url": url("Broken12"), "mode": "extract", "folderId": lf, "dir": "Target" }),
    )
    .unwrap();
    let fin = env.jobs.wait(&j.id).await.unwrap();
    assert_eq!(fin.status, JobStatus::Error);
    assert_eq!(
        fin.error.as_deref(),
        Some("MEGA bandwidth quota exceeded, try again later (retry in about 10 min)")
    );
    assert!(!root.join("Target/Broken").exists());

    // ── partial failures: other files and zips still land, "n of m", no empty folders or leftovers
    std::fs::create_dir_all(root.join("P1")).unwrap();
    let sessions_before = ls(&env.tmp.join("temp")).len();
    let j = import(json!({ "url": url("Mixed123"), "mode": "extract", "folderId": lf, "dir": "P1", "zip": "extract" })).unwrap();
    let fin = env.jobs.wait(&j.id).await.unwrap();
    assert_eq!(fin.status, JobStatus::Error);
    assert!(
        fin.error
            .as_deref()
            .unwrap()
            .starts_with("2 of 4 files could not be imported: "),
        "{:?}",
        fin.error
    );
    let res = fin.result.unwrap();
    assert_eq!((res.dir.as_str(), res.name.as_str()), ("P1", "Mixed"));
    let base = root.join("P1/Mixed");
    assert_eq!(
        std::fs::read(base.join("ok.jpg")).unwrap(),
        bytes_of(6, 100)
    );
    assert_eq!(
        std::fs::read(base.join("z/inner/one.jpg")).unwrap(),
        bytes_of(9, 300)
    );
    assert_eq!(ls(&base), vec!["ok.jpg", "z"]); // no 'bad' folder, no empty 'Deep', no .part
                                                // temp zip mode: a broken zip leaves no orphan session dir
    std::fs::create_dir_all(root.join("P2")).unwrap();
    let bad = find(&mock.trees["Mixed123"], "bad.zip").id.clone();
    let t = import(json!({ "url": url("Mixed123"), "nodes": [bad], "mode": "extract", "folderId": lf, "dir": "P2", "zip": "temp" })).unwrap();
    assert_eq!(env.jobs.wait(&t.id).await.unwrap().status, JobStatus::Error);
    assert!(ls(&root.join("P2")).is_empty());
    assert!(
        ls(&env.tmp.join("temp"))
            .iter()
            .filter(|n| *n != "job-assets")
            .count()
            <= sessions_before
    );

    // ── a link where everything fails leaves no empty folder (extract and temp)
    std::fs::create_dir_all(root.join("P3")).unwrap();
    let j = import(json!({ "url": url("AllBad12"), "mode": "extract", "folderId": lf, "dir": "P3", "zip": "extract" })).unwrap();
    let fin = env.jobs.wait(&j.id).await.unwrap();
    assert_eq!(fin.status, JobStatus::Error);
    assert!(
        fin.error
            .as_deref()
            .unwrap()
            .starts_with("2 of 2 files could not be imported"),
        "{:?}",
        fin.error
    );
    assert!(fin.result.is_none());
    assert!(ls(&root.join("P3")).is_empty());
    let temps = lib.temps().len();
    let t = import(json!({ "url": url("AllBad12"), "mode": "temp", "zip": "extract" })).unwrap();
    assert_eq!(env.jobs.wait(&t.id).await.unwrap().status, JobStatus::Error);
    assert_eq!(lib.temps().len(), temps);

    svc.close().await;
    env.jobs.close().await;
    let _ = std::fs::remove_dir_all(&env.tmp);
}
