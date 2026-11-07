//! What the viewer, the selection actions and drag & drop need from a backend beyond browsing
//! (`Backend::ops`): media details, display images, favourites, file operations, albums,
//! uploads, downloads, and where a video decoder can read an item from.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pa_core::{Album, DirTarget, FileRef, LibrariesResponse, MediaDetails, MediaItem};

use crate::backend::{BResult, BackendError, BoxFut, ThumbData};

/// Images the viewer asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageSize {
    /// grid / filmstrip thumbnail (320 / 640 / 1600)
    Thumb(u32),
    /// zoomed-in display rendition (≤ 4096, never the raw original)
    Zoom,
    /// video filmstrip sprite: `n` frames of height `h`
    Filmstrip(u32, u32),
}

/// Where a decoder (ffmpeg) reads a media file from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSource {
    /// a local file (embedded / same-machine server)
    Path(PathBuf),
    /// an URL of the server (original bytes, ranged)
    Url(String),
}

impl MediaSource {
    /// the ffmpeg `-i` argument
    pub fn as_input(&self) -> String {
        match self {
            MediaSource::Path(p) => p.to_string_lossy().into_owned(),
            MediaSource::Url(u) => u.clone(),
        }
    }
}

/// Media operations (all futures are `Send + 'static`; they run on the tokio runtime).
pub trait MediaOps: Send + Sync {
    /// `GET /api/media/:id`
    fn media(&self, id: &str) -> BoxFut<MediaDetails>;
    /// an image of an item
    fn image(&self, id: &str, size: ImageSize) -> BoxFut<ThumbData>;
    /// favourite / unfavourite
    fn set_favorite(&self, ids: Vec<String>, favorite: bool) -> BoxFut<()>;
    /// delete (to the trash where the server has one)
    fn remove(&self, refs: Vec<FileRef>) -> BoxFut<()>;
    /// rename one file
    fn rename(&self, file: FileRef, name: String) -> BoxFut<()>;
    /// move / copy files into a directory
    fn move_files(&self, refs: Vec<FileRef>, to: DirTarget, copy: bool) -> BoxFut<()>;
    /// libraries, temp sessions, linked phones (action conditions, folder picker)
    fn libraries(&self) -> BoxFut<LibrariesResponse>;
    /// albums
    fn albums(&self) -> BoxFut<Vec<Album>>;
    /// add items to an album
    fn add_to_album(&self, album: String, ids: Vec<String>) -> BoxFut<()>;
    /// new album with items
    fn create_album(&self, name: String, ids: Vec<String>) -> BoxFut<Album>;
    /// upload a local file into a directory (`rel`: its path inside the drop, for folders)
    fn upload(&self, file: PathBuf, to: DirTarget, rel: String) -> BoxFut<()>;
    /// save the original of one item / a zip of several into `dest` (a file path)
    fn download(&self, ids: Vec<String>, dest: PathBuf) -> BoxFut<PathBuf>;
    /// where the video / audio decoder reads `item` from (`path`: MediaDetails.path)
    fn source(&self, item: &MediaItem, path: Option<&str>) -> Option<MediaSource>;
    /// the original's URL (copy link)
    fn file_url(&self, id: &str) -> Option<String>;
}

fn unsupported<T: Send + 'static>() -> BoxFut<T> {
    Box::pin(async {
        Err(BackendError {
            status: 0,
            message: "not supported".into(),
        })
    })
}

/// A unique file name in `dir` (`name`, `name (1)`, …).
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(p)
}

// ───────── HTTP ─────────

#[cfg(feature = "http")]
mod http {
    use super::*;
    use crate::backend::HttpBackend;

    fn berr(e: pa_client::Error) -> BackendError {
        BackendError {
            status: e.status(),
            message: e.to_string(),
        }
    }

    impl MediaOps for HttpBackend {
        fn media(&self, id: &str) -> BoxFut<MediaDetails> {
            let (c, id) = (self.client().clone(), id.to_string());
            Box::pin(async move { c.media(&id).await.map_err(berr) })
        }
        fn image(&self, id: &str, size: ImageSize) -> BoxFut<ThumbData> {
            let c = self.client().clone();
            let url = match size {
                ImageSize::Thumb(s) => c.thumb_url(id, s),
                ImageSize::Zoom => c.zoom_url(id),
                ImageSize::Filmstrip(n, h) => c.filmstrip_url(id, n, h),
            };
            Box::pin(async move {
                c.get_bytes(&url)
                    .await
                    .map(ThumbData::Encoded)
                    .map_err(berr)
            })
        }
        fn set_favorite(&self, ids: Vec<String>, favorite: bool) -> BoxFut<()> {
            let c = self.client().clone();
            Box::pin(async move { c.set_favorite(&ids, favorite).await.map_err(berr) })
        }
        fn remove(&self, refs: Vec<FileRef>) -> BoxFut<()> {
            let c = self.client().clone();
            Box::pin(async move { c.remove(&refs).await.map_err(berr) })
        }
        fn rename(&self, file: FileRef, name: String) -> BoxFut<()> {
            let c = self.client().clone();
            Box::pin(async move { c.rename(&file, &name).await.map(|_| ()).map_err(berr) })
        }
        fn move_files(&self, refs: Vec<FileRef>, to: DirTarget, copy: bool) -> BoxFut<()> {
            let c = self.client().clone();
            Box::pin(async move {
                c.move_files(&refs, &to, copy)
                    .await
                    .map(|_| ())
                    .map_err(berr)
            })
        }
        fn libraries(&self) -> BoxFut<LibrariesResponse> {
            let c = self.client().clone();
            Box::pin(async move { c.libraries().await.map_err(berr) })
        }
        fn albums(&self) -> BoxFut<Vec<Album>> {
            let c = self.client().clone();
            Box::pin(async move { c.albums().await.map_err(berr) })
        }
        fn add_to_album(&self, album: String, ids: Vec<String>) -> BoxFut<()> {
            let c = self.client().clone();
            Box::pin(async move { c.add_to_album(&album, &ids).await.map(|_| ()).map_err(berr) })
        }
        fn create_album(&self, name: String, ids: Vec<String>) -> BoxFut<Album> {
            let c = self.client().clone();
            Box::pin(async move { c.create_album(&name, &ids).await.map_err(berr) })
        }
        fn upload(&self, file: PathBuf, to: DirTarget, rel: String) -> BoxFut<()> {
            let c = self.client().clone();
            Box::pin(async move {
                let mtime = std::fs::metadata(&file)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64);
                let opts = pa_client::UploadOptions {
                    folder_id: to.folder_id,
                    dir: to.dir,
                    path: rel,
                    zip: None,
                    mtime,
                };
                c.upload_path(&file, &opts).await.map(|_| ()).map_err(berr)
            })
        }
        fn download(&self, ids: Vec<String>, dest: PathBuf) -> BoxFut<PathBuf> {
            let c = self.client().clone();
            Box::pin(async move {
                let url = if ids.len() == 1 {
                    c.file_url(&ids[0], true)
                } else {
                    c.download_url(&ids, None)
                };
                let bytes = c.get_bytes(&url).await.map_err(berr)?;
                let d2 = dest.clone();
                tokio::task::spawn_blocking(move || std::fs::write(&d2, &bytes))
                    .await
                    .map_err(|e| BackendError {
                        status: 0,
                        message: e.to_string(),
                    })?
                    .map_err(|e| BackendError {
                        status: 0,
                        message: format!("{}: {e}", dest.display()),
                    })?;
                Ok(dest)
            })
        }
        fn source(&self, item: &MediaItem, path: Option<&str>) -> Option<MediaSource> {
            // same machine (desktop app / local server): ffmpeg reads the file itself
            if let Some(p) = path.filter(|p| Path::new(p).is_absolute() && Path::new(p).is_file()) {
                return Some(MediaSource::Path(PathBuf::from(p)));
            }
            Some(MediaSource::Url(self.client().file_url(&item.id, false)))
        }
        fn file_url(&self, id: &str) -> Option<String> {
            Some(self.client().file_url(id, false))
        }
    }
}

// ───────── demo ─────────

impl MediaOps for crate::backend::DemoBackend {
    fn media(&self, id: &str) -> BoxFut<MediaDetails> {
        let id = id.to_string();
        let item = self.demo_item(&id);
        Box::pin(async move {
            let item = item.ok_or(BackendError {
                status: 404,
                message: "not found".into(),
            })?;
            Ok(MediaDetails {
                path: format!("/demo/{}", item.name),
                item,
                exif: None,
                video: None,
                gps: None,
                place: None,
                camera: Some(pa_core::CameraInfo {
                    make: Some("Demo".into()),
                    model: Some("Camera".into()),
                    lens: None,
                }),
                tz_offset: None,
            })
        })
    }
    fn image(&self, id: &str, size: ImageSize) -> BoxFut<ThumbData> {
        use crate::backend::Backend;
        match size {
            ImageSize::Thumb(s) => self.thumb(id.to_string(), s),
            ImageSize::Zoom => self.thumb(id.to_string(), 2400),
            ImageSize::Filmstrip(..) => unsupported(),
        }
    }
    fn set_favorite(&self, _ids: Vec<String>, _favorite: bool) -> BoxFut<()> {
        Box::pin(async { Ok(()) })
    }
    fn remove(&self, _refs: Vec<FileRef>) -> BoxFut<()> {
        unsupported()
    }
    fn rename(&self, _file: FileRef, _name: String) -> BoxFut<()> {
        unsupported()
    }
    fn move_files(&self, _r: Vec<FileRef>, _t: DirTarget, _c: bool) -> BoxFut<()> {
        unsupported()
    }
    fn libraries(&self) -> BoxFut<LibrariesResponse> {
        Box::pin(async {
            Ok(LibrariesResponse {
                libraries: vec![],
                temp: vec![],
                devices: vec![],
            })
        })
    }
    fn albums(&self) -> BoxFut<Vec<Album>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn add_to_album(&self, _a: String, _ids: Vec<String>) -> BoxFut<()> {
        unsupported()
    }
    fn create_album(&self, _n: String, _ids: Vec<String>) -> BoxFut<Album> {
        unsupported()
    }
    fn upload(&self, _f: PathBuf, _t: DirTarget, _r: String) -> BoxFut<()> {
        unsupported()
    }
    fn download(&self, _ids: Vec<String>, _dest: PathBuf) -> BoxFut<PathBuf> {
        unsupported()
    }
    fn source(&self, _item: &MediaItem, _path: Option<&str>) -> Option<MediaSource> {
        None
    }
    fn file_url(&self, _id: &str) -> Option<String> {
        None
    }
}

/// `Arc` helper: the ops of a backend, if it has any.
pub fn ops_of(b: &Arc<dyn crate::backend::Backend>) -> Option<&dyn MediaOps> {
    b.ops()
}

/// result alias
pub type OpResult<T> = BResult<T>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join(format!("pa-ui-uniq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "a.jpg"), dir.join("a.jpg"));
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.jpg"), dir.join("a (1).jpg"));
        std::fs::write(dir.join("a (1).jpg"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.jpg"), dir.join("a (2).jpg"));
        std::fs::write(dir.join("noext"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "noext"), dir.join("noext (1)"));
        std::fs::remove_dir_all(dir).ok();
    }
}
