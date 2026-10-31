//! HERDR: share images within a window and retire unused sources after a short grace.

use std::{collections::HashMap, rc::Rc, sync::Arc, time::Duration};

use gpui::{
    App, AppContext as _, BorrowAppContext as _, Entity, Global, ImageCacheItem, ImageSource,
    Resource, Task, WindowId,
};

const GRACE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Caches(HashMap<WindowId, Entity<Cache>>);
impl Global for Caches {}

struct Cache {
    images: HashMap<Resource, (Rc<ImageCacheItem>, bool)>,
    _clock: Task<()>,
}

impl Cache {
    fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            cx.on_release(|this: &mut Self, cx| {
                for (_, (image, _)) in this.images.drain() {
                    if let Some(Ok(image)) = image.get() {
                        cx.drop_image(image, None);
                    }
                }
            })
            .detach();
            let clock = cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(GRACE).await;
                    if this
                        .update(cx, |this, cx| {
                            this.images.retain(|_, (image, unused)| {
                                if Rc::strong_count(image) > 1 {
                                    *unused = false;
                                    return true;
                                }
                                if !*unused {
                                    *unused = true;
                                    return true;
                                }
                                if let Some(Ok(image)) = image.get() {
                                    cx.drop_image(image, None);
                                }
                                false
                            });
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
            Self {
                images: HashMap::new(),
                _clock: clock,
            }
        })
    }
}

pub(super) fn source(path: &str) -> ImageSource {
    let ImageSource::Resource(resource) = ImageSource::from(path) else {
        unreachable!("a path is an image resource")
    };
    ImageSource::Custom(Arc::new(move |window, cx| {
        if !cx.has_global::<Caches>() {
            cx.set_global(Caches::default());
            cx.on_window_closed(|cx, id| {
                cx.global_mut::<Caches>().0.remove(&id);
            })
            .detach();
        }
        let cache = cx.update_global::<Caches, _>(|caches, cx| {
            caches
                .0
                .entry(window.window_handle().window_id())
                .or_insert_with(|| Cache::new(cx))
                .clone()
        });
        cache.update(cx, |cache, cx| {
            let (image, unused) = cache
                .images
                .entry(resource.clone())
                .or_insert_with(|| (Rc::new(ImageCacheItem::new(&resource, cx)), false));
            *unused = false;
            // Cached GPUI subtrees keep these leases even when they skip layout and paint.
            window.use_keyed_state(("shell-image", gpui::hash(&resource)), cx, |_, _| {
                image.clone()
            });
            image.use_image(window)
        })
    }))
}

#[cfg(feature = "render-test-support")]
pub(crate) fn loaded(window: &gpui::Window, cx: &App) -> Vec<Arc<gpui::RenderImage>> {
    cx.try_global::<Caches>()
        .and_then(|caches| caches.0.get(&window.window_handle().window_id()))
        .map(|cache| {
            cache
                .read(cx)
                .images
                .values()
                .filter_map(|(image, _)| image.get()?.ok())
                .collect()
        })
        .unwrap_or_default()
}
