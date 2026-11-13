//! Fonts page (features/fonts/index.tsx): upload TTF / OTF / WOFF / WOFF2 files, preview every
//! uploaded face in itself, delete, and the bundled families. Uploaded fonts are registered with
//! Slint's shared font collection so any `Text` (and later the editors' text layers) can use them
//! by family name.

use std::cell::RefCell;
use std::collections::HashSet;
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;

use pa_core::{FontFormat, FontInfo, FontStyle, FontsResponse};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::api::enc;
use crate::app::AppInner;
use crate::router::Route;
use crate::{BundledFont, FontFaceRow, FontFamilyCard, FontsState, ToastKind};

const FONT_EXT: [&str; 4] = ["ttf", "otf", "woff", "woff2"];

/// family, label, category key, weights, italic — the 1.x set (lib/fonts.ts BUNDLED_FONTS),
/// compiled into the UI from crates/pa-ui/fonts
const BUNDLED: [(&str, &str, &str, &str, bool); 12] = [
    ("Inter", "Inter", "sans", "300 400 500 600 700 800 900", false),
    ("Bricolage Grotesque", "Bricolage Grotesque", "sans", "600 700", false),
    ("Montserrat", "Montserrat", "sans", "400 700 900", true),
    ("Playfair Display", "Playfair Display", "serif", "400 700", true),
    ("Bebas Neue", "Bebas Neue", "display", "400", false),
    ("Oswald", "Oswald", "display", "400 700", false),
    ("Anton", "Anton", "display", "400", false),
    ("Lobster", "Lobster", "script", "400", false),
    ("Pacifico", "Pacifico", "script", "400", false),
    ("Caveat", "Caveat", "script", "400 700", false),
    ("Dancing Script", "Dancing Script", "script", "400 700", false),
    ("Space Mono", "Space Mono", "mono", "400 700", true),
];

thread_local! {
    /// font ids already handed to the shared collection
    static REGISTERED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// the last list from the server
    static FONTS: RefCell<Vec<FontInfo>> = const { RefCell::new(Vec::new()) };
}

/// The uploaded fonts of the last list.
pub(crate) fn uploaded() -> Vec<FontInfo> {
    FONTS.with(|f| f.borrow().clone())
}

/// Families a text layer can use: (family, weights, has italic), bundled first, then the
/// uploaded faces the renderer has.
pub(crate) fn picker() -> Vec<(String, Vec<u16>, bool)> {
    let mut out: Vec<(String, Vec<u16>, bool)> = BUNDLED
        .iter()
        .map(|(family, _, _, weights, italic)| {
            ((*family).to_string(), weights.split(' ').filter_map(|w| w.parse().ok()).collect(), *italic)
        })
        .collect();
    let registered = REGISTERED.with(|r| r.borrow().clone());
    let mut custom: Vec<(String, Vec<u16>, bool)> = Vec::new();
    for f in uploaded().iter().filter(|f| registered.contains(&f.id)) {
        let i = match custom.iter().position(|c| c.0 == f.family) {
            Some(i) => i,
            None => {
                custom.push((f.family.clone(), Vec::new(), false));
                custom.len() - 1
            }
        };
        let c = &mut custom[i];
        let w = f.weight.clamp(1, 1000) as u16;
        if !c.1.contains(&w) {
            c.1.push(w);
        }
        c.2 |= f.style == FontStyle::Italic;
    }
    for c in custom.iter_mut() {
        c.1.sort_unstable();
    }
    custom.sort_by_key(|c| c.0.to_lowercase());
    custom.retain(|c| !BUNDLED.iter().any(|b| b.0 == c.0));
    out.extend(custom);
    out
}

/// Bumps whenever a face is registered, so text drawn with a fallback is drawn again.
pub(crate) fn epoch() -> usize {
    REGISTERED.with(|r| r.borrow().len())
}

pub(crate) fn enter(app: &Rc<AppInner>) {
    publish_bundled(app);
    load(app);
}

/// Load the list (also refreshes the registered set). Safe on any page.
pub(crate) fn load(app: &Rc<AppInner>) {
    crate::api::call::<FontsResponse>(app, "GET", "/api/fonts", None, |a, r| {
        let st = a.window.global::<FontsState>();
        st.set_loading(false);
        match r {
            Ok(r) => {
                let list: Vec<FontInfo> = r.fonts.into_iter().filter(|f| !f.bundled).collect();
                FONTS.with(|f| *f.borrow_mut() = list.clone());
                register_all(a, &list);
                publish(a);
            }
            Err(e) => a.error_toast(&e.message),
        }
    });
}

pub(crate) fn on_fonts_changed(app: &Rc<AppInner>) {
    load(app);
}

/// OS file drops go here while the Fonts page is open
pub(crate) fn wants_drop(app: &AppInner) -> bool {
    matches!(*app.route.borrow(), Route::Fonts)
}

fn publish_bundled(app: &Rc<AppInner>) {
    let rows: Vec<BundledFont> = BUNDLED
        .iter()
        .map(|(family, label, cat, weights, italic)| BundledFont {
            family: (*family).into(),
            label: (*label).into(),
            category: app.t(&format!("fonts.cat.{cat}")).into(),
            weights: ModelRc::new(VecModel::from(
                weights
                    .split(' ')
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            )),
            italic: *italic,
        })
        .collect();
    app.window
        .global::<FontsState>()
        .set_bundled(ModelRc::new(VecModel::from(rows)));
}

fn publish(app: &Rc<AppInner>) {
    let list = uploaded();
    let mut fams: Vec<(String, Vec<FontInfo>)> = vec![];
    for f in &list {
        match fams.iter_mut().find(|(n, _)| *n == f.family) {
            Some((_, v)) => v.push(f.clone()),
            None => fams.push((f.family.clone(), vec![f.clone()])),
        }
    }
    for (_, v) in fams.iter_mut() {
        v.sort_by(|a, b| {
            a.weight
                .cmp(&b.weight)
                .then((a.style == FontStyle::Italic).cmp(&(b.style == FontStyle::Italic)))
        });
    }
    fams.sort_by_key(|(n, _)| n.to_lowercase());
    let registered = REGISTERED.with(|r| r.borrow().clone());
    let cards: Vec<FontFamilyCard> = fams
        .into_iter()
        .map(|(family, faces)| {
            let count = if faces.len() == 1 {
                app.t("fonts.style1")
            } else {
                crate::i18n::tv(app.lang(), "fonts.styles", &[("n", &faces.len().to_string())])
            };
            let rows: Vec<FontFaceRow> = faces
                .iter()
                .map(|f| FontFaceRow {
                    id: f.id.clone().into(),
                    family: f.family.clone().into(),
                    sub: f.subfamily.clone().into(),
                    weight: f.weight as i32,
                    italic: f.style == FontStyle::Italic,
                    format: format_label(f.format).into(),
                    // WOFF2 can't be handed to the renderer: it previews in the UI font
                    previewable: registered.contains(&f.id),
                })
                .collect();
            FontFamilyCard {
                family: family.into(),
                count: count.into(),
                faces: ModelRc::new(VecModel::from(rows)),
            }
        })
        .collect();
    let st = app.window.global::<FontsState>();
    st.set_custom_count(list.len() as i32);
    st.set_families(ModelRc::new(VecModel::from(cards)));
}

fn format_label(f: FontFormat) -> &'static str {
    match f {
        FontFormat::Truetype => "ttf",
        FontFormat::Opentype => "otf",
        FontFormat::Woff => "woff",
        FontFormat::Woff2 => "woff2",
    }
}

/// Fetch and register every uploaded face not registered yet (TTF / OTF / WOFF).
fn register_all(app: &Rc<AppInner>, list: &[FontInfo]) {
    for f in list {
        if f.format == FontFormat::Woff2 || REGISTERED.with(|r| r.borrow().contains(&f.id)) {
            continue;
        }
        let fut = app.backend.get_bytes(&f.url);
        let id = f.id.clone();
        let w = Rc::downgrade(app);
        app.rt.spawn_then(
            async move {
                let b = fut.await.ok()?;
                sfnt_of(&b)
            },
            move |data| {
                let (Some(a), Some(data)) = (w.upgrade(), data) else {
                    return;
                };
                let blob = slint::fontique_011::fontique::Blob::new(std::sync::Arc::new(data));
                let n = slint::fontique_011::shared_collection()
                    .register_fonts(blob, None)
                    .len();
                if n > 0 {
                    REGISTERED.with(|r| r.borrow_mut().insert(id));
                    publish(&a);
                    crate::editor::fonts_changed(&a);
                    crate::video_editor::fonts_changed();
                }
            },
        );
    }
}

/// TTF / OTF / TTC as they are; WOFF 1.0 rebuilt into an sfnt; None for anything else.
pub(crate) fn sfnt_of(b: &[u8]) -> Option<Vec<u8>> {
    match b.get(0..4)? {
        [0, 1, 0, 0] | b"OTTO" | b"true" | b"ttcf" => Some(b.to_vec()),
        b"wOFF" => woff_to_sfnt(b),
        _ => None,
    }
}

fn be16(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]))
}
fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// WOFF 1.0 → sfnt: same table directory, each table inflated (zlib) when compressed, 4-byte
/// aligned. Checksums are copied from the WOFF directory (they are the sfnt checksums).
fn woff_to_sfnt(b: &[u8]) -> Option<Vec<u8>> {
    let flavor = be32(b, 4)?;
    let n = be16(b, 12)? as usize;
    let mut entry_sel = 0u16;
    while (1usize << (entry_sel + 1)) <= n {
        entry_sel += 1;
    }
    let search = (1u16 << entry_sel) * 16;
    let mut out = Vec::new();
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&search.to_be_bytes());
    out.extend_from_slice(&entry_sel.to_be_bytes());
    out.extend_from_slice(&((n as u16) * 16 - search).to_be_bytes());
    let dir_at = out.len();
    out.resize(dir_at + n * 16, 0);
    for i in 0..n {
        let e = 44 + i * 20;
        let tag = b.get(e..e + 4)?;
        let off = be32(b, e + 4)? as usize;
        let comp = be32(b, e + 8)? as usize;
        let orig = be32(b, e + 12)? as usize;
        let sum = be32(b, e + 16)?;
        let src = b.get(off..off.checked_add(comp)?)?;
        let data = if comp < orig {
            let mut v = Vec::with_capacity(orig);
            flate2::read::ZlibDecoder::new(src).read_to_end(&mut v).ok()?;
            if v.len() != orig {
                return None;
            }
            v
        } else {
            src.to_vec()
        };
        let at = out.len();
        out.extend_from_slice(&data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
        let d = dir_at + i * 16;
        out[d..d + 4].copy_from_slice(tag);
        out[d + 4..d + 8].copy_from_slice(&sum.to_be_bytes());
        out[d + 8..d + 12].copy_from_slice(&(at as u32).to_be_bytes());
        out[d + 12..d + 16].copy_from_slice(&(orig as u32).to_be_bytes());
    }
    Some(out)
}

/// "Choose files": the native picker, then upload.
pub(crate) fn browse(app: &Rc<AppInner>) {
    let fut = app.host.pick_files(
        &app.t("fonts.upload"),
        vec![crate::host::FileFilter {
            name: "Fonts".into(),
            extensions: FONT_EXT.iter().map(|e| e.to_string()).collect(),
        }],
    );
    let w = Rc::downgrade(app);
    app.rt.spawn_then(fut, move |files| {
        if let Some(a) = w.upgrade() {
            upload(&a, files);
        }
    });
}

/// Upload font files one after the other (PUT /api/fonts?name=…), then reload the list.
pub(crate) fn upload(app: &Rc<AppInner>, files: Vec<PathBuf>) {
    if files.is_empty() {
        return;
    }
    let (ok, bad): (Vec<PathBuf>, Vec<PathBuf>) = files.into_iter().partition(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| FONT_EXT.contains(&e.to_lowercase().as_str()))
    });
    for p in &bad {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        app.toast(ToastKind::Error, &crate::i18n::tv(app.lang(), "fonts.badFile", &[("name", &name)]), "");
    }
    if ok.is_empty() {
        return;
    }
    let names: Vec<String> = ok
        .iter()
        .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        .collect();
    set_uploading(app, &names);
    upload_next(app, ok, 0);
}

fn set_uploading(app: &Rc<AppInner>, names: &[String]) {
    let v: Vec<SharedString> = names.iter().map(SharedString::from).collect();
    app.window
        .global::<FontsState>()
        .set_uploading(ModelRc::new(VecModel::from(v)));
}

fn upload_next(app: &Rc<AppInner>, mut left: Vec<PathBuf>, added: usize) {
    if left.is_empty() {
        set_uploading(app, &[]);
        load(app);
        if added == 1 {
            app.toast(ToastKind::Success, &app.t("fonts.added"), "");
        } else if added > 1 {
            app.toast(
                ToastKind::Success,
                &crate::i18n::tv(app.lang(), "fonts.addedMany", &[("n", &added.to_string())]),
                "",
            );
        }
        return;
    }
    let p = left.remove(0);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let names: Vec<String> = left
        .iter()
        .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        .collect();
    let backend = app.backend.clone();
    let path = format!("/api/fonts?name={}", enc(&name));
    let w = Rc::downgrade(app);
    app.rt.spawn_then(
        async move {
            let body = crate::rt::read_file(p).await?;
            backend.put_bytes(&path, body).await.map_err(|e| e.message)
        },
        move |r| {
            let Some(a) = w.upgrade() else { return };
            let ok = match r {
                Ok(_) => true,
                Err(e) => {
                    a.toast(ToastKind::Error, &a.t("common.error"), &format!("{name}: {e}"));
                    false
                }
            };
            set_uploading(&a, &names);
            upload_next(&a, left, added + ok as usize);
        },
    );
}

/// Delete one face (after a confirmation).
pub(crate) fn delete(app: &Rc<AppInner>, id: String, label: String) {
    let title = crate::i18n::tv(app.lang(), "fonts.deleteConfirm", &[("name", &label)]);
    let del = app.t("common.delete");
    super::dialogs::confirm(app, &title, "", &del, true, move |a| {
        crate::api::call::<serde_json::Value>(
            a,
            "DELETE",
            &format!("/api/fonts/{}", enc(&id)),
            None,
            |a, r| match r {
                Ok(_) => {
                    a.toast(ToastKind::Success, &a.t("fonts.deleted"), "");
                    load(a);
                }
                Err(e) => a.error_toast(&e.message),
            },
        );
    });
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let st = app.window.global::<FontsState>();
    st.set_sample(SharedString::new());
    let w = Rc::downgrade(app);
    st.on_browse(move || {
        if let Some(a) = w.upgrade() {
            browse(&a);
        }
    });
    let w = Rc::downgrade(app);
    st.on_delete(move |id, label| {
        if let Some(a) = w.upgrade() {
            delete(&a, id.into(), label.into());
        }
    });
    // uploaded fonts are usable in every text field from the start, not only after a visit here
    load(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sfnt_passes_through() {
        let ttf = [0u8, 1, 0, 0, 0, 0, 0, 0];
        assert_eq!(sfnt_of(&ttf).as_deref(), Some(&ttf[..]));
        assert_eq!(sfnt_of(b"OTTOxxxx").unwrap(), b"OTTOxxxx".to_vec());
        assert!(sfnt_of(b"wOF2xxxx").is_none());
        assert!(sfnt_of(b"PK\x03\x04").is_none());
    }

    /// a WOFF built from a tiny two-table sfnt (one zlib-compressed table, one stored) comes back
    /// with the same tables
    #[test]
    fn woff_roundtrip() {
        use std::io::Write;
        let t1: Vec<u8> = (0..200u32).map(|i| (i % 7) as u8).collect();
        let t2: Vec<u8> = vec![1, 2, 3];
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&t1).unwrap();
        let c1 = z.finish().unwrap();
        let mut w = Vec::new();
        w.extend_from_slice(b"wOFF");
        w.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        w.extend_from_slice(&0u32.to_be_bytes()); // length (unused here)
        w.extend_from_slice(&2u16.to_be_bytes());
        w.resize(44, 0);
        let off1 = 44 + 2 * 20;
        let off2 = off1 + c1.len();
        for (tag, off, comp, orig) in [
            (b"aaaa", off1, c1.len(), t1.len()),
            (b"bbbb", off2, t2.len(), t2.len()),
        ] {
            w.extend_from_slice(tag);
            w.extend_from_slice(&(off as u32).to_be_bytes());
            w.extend_from_slice(&(comp as u32).to_be_bytes());
            w.extend_from_slice(&(orig as u32).to_be_bytes());
            w.extend_from_slice(&7u32.to_be_bytes());
        }
        w.extend_from_slice(&c1);
        w.extend_from_slice(&t2);
        let s = sfnt_of(&w).unwrap();
        assert_eq!(&s[0..4], &[0, 1, 0, 0]);
        assert_eq!(be16(&s, 4), Some(2));
        let table = |i: usize| {
            let d = 12 + i * 16;
            let off = be32(&s, d + 8).unwrap() as usize;
            let len = be32(&s, d + 12).unwrap() as usize;
            (s[d..d + 4].to_vec(), s[off..off + len].to_vec())
        };
        assert_eq!(table(0), (b"aaaa".to_vec(), t1));
        assert_eq!(table(1), (b"bbbb".to_vec(), t2));
    }
}
