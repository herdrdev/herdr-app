use crate::{DevicePixels, Pixels, Result, SharedString, Size, size};
use smallvec::SmallVec;

use image::{Delay, Frame};
use std::{
    borrow::Cow,
    fmt,
    hash::Hash,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
};

/// A source of assets for this app to use.
pub trait AssetSource: 'static + Send + Sync {
    /// Load the given asset from the source path.
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>>;

    /// Normalize a resource's identity without changing how it is read.
    fn image_resource(&self, resource: &crate::Resource) -> crate::Resource {
        resource.clone()
    }

    /// List the assets at the given path.
    fn list(&self, path: &str) -> Result<Vec<SharedString>>;
}

impl AssetSource for () {
    fn load(&self, _path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(None)
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

/// A unique identifier for the image cache
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ImageId(pub usize);

#[derive(PartialEq, Eq, Hash, Clone)]
#[expect(missing_docs)]
pub struct RenderImageParams {
    pub image_id: ImageId,
    pub frame_index: usize,
}

/// A cached and processed image, in BGRA format
pub struct RenderImage {
    /// The ID associated with this image
    pub id: ImageId,
    /// The scale factor of this image on render.
    pub(crate) scale_factor: f32,
    data: SmallVec<[Arc<Frame>; 1]>,
    animation: Option<Animation>,
    _animation_source: Option<Arc<RenderImage>>,
}

struct Animation {
    bytes: Vec<u8>,
    format: image::ImageFormat,
}

impl PartialEq for RenderImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for RenderImage {}

impl RenderImage {
    /// Create a new image from the given data.
    pub fn new(data: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        Self {
            id: ImageId(NEXT_ID.fetch_add(1, SeqCst)),
            scale_factor: 1.0,
            data: data.into().into_iter().map(Arc::new).collect(),
            animation: None,
            _animation_source: None,
        }
    }

    pub(crate) fn with_animation(mut self, bytes: Vec<u8>, format: image::ImageFormat) -> Self {
        self.animation = Some(Animation { bytes, format });
        self
    }

    pub(crate) fn cache_bytes(&self) -> usize {
        self.data.iter().map(|frame| frame.buffer().as_raw().len()).sum::<usize>()
            + self.animation.as_ref().map_or(0, |animation| animation.bytes.len())
    }

    pub(crate) fn has_animation(&self) -> bool {
        self.animation.is_some()
    }

    pub(crate) fn prepare_animation(self: &Arc<Self>) -> Self {
        use image::AnimationDecoder;
        use std::io::Cursor;
        let mut image = Self::new(SmallVec::new());
        image.data.extend(self.data.first().cloned());
        // A cached animated scene must keep its encoded source alive too.
        image._animation_source = Some(self.clone());
        let Some(animation) = &self.animation else {
            return image;
        };
        let frames = match animation.format {
            image::ImageFormat::Gif => {
                image::codecs::gif::GifDecoder::new(Cursor::new(&animation.bytes))
                    .map(|decoder| decoder.into_frames())
            }
            image::ImageFormat::WebP => image::codecs::webp::WebPDecoder::new(Cursor::new(
                &animation.bytes,
            ))
            .map(|mut decoder| {
                let _ = decoder.set_background_color(image::Rgba([0, 0, 0, 0]));
                decoder.into_frames()
            }),
            _ => unreachable!(),
        };
        if let Ok(frames) = frames {
            image.data.extend(
                frames
                    .filter_map(|frame| match frame {
                        Ok(frame) => Some(frame),
                        Err(error) => {
                            log::debug!("Skipping animation frame: {error}");
                            None
                        }
                    })
                    .skip(1)
                    .map(|mut frame| {
                        for pixel in frame.buffer_mut().chunks_exact_mut(4) {
                            pixel.swap(0, 2);
                        }
                        Arc::new(frame)
                    }),
            );
        }
        image
    }

    fn frame(&self, index: usize) -> Option<&Frame> {
        self.data.get(index).map(|frame| frame.as_ref())
    }

    /// Convert this image into a byte slice.
    pub fn as_bytes(&self, frame_index: usize) -> Option<&[u8]> {
        self.frame(frame_index)
            .map(|frame| frame.buffer().as_raw().as_slice())
    }

    /// Get the size of this image, in pixels.
    pub fn size(&self, frame_index: usize) -> Size<DevicePixels> {
        self.frame(frame_index)
            .map(|frame| {
                let (width, height) = frame.buffer().dimensions();
                size(width.into(), height.into())
            })
            .unwrap_or_default()
    }

    /// Get the size of this image, in pixels for display, adjusted for the scale factor.
    pub(crate) fn render_size(&self, frame_index: usize) -> Size<Pixels> {
        self.size(frame_index)
            .map(|v| (v.0 as f32 / self.scale_factor).into())
    }

    /// Get the delay of this frame from the previous
    pub fn delay(&self, frame_index: usize) -> Delay {
        self.frame(frame_index)
            .map(|frame| frame.delay())
            .unwrap_or(Delay::from_numer_denom_ms(100, 1))
    }

    /// Get the number of frames for this image.
    pub fn frame_count(&self) -> usize {
        self.data.len()
    }
}

impl fmt::Debug for RenderImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageData")
            .field("id", &self.id)
            .field("size", &self.data.first().map(|f| f.buffer().dimensions()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::SmallVec;

    #[test]
    fn empty_render_image_does_not_panic() {
        let image = RenderImage::new(SmallVec::new());
        assert_eq!(image.frame_count(), 0);
        assert_eq!(image.size(0), Size::default());
        assert_eq!(image.as_bytes(0), None);
        assert_eq!(image.render_size(0), Size::default());
        assert_eq!(image.delay(0), Delay::from_numer_denom_ms(100, 1));
        let _ = format!("{image:?}");
    }
}
