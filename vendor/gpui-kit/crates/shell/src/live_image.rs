//! HERDR: pictures a host hands over from memory, drawn wherever
//! `image(path)` names their path instead of the file there.
//!
//! A path's newest picture is used by the next frame; the one it replaced is
//! freed by the window once no frame draws it (the frame-end tile retirement in
//! GPUI's `Window::draw`), so replacing a picture every frame doesn't grow the
//! sprite atlas. Keys are spelled as [`crate::AppAssets::cache_key`] spells
//! them, so `mods/a/x` and `./mods/a/x` are the same picture, as they are the
//! same file.
//!
//! Each picture belongs to the evaluated application that set it. Once that
//! application is retired (its view closed, reloaded or restarted) the picture
//! stops showing, without the host having to notice every way that happens.

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use gpui::{RenderImage, SharedString};

use crate::runtime::ApplicationGeneration;

thread_local! {
    static IMAGES: RefCell<HashMap<SharedString, (Arc<RenderImage>, Owner)>> = RefCell::new(HashMap::new());
}

/// The evaluated application a picture belongs to.
#[derive(Clone)]
pub struct Owner(Rc<ApplicationGeneration>);

impl Owner {
    /// The application of the script call in progress, if it has one.
    pub fn current() -> Option<Self> {
        crate::scope::current_application_generation().map(Self)
    }

    /// False once the application is retired: closed, reloaded or restarted.
    pub fn is_live(&self) -> bool {
        self.0.is_active()
    }

    pub fn same(&self, other: &Owner) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// The key `path` is stored and looked up under, or `None` for a path
/// `image(path)` doesn't load from the application directory: a URL, or a
/// path that is empty, only `.` or `/`, or has `..`.
pub fn key(path: &str) -> Option<SharedString> {
    if path.split(['/', '\\']).any(|part| part == "..") {
        return None;
    }
    match gpui::ImageSource::from(path) {
        gpui::ImageSource::Resource(gpui::Resource::Embedded(_)) => crate::AppAssets::relative_key(path),
        _ => None,
    }
}

/// Shows `image` for an already-keyed path from the next frame on, for as long
/// as `owner` is live. Does nothing once the thread is ending.
pub fn set(key: SharedString, image: Arc<RenderImage>, owner: Owner) {
    let _ = IMAGES.try_with(|images| images.borrow_mut().insert(key, (image, owner)));
}

/// Shows the path's file again. Does nothing once the thread is ending.
pub fn clear(key: &str) {
    let _ = IMAGES.try_with(|images| images.borrow_mut().remove(key));
}

/// The picture shown for an already-keyed path, if one was set by an
/// application that is still live.
pub fn get(key: &str) -> Option<Arc<RenderImage>> {
    IMAGES.with_borrow_mut(|images| {
        let (image, owner) = images.get(key)?;
        if owner.is_live() {
            return Some(image.clone());
        }
        images.remove(key);
        None
    })
}

/// How many paths currently show a picture from memory.
pub fn len() -> usize {
    IMAGES.with_borrow_mut(|images| {
        images.retain(|_, (_, owner)| owner.is_live());
        images.len()
    })
}
