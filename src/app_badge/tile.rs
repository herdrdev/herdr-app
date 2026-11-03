//! The Dock count, drawn over the icon. macOS shows `NSDockTile.badgeLabel`
//! only while the app may post notifications, and how many agents are waiting
//! must not depend on that permission. A tile's content view is the app's own
//! drawing, which the Dock always shows.

use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained, runtime::Bool};
use objc2_app_kit::{
    NSApplication, NSBezierPath, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSImage, NSImageView, NSStringDrawing, NSView,
};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

/// Badge height as a share of the icon's side, about the system badge's size.
const HEIGHT: f64 = 0.36;
/// Digit size as a share of the badge height.
const DIGITS: f64 = 0.62;

/// Shows `count` over the icon, or the plain icon for zero.
pub(super) fn show(count: usize, main_thread: MainThreadMarker) {
    let app = NSApplication::sharedApplication(main_thread);
    let tile = app.dockTile();
    let view = (count > 0)
        .then(|| app.applicationIconImage())
        .flatten()
        .map(|icon| NSImageView::imageViewWithImage(&badged(icon, count), main_thread));
    tile.setContentView(view.as_deref().map(|view| -> &NSView { view }));
    tile.display();
}

/// The icon with a red pill in its top-right corner holding `count`. The
/// drawing runs at whatever resolution the Dock renders, so it stays sharp.
fn badged(icon: Retained<NSImage>, count: usize) -> Retained<NSImage> {
    let size = icon.size();
    let height = size.width.min(size.height) * HEIGHT;
    let label = NSString::from_str(&count.to_string());
    let font = NSFont::boldSystemFontOfSize(height * DIGITS);
    let white = NSColor::whiteColor();
    // SAFETY: AppKit's attribute names are immutable, process-lifetime statics.
    #[allow(unsafe_code)]
    let keys = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
    let attributes = NSDictionary::from_slices(&keys, &[font.as_ref(), white.as_ref()]);
    // SAFETY: the attributes map the font key to an NSFont and the color key
    // to an NSColor, the types string drawing reads them as.
    #[allow(unsafe_code)]
    let text = unsafe { label.sizeWithAttributes(Some(&attributes)) };
    let width = height.max(text.width + height / 2.);
    let pill = NSRect::new(
        NSPoint::new(size.width - width, size.height - height),
        NSSize::new(width, height),
    );
    // Digits are centred on their cap height, from the baseline the point sets.
    let origin = NSPoint::new(
        pill.origin.x + (width - text.width) / 2.,
        pill.origin.y + (height - font.capHeight()) / 2. + font.descender(),
    );
    let draw = RcBlock::new(move |_: NSRect| {
        icon.drawInRect(NSRect::new(NSPoint::ZERO, size));
        NSColor::systemRedColor().setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(pill, height / 2., height / 2.)
            .fill();
        // SAFETY: as for the measurement above.
        #[allow(unsafe_code)]
        unsafe {
            label.drawAtPoint_withAttributes(origin, Some(&attributes));
        }
        Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(size, false, &draw)
}
