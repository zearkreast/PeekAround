//! Media actions — ONE registry used by the selection bar, the viewer's top bar and the
//! right-click context menu (mirrors `features/gallery/actions.tsx`, same ids, groups, order,
//! surfaces and visibility rules).
//!
//! Other screens register their own actions (album page: remove from album / set cover, phones:
//! send to phone …) with [`register`]; built-ins can be replaced by registering the same id.
//!
//! ```ignore
//! pa_ui::actions::register(Action {
//!     id: "album-remove".into(),
//!     icon: "image-minus",              // a VIcons name (ui/viewer/icons.slint)
//!     label_key: "album.remove",
//!     group: Group::Organize,
//!     order: 6,
//!     surfaces: &[Surface::Selection, Surface::Menu, Surface::Viewer],
//!     visible: Rc::new(|items, ctx| ctx.album.is_some() && !items.is_empty()),
//!     run: Run::Custom(Rc::new(|items| { /* … */ })),
//!     ..Action::default()
//! });
//! ```

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use pa_core::{MediaItem, MediaKind, TrashMode};

use crate::i18n::{self, Lang};

/// Where an action is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    /// the floating selection bar
    Selection,
    /// the viewer's top bar / "more" menu
    Viewer,
    /// right-click menu
    Menu,
}

/// Action groups (ordering + separators).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Group {
    /// favourite / download / share / edit
    Primary,
    /// share targets
    Share,
    /// slideshow / collage
    Create,
    /// move / copy / rename / albums
    Organize,
    /// show in folder / file manager
    #[default]
    Open,
    /// delete
    Danger,
}

/// What the actions can know about the current page / server.
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    /// server allows writes (`!caps.readonly`)
    pub writable: bool,
    /// the server runs as the desktop app on this machine (`info.mode == desktop`)
    pub desktop: bool,
    /// zip download of several items
    pub zip_download: bool,
    /// trash behaviour
    pub trash: Option<TrashMode>,
    /// albums are available
    pub albums: bool,
    /// temp (zip peek) folders
    pub temp: HashSet<String>,
    /// folders on a linked phone (view + copy only)
    pub device: HashSet<String>,
    /// MEGA streaming folders (view + copy only)
    pub remote: HashSet<String>,
    /// the album page that is open (album actions)
    pub album: Option<String>,
    /// an in-app folder page can be opened (router supports it)
    pub folder_pages: bool,
    /// surface being built
    pub surface: Option<Surface>,
}

impl Ctx {
    /// some items live on a phone or a MEGA link (read + copy only)
    pub fn on_device(&self, items: &[MediaItem]) -> bool {
        items
            .iter()
            .any(|i| self.device.contains(&i.folder_id) || self.remote.contains(&i.folder_id))
    }
    /// some items are in a temporary session
    pub fn any_temp(&self, items: &[MediaItem]) -> bool {
        items.iter().any(|i| self.temp.contains(&i.folder_id))
    }
}

/// visibility predicate
pub type VisibleFn = Rc<dyn Fn(&[MediaItem], &Ctx) -> bool>;
/// dynamic label
pub type LabelFn = Rc<dyn Fn(&[MediaItem], &Ctx, Lang) -> String>;
/// "on" state (filled heart)
pub type ActiveFn = Rc<dyn Fn(&[MediaItem]) -> bool>;

/// What running an action does.
#[derive(Clone, Default)]
pub enum Run {
    /// built-in, handled by `actions_ui`
    #[default]
    Builtin,
    /// registered by another screen
    Custom(Rc<dyn Fn(Vec<MediaItem>)>),
}

/// One action.
#[derive(Clone)]
pub struct Action {
    /// stable id (same as the web UI)
    pub id: String,
    /// VIcons name
    pub icon: &'static str,
    /// alternative icon while `active` (heart-off)
    pub icon_active: Option<&'static str>,
    /// i18n key of the label
    pub label_key: &'static str,
    /// label override
    pub label: Option<LabelFn>,
    /// highlighted state
    pub active: Option<ActiveFn>,
    /// group
    pub group: Group,
    /// order inside the group
    pub order: i32,
    /// surfaces (empty = all)
    pub surfaces: &'static [Surface],
    /// red
    pub destructive: bool,
    /// clear the selection after running from the selection bar
    pub clears_selection: bool,
    /// visibility
    pub visible: VisibleFn,
    /// behaviour
    pub run: Run,
}

impl Default for Action {
    fn default() -> Action {
        Action {
            id: String::new(),
            icon: "ellipsis-vertical",
            icon_active: None,
            label_key: "",
            label: None,
            active: None,
            group: Group::Open,
            order: 50,
            surfaces: &[],
            destructive: false,
            clears_selection: false,
            visible: Rc::new(|_, _| true),
            run: Run::Builtin,
        }
    }
}

impl Action {
    /// label for `items`
    pub fn label_for(&self, items: &[MediaItem], ctx: &Ctx, lang: Lang) -> String {
        match &self.label {
            Some(f) => f(items, ctx, lang),
            None => i18n::t(lang, self.label_key),
        }
    }
    /// active state for `items`
    pub fn is_active(&self, items: &[MediaItem]) -> bool {
        self.active.as_ref().is_some_and(|f| f(items))
    }
    /// icon for `items`
    pub fn icon_for(&self, items: &[MediaItem]) -> &'static str {
        match self.icon_active {
            Some(i) if self.is_active(items) => i,
            _ => self.icon,
        }
    }
    fn on(&self, s: Surface) -> bool {
        self.surfaces.is_empty() || self.surfaces.contains(&s)
    }
}

thread_local! {
    static REGISTRY: RefCell<Vec<Rc<Action>>> = RefCell::new(builtins());
    static REV: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Register (or replace by id) an action.
pub fn register(a: Action) {
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        r.retain(|x| x.id != a.id);
        r.push(Rc::new(a));
    });
    REV.with(|v| v.set(v.get() + 1));
}

/// Remove an action by id.
pub fn unregister(id: &str) {
    REGISTRY.with(|r| r.borrow_mut().retain(|x| x.id != id));
    REV.with(|v| v.set(v.get() + 1));
}

/// Registry revision (bumps on every change).
pub fn revision() -> u64 {
    REV.with(|v| v.get())
}

/// Actions for `items` on `surface`, sorted by group then order.
pub fn for_items(items: &[MediaItem], surface: Surface, ctx: &Ctx) -> Vec<Rc<Action>> {
    let ctx = Ctx {
        surface: Some(surface),
        ..ctx.clone()
    };
    let mut v: Vec<Rc<Action>> = REGISTRY.with(|r| {
        r.borrow()
            .iter()
            .filter(|a| a.on(surface) && (a.visible)(items, &ctx))
            .cloned()
            .collect()
    });
    v.sort_by_key(|a| (a.group, a.order));
    v
}

/// Look an action up by id.
pub fn get(id: &str) -> Option<Rc<Action>> {
    REGISTRY.with(|r| r.borrow().iter().find(|a| a.id == id).cloned())
}

fn is_media(i: &MediaItem) -> bool {
    matches!(i.kind, MediaKind::Image | MediaKind::Video)
}

/// all favourites → the action unfavourites
pub fn all_favorite(items: &[MediaItem]) -> bool {
    !items.is_empty() && items.iter().all(|i| i.favorite)
}

/// "Explorer" / "Finder" / the generic label
pub fn reveal_label(lang: Lang) -> String {
    if cfg!(windows) {
        "Explorer".into()
    } else if cfg!(target_os = "macos") {
        "Finder".into()
    } else {
        i18n::t(lang, "mobile.action.reveal")
    }
}

const MENU_VIEWER: &[Surface] = &[Surface::Menu, Surface::Viewer];

fn builtins() -> Vec<Rc<Action>> {
    let v: Vec<Action> = vec![
        Action {
            id: "download".into(),
            icon: "download",
            label_key: "common.download",
            group: Group::Primary,
            order: 11,
            // several items need the zip route
            visible: Rc::new(|items, c| items.len() == 1 || (c.zip_download && !items.is_empty())),
            ..Default::default()
        },
        Action {
            id: "favorite".into(),
            icon: "heart",
            icon_active: Some("heart-off"),
            label_key: "sel.favorite",
            label: Some(Rc::new(|items, _, l| {
                i18n::t(
                    l,
                    if all_favorite(items) {
                        "sel.unfavorite"
                    } else {
                        "sel.favorite"
                    },
                )
            })),
            active: Some(Rc::new(all_favorite)),
            group: Group::Primary,
            order: 30,
            // favourites live in our database: fine for MEGA items, not for phone items
            visible: Rc::new(|items, c| {
                items.iter().any(is_media) && !items.iter().any(|i| c.device.contains(&i.folder_id))
            }),
            ..Default::default()
        },
        Action {
            id: "edit".into(),
            icon: "square-pen",
            label_key: "viewer.edit",
            group: Group::Primary,
            order: 20,
            visible: Rc::new(|items, c| c.writable && items.len() == 1 && is_media(&items[0]) && !c.on_device(items)),
            ..Default::default()
        },
        Action {
            id: "slideshow".into(),
            icon: "presentation",
            label_key: "sel.slideshow",
            group: Group::Create,
            order: 10,
            visible: Rc::new(|items, _| items.iter().any(is_media)),
            ..Default::default()
        },
        Action {
            id: "collage".into(),
            icon: "layout-grid",
            label_key: "sel.collage",
            group: Group::Create,
            order: 20,
            visible: Rc::new(|items, c| {
                c.writable && items.iter().any(|i| i.kind == MediaKind::Image) && !c.on_device(items)
            }),
            ..Default::default()
        },
        Action {
            id: "add-to-album".into(),
            icon: "book-image",
            label_key: "album.addTo",
            group: Group::Organize,
            order: 5,
            visible: Rc::new(|items, c| c.albums && items.iter().any(is_media)),
            ..Default::default()
        },
        Action {
            id: "move".into(),
            icon: "folder-input",
            label_key: "sel.move",
            group: Group::Organize,
            order: 10,
            clears_selection: true,
            visible: Rc::new(|items, c| c.writable && !items.is_empty() && !c.on_device(items)),
            ..Default::default()
        },
        Action {
            id: "copy".into(),
            icon: "copy",
            label_key: "sel.copy",
            group: Group::Organize,
            order: 20,
            clears_selection: true,
            visible: Rc::new(|items, c| c.writable && !items.is_empty() && !c.on_device(items)),
            ..Default::default()
        },
        Action {
            id: "rename".into(),
            icon: "pencil",
            label_key: "common.rename",
            group: Group::Organize,
            order: 30,
            surfaces: MENU_VIEWER,
            visible: Rc::new(|items, c| c.writable && items.len() == 1 && !c.on_device(items)),
            ..Default::default()
        },
        Action {
            id: "show-in-folder".into(),
            icon: "folder-open",
            label_key: "viewer.showInFolder",
            group: Group::Open,
            order: 10,
            surfaces: MENU_VIEWER,
            visible: Rc::new(|items, c| c.folder_pages && items.len() == 1),
            ..Default::default()
        },
        Action {
            id: "reveal".into(),
            icon: "external-link",
            label_key: "mobile.action.reveal",
            label: Some(Rc::new(|_, _, l| reveal_label(l))),
            group: Group::Open,
            order: 20,
            surfaces: MENU_VIEWER,
            visible: Rc::new(|items, c| items.len() == 1 && c.desktop && !c.on_device(items)),
            ..Default::default()
        },
        Action {
            id: "copy-path".into(),
            icon: "clipboard-copy",
            label_key: "native.copyPath",
            group: Group::Open,
            order: 30,
            surfaces: MENU_VIEWER,
            visible: Rc::new(|items, c| items.len() == 1 && c.desktop && !c.on_device(items)),
            ..Default::default()
        },
        Action {
            id: "delete".into(),
            icon: "trash-2",
            label_key: "common.delete",
            group: Group::Danger,
            order: 10,
            destructive: true,
            clears_selection: true,
            visible: Rc::new(|items, c| c.writable && !items.is_empty() && !c.on_device(items)),
            ..Default::default()
        },
    ];
    v.into_iter().map(Rc::new).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, folder: &str, kind: MediaKind, fav: bool) -> MediaItem {
        MediaItem {
            id: id.into(),
            folder_id: folder.into(),
            dir: String::new(),
            name: format!("{id}.jpg"),
            ext: "jpg".into(),
            kind,
            size: 1,
            mtime: 0,
            width: None,
            height: None,
            duration: None,
            taken_at: 0,
            local_at: 0,
            flags: 0,
            thumbhash: None,
            favorite: fav,
            playback: pa_core::Playback::Direct,
        }
    }

    fn ids(v: &[Rc<Action>]) -> Vec<&str> {
        v.iter().map(|a| a.id.as_str()).collect()
    }

    #[test]
    fn builtin_visibility_and_order() {
        let ctx = Ctx {
            writable: true,
            desktop: true,
            zip_download: true,
            albums: true,
            folder_pages: true,
            ..Default::default()
        };
        let one = [item("a", "f1", MediaKind::Image, false)];
        let menu = for_items(&one, Surface::Menu, &ctx);
        assert_eq!(
            ids(&menu),
            vec![
                "download",
                "edit",
                "favorite",
                "slideshow",
                "collage",
                "add-to-album",
                "move",
                "copy",
                "rename",
                "show-in-folder",
                "reveal",
                "copy-path",
                "delete"
            ]
        );
        // the selection bar has no rename / folder / reveal
        let sel = for_items(&one, Surface::Selection, &ctx);
        assert!(!ids(&sel).contains(&"rename") && !ids(&sel).contains(&"reveal"));
        // read-only server: no file operations
        let ro = Ctx {
            writable: false,
            ..ctx.clone()
        };
        let v = ids(&for_items(&one, Surface::Menu, &ro))
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        assert!(!v
            .iter()
            .any(|i| i == "delete" || i == "move" || i == "rename" || i == "edit"));
        assert!(v.iter().any(|i| i == "favorite"));
        // phone items: view + copy only, no favourites
        let phone = Ctx {
            device: ["f1".to_string()].into(),
            ..ctx.clone()
        };
        let v = for_items(&one, Surface::Menu, &phone);
        assert!(!ids(&v).contains(&"favorite") && !ids(&v).contains(&"delete"));
        assert!(!ids(&v).contains(&"edit") && ids(&v).contains(&"download"));
        // MEGA items keep favourites
        let mega = Ctx {
            remote: ["f1".to_string()].into(),
            ..ctx.clone()
        };
        let v = for_items(&one, Surface::Menu, &mega);
        assert!(ids(&v).contains(&"favorite") && !ids(&v).contains(&"move"));
        // several items without zip download: no download
        let two = [
            item("a", "f1", MediaKind::Image, true),
            item("b", "f1", MediaKind::Video, true),
        ];
        let nozip = Ctx {
            zip_download: false,
            ..ctx.clone()
        };
        assert!(!ids(&for_items(&two, Surface::Selection, &nozip)).contains(&"download"));
        // favourite label / icon flip when everything is a favourite
        let fav = get("favorite").unwrap();
        assert!(fav.is_active(&two));
        assert_eq!(fav.icon_for(&two), "heart-off");
        assert_eq!(fav.label_for(&two, &ctx, Lang::En), "Remove from favorites");
        assert_eq!(fav.label_for(&one, &ctx, Lang::En), "Add to favorites");
    }

    #[test]
    fn register_replaces_by_id() {
        let r0 = revision();
        register(Action {
            id: "test-x".into(),
            label_key: "common.open",
            group: Group::Share,
            visible: Rc::new(|items, _| items.len() == 1),
            run: Run::Custom(Rc::new(|_| {})),
            ..Default::default()
        });
        assert!(revision() > r0);
        let one = [item("a", "f1", MediaKind::Image, false)];
        let v = for_items(&one, Surface::Menu, &Ctx::default());
        assert!(ids(&v).contains(&"test-x"));
        unregister("test-x");
        assert!(!ids(&for_items(&one, Surface::Menu, &Ctx::default())).contains(&"test-x"));
    }
}
