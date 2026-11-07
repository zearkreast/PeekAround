//! Size-capped LRU disk cache for bytes fetched from MEGA folders (mirrors `remote/cache.ts`):
//! whole originals that were viewed (images, small files) so zooming / re-opening never downloads
//! them again. Entries are keyed like thumbnails (sha1 of `abs|mtime|size`); recency = last
//! access, kept in memory (seeded from file mtimes at startup, refreshed with utimes on access).

use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;
use tokio::sync::OnceCell;

struct Entry {
    size: u64,
    at: SystemTime,
}

pub struct RemoteCache {
    pub dir: PathBuf,
    cap: AtomicU64,
    st: Mutex<(HashMap<String, Entry>, u64)>,
    ready: OnceCell<()>,
}

impl RemoteCache {
    /// `new RemoteCache(dir, capBytes)` (`Settings.remoteCacheMb * 1 MiB`)
    pub fn new(dir: impl Into<PathBuf>, cap_bytes: u64) -> Self {
        let dir = dir.into();
        let _ = std::fs::create_dir_all(&dir);
        Self {
            dir,
            cap: AtomicU64::new(cap_bytes),
            st: Mutex::new((HashMap::new(), 0)),
            ready: OnceCell::new(),
        }
    }

    pub fn cap_bytes(&self) -> u64 {
        self.cap.load(Ordering::Relaxed)
    }

    /// sha1(`abs|mtime|size`) in hex
    pub fn key(abs: &str, mtime: i64, size: i64) -> String {
        hex::encode(Sha1::digest(format!("{abs}|{mtime}|{size}").as_bytes()))
    }

    pub fn file(&self, key: &str) -> PathBuf {
        self.dir.join(&key[..2.min(key.len())]).join(key)
    }

    async fn load(&self) {
        self.ready
            .get_or_init(|| async {
                let mut found = Vec::new();
                if let Ok(mut rd) = tokio::fs::read_dir(&self.dir).await {
                    while let Ok(Some(sub)) = rd.next_entry().await {
                        let Ok(mut rd2) = tokio::fs::read_dir(sub.path()).await else {
                            continue;
                        };
                        while let Ok(Some(e)) = rd2.next_entry().await {
                            let name = e.file_name().to_string_lossy().into_owned();
                            let p = e.path();
                            if name.ends_with(".tmp") {
                                let _ = tokio::fs::remove_file(&p).await;
                                continue;
                            }
                            if let Ok(m) = tokio::fs::metadata(&p).await {
                                if m.is_file() {
                                    found.push((
                                        name,
                                        m.len(),
                                        m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                                    ));
                                }
                            }
                        }
                    }
                }
                let mut st = self.st.lock().unwrap();
                for (k, size, at) in found {
                    st.1 += size;
                    st.0.insert(k, Entry { size, at });
                }
            })
            .await;
    }

    /// path of a complete cached entry (marks it as recently used), or None
    pub async fn get(&self, key: &str) -> Option<PathBuf> {
        self.load().await;
        if !self.st.lock().unwrap().0.contains_key(key) {
            return None;
        }
        let f = self.file(key);
        if tokio::fs::metadata(&f).await.is_err() {
            let mut st = self.st.lock().unwrap();
            if let Some(e) = st.0.remove(key) {
                st.1 -= e.size;
            }
            return None;
        }
        let now = SystemTime::now();
        if let Some(e) = self.st.lock().unwrap().0.get_mut(key) {
            e.at = now;
        }
        let f2 = f.clone();
        tokio::task::spawn_blocking(move || {
            if let Ok(h) = std::fs::OpenOptions::new().write(true).open(&f2) {
                let _ = h.set_times(
                    std::fs::FileTimes::new()
                        .set_modified(now)
                        .set_accessed(now),
                );
            }
        });
        Some(f)
    }

    /// whether an entry of `size` bytes may be cached at all (≤ ¼ of the cap)
    pub fn accepts(&self, size: u64) -> bool {
        let cap = self.cap_bytes();
        cap > 0 && size as f64 <= cap as f64 / 4.0
    }

    /// a temp file next to the cache (same file system → atomic rename on commit)
    pub fn tmp_file(&self, key: &str) -> PathBuf {
        self.dir.join(format!(
            "{key}.{}.{:08x}.tmp",
            std::process::id(),
            rand::random::<u32>()
        ))
    }

    /// move a completely written temp file into the cache and evict old entries over the cap
    pub async fn commit(&self, key: &str, tmp: &Path) -> std::io::Result<PathBuf> {
        self.load().await;
        let size = tokio::fs::metadata(tmp).await?.len();
        let f = self.file(key);
        if let Some(p) = f.parent() {
            tokio::fs::create_dir_all(p).await?;
        }
        tokio::fs::rename(tmp, &f).await?;
        {
            let mut st = self.st.lock().unwrap();
            if let Some(old) = st.0.remove(key) {
                st.1 -= old.size;
            }
            st.0.insert(
                key.to_string(),
                Entry {
                    size,
                    at: SystemTime::now(),
                },
            );
            st.1 += size;
        }
        self.evict(Some(key)).await;
        Ok(f)
    }

    pub async fn evict(&self, keep: Option<&str>) {
        let gone: Vec<String> = {
            let mut st = self.st.lock().unwrap();
            let cap = self.cap_bytes();
            if st.1 <= cap {
                return;
            }
            let mut order: Vec<(String, SystemTime, u64)> =
                st.0.iter()
                    .map(|(k, e)| (k.clone(), e.at, e.size))
                    .collect();
            order.sort_by_key(|x| x.1);
            let mut gone = Vec::new();
            for (k, _, size) in order {
                if st.1 <= cap {
                    break;
                }
                if Some(k.as_str()) == keep {
                    continue;
                }
                st.0.remove(&k);
                st.1 -= size;
                gone.push(k);
            }
            gone
        };
        for k in gone {
            let _ = tokio::fs::remove_file(self.file(&k)).await;
        }
    }

    /// `PATCH /api/settings {remoteCacheMb}`
    pub async fn set_cap(&self, bytes: u64) {
        self.cap.store(bytes, Ordering::Relaxed);
        self.load().await;
        self.evict(None).await;
    }

    /// `GET /api/cache` → `remoteBytes`
    pub async fn size_bytes(&self) -> u64 {
        self.load().await;
        self.st.lock().unwrap().1
    }

    /// `DELETE /api/cache` (remote part)
    pub async fn clear(&self) {
        self.load().await;
        {
            let mut st = self.st.lock().unwrap();
            st.0.clear();
            st.1 = 0;
        }
        let _ = tokio::fs::remove_dir_all(&self.dir).await;
        let _ = tokio::fs::create_dir_all(&self.dir).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lru_eviction() {
        let dir = std::env::temp_dir().join(format!("pa-remote-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let c = RemoteCache::new(&dir, 100);
        assert!(c.accepts(25) && !c.accepts(26));
        let put = |name: &str, n: usize| {
            let t = c.tmp_file(name);
            std::fs::write(&t, vec![0u8; n]).unwrap();
            t
        };
        let ka = RemoteCache::key("remote://mega/x/a.jpg", 1, 2);
        assert_eq!(ka.len(), 40);
        for (k, n) in [("aa1", 40), ("bb2", 40)] {
            let t = put(k, n);
            c.commit(k, &t).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(c.get("aa1").await.is_some()); // aa1 is now the most recent
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let t = put("cc3", 40);
        c.commit("cc3", &t).await.unwrap();
        assert!(c.get("bb2").await.is_none());
        assert!(c.get("aa1").await.is_some() && c.get("cc3").await.is_some());
        assert_eq!(c.size_bytes().await, 80);
        // a fresh instance seeds itself from the files (temp leftovers are removed)
        std::fs::write(dir.join("zz.tmp"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("tm")).unwrap();
        std::fs::write(dir.join("tm").join("tmp1.tmp"), b"x").unwrap();
        let c2 = RemoteCache::new(&dir, 100);
        assert_eq!(c2.size_bytes().await, 80);
        assert!(!dir.join("tm").join("tmp1.tmp").exists());
        c2.set_cap(40).await;
        assert_eq!(c2.size_bytes().await, 40);
        c2.clear().await;
        assert_eq!(c2.size_bytes().await, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
