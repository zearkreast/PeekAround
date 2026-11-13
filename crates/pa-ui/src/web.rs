//! The browser's side of what the native builds do with files and codecs: files the user picked
//! or dropped (a page never sees their paths), downloads, image decoding by the browser's own
//! decoders.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

type Canvas = (web_sys::HtmlCanvasElement, web_sys::CanvasRenderingContext2d);

thread_local! {
    static FILES: RefCell<Vec<(PathBuf, web_sys::File)>> = const { RefCell::new(Vec::new()) };
    static CANVAS: RefCell<Option<Canvas>> = const { RefCell::new(None) };
}

pub(crate) fn js_err(e: JsValue) -> String {
    e.as_string()
        .or_else(|| js_sys::Reflect::get(&e, &"message".into()).ok()?.as_string())
        .unwrap_or_else(|| format!("{e:?}"))
}

/// Keep a file of the page and give it a path the UI can pass around like a local one
/// (`/@browser/<n>/<name>`; only [`file`] and [`read`] understand it).
pub fn register(file: web_sys::File) -> PathBuf {
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        let p = PathBuf::from(format!("/@browser/{}/{}", f.len(), file.name()));
        f.push((p.clone(), file));
        p
    })
}

/// Keep the files of a folder the user picked (`webkitRelativePath`: `Folder/sub/file`) under
/// one registry path per top-level folder; returns those folder paths.
pub fn register_tree(files: Vec<web_sys::File>) -> Vec<PathBuf> {
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        let root = PathBuf::from(format!("/@browser/{}", f.len()));
        let mut tops: Vec<PathBuf> = vec![];
        for file in files {
            let rel = js_sys::Reflect::get(&file, &"webkitRelativePath".into())
                .ok()
                .and_then(|v| v.as_string())
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| file.name());
            let p = root.join(&rel);
            let top = root.join(rel.split('/').next().unwrap_or(&rel));
            if !tops.contains(&top) {
                tops.push(top);
            }
            f.push((p, file));
        }
        tops
    })
}

/// Is `path` a folder of [`register_tree`]?
pub fn is_dir(path: &Path) -> bool {
    FILES.with(|f| f.borrow().iter().any(|(p, _)| p != path && p.starts_with(path)))
}

/// The direct entries of a folder of [`register_tree`].
pub fn read_dir(path: &Path) -> Vec<PathBuf> {
    let depth = path.components().count();
    FILES.with(|f| {
        let mut out: Vec<PathBuf> = vec![];
        for (p, _) in f.borrow().iter() {
            if p != path && p.starts_with(path) {
                let child: PathBuf = p.components().take(depth + 1).collect();
                if !out.contains(&child) {
                    out.push(child);
                }
            }
        }
        out
    })
}

/// The file behind a path of [`register`].
pub fn file(path: &Path) -> Option<web_sys::File> {
    FILES.with(|f| {
        f.borrow()
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, f)| f.clone())
    })
}

/// Size in bytes of a registered file.
pub fn size(path: &Path) -> Option<u64> {
    file(path).map(|f| f.size() as u64)
}

/// The bytes of a registered file.
pub async fn read(path: &Path) -> Result<Vec<u8>, String> {
    let f = file(path).ok_or_else(|| format!("{}: not found", path.display()))?;
    let buf = JsFuture::from(f.array_buffer()).await.map_err(js_err)?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

/// The file picker of the browser (`folder`: pick a whole folder; the answer is its path).
pub async fn pick_files(multiple: bool, accept: &str, folder: bool) -> Vec<PathBuf> {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return vec![];
    };
    let Some(input) = doc
        .create_element("input")
        .ok()
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
    else {
        return vec![];
    };
    input.set_type("file");
    input.set_multiple(multiple);
    if !accept.is_empty() {
        input.set_accept(accept);
    }
    if folder {
        let _ = input.set_attribute("webkitdirectory", "");
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let done = Closure::<dyn FnMut()>::new(move || {
        if let Some(t) = tx.borrow_mut().take() {
            let _ = t.send(());
        }
    });
    for ev in ["change", "cancel"] {
        let _ = input.add_event_listener_with_callback(ev, done.as_ref().unchecked_ref());
    }
    input.click();
    let _ = rx.await;
    drop(done);
    let mut files = vec![];
    if let Some(list) = input.files() {
        for i in 0..list.length() {
            if let Some(f) = list.get(i) {
                files.push(f);
            }
        }
    }
    if folder {
        register_tree(files)
    } else {
        files.into_iter().map(register).collect()
    }
}

/// A touch screen without a mouse: the UI lays itself out as on a phone when it is narrow.
pub fn coarse_pointer() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(pointer: coarse)").ok().flatten())
        .is_some_and(|m| m.matches())
}

/// Let the browser download `url` as `name`.
pub fn save_url(url: &str, name: &str) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(a) = doc
        .create_element("a")
        .ok()
        .and_then(|e| e.dyn_into::<web_sys::HtmlAnchorElement>().ok())
    else {
        return;
    };
    a.set_href(url);
    a.set_download(name);
    if let Some(body) = doc.body() {
        let _ = body.append_child(&a);
        a.click();
        let _ = body.remove_child(&a);
    }
}

/// Let the browser download `bytes` as `name`.
pub fn save_bytes(bytes: &[u8], name: &str, mime: &str) {
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes).buffer());
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_buffer_source_sequence_and_options(&parts, &bag) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    save_url(&url, name);
    // the download has its own reference by the time this runs
    slint::Timer::single_shot(std::time::Duration::from_secs(60), move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
}

/// RGBA pixels of a decoded image.
pub struct Decoded {
    /// width
    pub width: u32,
    /// height
    pub height: u32,
    /// RGBA8
    pub pixels: Vec<u8>,
}

fn read_bitmap(bmp: &web_sys::ImageBitmap) -> Result<Decoded, String> {
    let (w, h) = (bmp.width(), bmp.height());
    if w == 0 || h == 0 {
        return Err("empty image".into());
    }
    CANVAS.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            let doc = web_sys::window()
                .and_then(|w| w.document())
                .ok_or("no document")?;
            let canvas = doc
                .create_element("canvas")
                .map_err(js_err)?
                .dyn_into::<web_sys::HtmlCanvasElement>()
                .map_err(|_| "no canvas")?;
            let opts = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&opts, &"willReadFrequently".into(), &JsValue::TRUE);
            let ctx = canvas
                .get_context_with_context_options("2d", &opts)
                .map_err(js_err)?
                .ok_or("no 2d context")?
                .dyn_into::<web_sys::CanvasRenderingContext2d>()
                .map_err(|_| "no 2d context")?;
            *c = Some((canvas, ctx));
        }
        let (canvas, ctx) = c.as_ref().ok_or("no canvas")?;
        // setting the size also clears the canvas
        canvas.set_width(w);
        canvas.set_height(h);
        ctx.draw_image_with_image_bitmap(bmp, 0.0, 0.0)
            .map_err(js_err)?;
        let data = ctx
            .get_image_data(0.0, 0.0, w as f64, h as f64)
            .map_err(js_err)?;
        Ok(Decoded {
            width: w,
            height: h,
            pixels: data.data().0,
        })
    })
}

async fn decode_blob(blob: &web_sys::Blob) -> Result<Decoded, String> {
    let win = web_sys::window().ok_or("no window")?;
    let bmp = JsFuture::from(win.create_image_bitmap_with_blob(blob).map_err(js_err)?)
        .await
        .map_err(js_err)?
        .dyn_into::<web_sys::ImageBitmap>()
        .map_err(|_| "not an image")?;
    let out = read_bitmap(&bmp);
    bmp.close();
    out
}

/// Fetch an image of this server and let the browser decode it (off the UI thread, with its
/// native codecs). The error keeps the HTTP status (0: network / not an image).
pub async fn fetch_image(url: &str) -> Result<Decoded, (u16, String)> {
    let win = web_sys::window().ok_or((0, "no window".to_string()))?;
    let res = JsFuture::from(win.fetch_with_str(url))
        .await
        .map_err(|e| (0, js_err(e)))?
        .dyn_into::<web_sys::Response>()
        .map_err(|_| (0, "no response".to_string()))?;
    if !res.ok() {
        return Err((res.status(), format!("HTTP {}", res.status())));
    }
    let blob = JsFuture::from(res.blob().map_err(|e| (0, js_err(e)))?)
        .await
        .map_err(|e| (0, js_err(e)))?
        .dyn_into::<web_sys::Blob>()
        .map_err(|_| (0, "no body".to_string()))?;
    decode_blob(&blob).await.map_err(|e| (0, e))
}
