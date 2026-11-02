//! COLRv1 paint traversal, with tiny-skia handling coverage and compositing.
use skrifa::{
    FontRef, GlyphId, MetadataProvider,
    color::{Brush, ColorGlyphFormat, ColorPainter, ColorStop, CompositeMode, Extend},
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
    raw::{TableProvider, types::BoundingBox},
};
use swash::{
    scale::{
        Source,
        image::{Content, Image},
    },
    zeno::Placement,
};
use tiny_skia::{
    BlendMode, FillRule, Mask, Path, PathBuilder, Pixmap, PixmapPaint, Point, Rect, Transform,
};

// Limits also apply to malformed fonts, before any per-layer allocations.
const MAX_SIDE: u32 = 2048;
const MAX_PIXELS: u32 = 1_048_576;
const MAX_NODES: usize = 4096;
const MAX_WORK: usize = 64 * 1_048_576;

// Validate expanded work before asking skrifa to traverse: callbacks cannot
// cancel its traversal, and a small acyclic DAG can expand exponentially.
fn validate_graph(data: &[u8], glyph: u32) -> Option<(usize, bool)> {
    fn uint(data: &[u8], at: usize, len: usize) -> Option<usize> {
        Some(
            data.get(at..at.checked_add(len)?)?
                .iter()
                .fold(0, |n, b| (n << 8) | usize::from(*b)),
        )
    }
    fn root(data: &[u8], glyph: usize) -> Option<usize> {
        let list = uint(data, 14, 4)?;
        let count = uint(data, list, 4)?;
        if count > 65536 {
            return None;
        }
        let records = data.get(list + 4..list + 4 + count * 6)?.as_chunks::<6>().0;
        // Use the same slice search as skrifa, including its choice among
        // malformed duplicate records, so validation walks the actual root.
        let ix = records
            .binary_search_by_key(&glyph, |r| usize::from(u16::from_be_bytes([r[0], r[1]])))
            .ok()?;
        list.checked_add(uint(data, list + 4 + ix * 6 + 2, 4)?)
    }
    fn visit(
        data: &[u8],
        at: usize,
        depth: usize,
        nodes: &mut usize,
        foreground: &mut bool,
    ) -> Option<()> {
        if depth >= 32 || *nodes >= MAX_NODES {
            return None;
        }
        *nodes += 1;
        match *data.get(at)? {
            1 => {
                let count = uint(data, at + 1, 1)?;
                let first = uint(data, at + 2, 4)?;
                let list = uint(data, 18, 4)?;
                for i in first..first.checked_add(count)? {
                    let child = list.checked_add(uint(data, list + 4 + i * 4, 4)?)?;
                    visit(data, child, depth + 1, nodes, foreground)?;
                }
            }
            2 | 3 => *foreground |= uint(data, at + 1, 2)? == 0xffff,
            4..=9 => {
                let line = at.checked_add(uint(data, at + 1, 3)?)?;
                let stops = uint(data, line + 1, 2)?;
                if stops > 64 || *nodes + stops > MAX_NODES {
                    return None;
                }
                *nodes += stops;
                for i in 0..stops {
                    let stride = if data[at] % 2 == 0 { 6 } else { 10 };
                    *foreground |= uint(data, line + 3 + i * stride + 2, 2)? == 0xffff;
                }
            }
            11 => visit(
                data,
                root(data, uint(data, at + 1, 2)?)?,
                depth + 1,
                nodes,
                foreground,
            )?,
            10 => {
                let child = at.checked_add(uint(data, at + 1, 3)?)?;
                // Skrifa tries fill_glyph optimization, then traverses again
                // when it fails. Nested PaintGlyphs can double work each time.
                visit(data, child, depth + 1, nodes, foreground)?;
                visit(data, child, depth + 1, nodes, foreground)?;
            }
            12..=31 => visit(
                data,
                at.checked_add(uint(data, at + 1, 3)?)?,
                depth + 1,
                nodes,
                foreground,
            )?,
            32 => {
                visit(
                    data,
                    at.checked_add(uint(data, at + 1, 3)?)?,
                    depth + 1,
                    nodes,
                    foreground,
                )?;
                visit(
                    data,
                    at.checked_add(uint(data, at + 5, 3)?)?,
                    depth + 1,
                    nodes,
                    foreground,
                )?;
            }
            _ => return None,
        }
        Some(())
    }
    let mut nodes = 0;
    let mut foreground = false;
    visit(
        data,
        root(data, glyph as usize)?,
        0,
        &mut nodes,
        &mut foreground,
    )?;
    Some((nodes, foreground))
}

fn empty_image() -> Image {
    Image {
        source: Source::ColorOutline(0),
        content: Content::Color,
        placement: Placement::default(),
        data: Vec::new(),
    }
}

pub(crate) fn has_glyph(data: &[u8], index: u32, glyph: u32) -> bool {
    FontRef::from_index(data, index).ok().is_some_and(|font| {
        font.color_glyphs()
            .get_with_format(GlyphId::new(glyph), ColorGlyphFormat::ColrV1)
            .is_some()
    })
}

pub(crate) fn uses_foreground(data: &[u8], index: u32, glyph: u32) -> bool {
    FontRef::from_index(data, index)
        .ok()
        .and_then(|font| font.colr().ok())
        .and_then(|table| validate_graph(table.offset_data().as_bytes(), glyph))
        .is_some_and(|(_, foreground)| foreground)
}

#[cfg(test)]
pub(crate) fn render(data: &[u8], index: u32, glyph: u32, pixels: f32) -> Option<Image> {
    render_with_foreground(data, index, glyph, pixels, [0, 0, 0, 255])
}

pub(crate) fn render_with_foreground(
    data: &[u8],
    index: u32,
    glyph: u32,
    pixels: f32,
    foreground: [u8; 4],
) -> Option<Image> {
    let font = FontRef::from_index(data, index).ok()?;
    let table = font.colr().ok()?;
    let (nodes, _) = validate_graph(table.offset_data().as_bytes(), glyph)?;
    let glyph = font
        .color_glyphs()
        .get_with_format(GlyphId::new(glyph), ColorGlyphFormat::ColrV1)?;
    let head = font.head().ok()?;
    let scale = pixels / f32::from(head.units_per_em());
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let bounds = match glyph.bounding_box(LocationRef::default(), Size::unscaled()) {
        Some(bounds) => bounds,
        None => {
            let mut measure = BoundsPainter {
                font: font.clone(),
                transforms: vec![Transform::identity()],
                clips: Vec::new(),
                bounds: None,
                failed: false,
            };
            glyph.paint(LocationRef::default(), &mut measure).ok()?;
            if measure.failed {
                return None;
            }
            let Some(b) = measure.bounds else {
                return Some(empty_image());
            };
            BoundingBox {
                x_min: b.left(),
                y_min: b.top(),
                x_max: b.right(),
                y_max: b.bottom(),
            }
        }
    };
    let left = (bounds.x_min * scale).floor();
    let top = (bounds.y_max * scale).ceil();
    let width = (bounds.x_max * scale).ceil() - left;
    let height = top - (bounds.y_min * scale).floor();
    if ![left, top, width, height].iter().all(|v| v.is_finite())
        || width <= 0.0
        || height <= 0.0
        || width > MAX_SIDE as f32
        || height > MAX_SIDE as f32
        || width * height > MAX_PIXELS as f32
        || width * height * nodes as f32 > MAX_WORK as f32
    {
        return None;
    }
    let (width, height) = (width as u32, height as u32);
    let colors = font
        .color_palettes()
        .get(0)?
        .colors()
        .iter()
        .map(|c| [c.red(), c.green(), c.blue(), c.alpha()])
        .collect();
    let mut painter = Painter {
        font,
        colors,
        foreground,
        transforms: vec![Transform::from_row(scale, 0.0, 0.0, -scale, -left, top)],
        clips: Vec::new(),
        layers: Vec::new(),
        image: Pixmap::new(width, height)?,
        failed: false,
    };
    glyph.paint(LocationRef::default(), &mut painter).ok()?;
    if painter.failed {
        return None;
    }
    // Crop transparent padding so placement is the same convention as swash.
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    for (i, p) in painter.image.pixels().iter().enumerate() {
        if p.alpha() != 0 {
            let (x, y) = (i as u32 % width, i as u32 / width);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + 1);
            max_y = max_y.max(y + 1);
        }
    }
    if max_x == 0 {
        return Some(empty_image());
    }
    let (w, h) = (max_x - min_x, max_y - min_y);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in min_y..max_y {
        let start = ((y * width + min_x) * 4) as usize;
        data.extend_from_slice(&painter.image.data()[start..start + (w * 4) as usize]);
    }
    Some(Image {
        source: Source::ColorOutline(0),
        content: Content::Color,
        placement: Placement {
            left: left as i32 + min_x as i32,
            top: top as i32 - min_y as i32,
            width: w,
            height: h,
        },
        data,
    })
}

struct Pen(PathBuilder);
impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.0.quad_to(cx, cy, x, y);
    }
    fn curve_to(&mut self, a: f32, b: f32, c: f32, d: f32, x: f32, y: f32) {
        self.0.cubic_to(a, b, c, d, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

fn glyph_path(font: &FontRef<'_>, glyph: GlyphId) -> Option<Path> {
    let mut pen = Pen(PathBuilder::new());
    font.outline_glyphs()
        .get(glyph)?
        .draw(
            DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
            &mut pen,
        )
        .ok()?;
    pen.0.finish()
}

struct BoundsPainter<'a> {
    font: FontRef<'a>,
    transforms: Vec<Transform>,
    clips: Vec<Option<Rect>>,
    bounds: Option<Rect>,
    failed: bool,
}
impl BoundsPainter<'_> {
    fn clip(&mut self, rect: Option<Rect>) {
        let rect = rect.and_then(|r| r.transform(*self.transforms.last().unwrap()));
        self.clips.push(match self.clips.last() {
            Some(previous) => previous.and_then(|p| rect.and_then(|r| p.intersect(&r))),
            None => rect,
        });
    }
}
impl ColorPainter for BoundsPainter<'_> {
    fn push_transform(&mut self, t: skrifa::color::Transform) {
        self.transforms.push(
            self.transforms
                .last()
                .unwrap()
                .pre_concat(Transform::from_row(t.xx, t.yx, t.xy, t.yy, t.dx, t.dy)),
        );
    }
    fn pop_transform(&mut self) {
        self.transforms.pop();
    }
    fn push_clip_glyph(&mut self, glyph: GlyphId) {
        let path = glyph_path(&self.font, glyph);
        // Transform the path, not just its bounding rectangle, for rotated outlines.
        let rect = path
            .and_then(|p| p.transform(*self.transforms.last().unwrap()))
            .map(|p| p.bounds());
        self.clips.push(match self.clips.last() {
            Some(previous) => previous.and_then(|p| rect.and_then(|r| p.intersect(&r))),
            None => rect,
        });
    }
    fn push_clip_box(&mut self, b: BoundingBox<f32>) {
        self.clip(Rect::from_ltrb(b.x_min, b.y_min, b.x_max, b.y_max));
    }
    fn pop_clip(&mut self) {
        self.clips.pop();
    }
    fn fill(&mut self, _: Brush<'_>) {
        match self.clips.last() {
            Some(Some(rect)) => {
                self.bounds = self.bounds.map_or(Some(*rect), |b| {
                    Rect::from_ltrb(
                        b.left().min(rect.left()),
                        b.top().min(rect.top()),
                        b.right().max(rect.right()),
                        b.bottom().max(rect.bottom()),
                    )
                });
                if self.bounds.is_none() {
                    self.failed = true;
                }
            }
            Some(None) => {}
            None => self.failed = true,
        }
    }
    fn push_layer(&mut self, _: CompositeMode) {}
}

struct Painter<'a> {
    font: FontRef<'a>,
    colors: Vec<[u8; 4]>,
    foreground: [u8; 4],
    transforms: Vec<Transform>,
    clips: Vec<Mask>,
    layers: Vec<(Pixmap, BlendMode, Option<Vec<Mask>>)>,
    image: Pixmap,
    failed: bool,
}
impl Painter<'_> {
    fn transform(&self) -> Transform {
        *self.transforms.last().unwrap()
    }
    fn clip(&mut self, path: Option<Path>) {
        let Some(mut mask) = Mask::new(self.image.width(), self.image.height()) else {
            self.failed = true;
            return;
        };
        if let Some(path) = path {
            mask.fill_path(&path, FillRule::Winding, true, self.transform());
        }
        if let Some(previous) = self.clips.last() {
            for (a, b) in mask.data_mut().iter_mut().zip(previous.data()) {
                *a = ((u16::from(*a) * u16::from(*b) + 127) / 255) as u8;
            }
        }
        self.clips.push(mask);
    }
    fn color(&self, index: u16, alpha: f32) -> [f32; 4] {
        let color = if index == 0xffff {
            self.foreground
        } else {
            self.colors.get(index as usize).copied().unwrap_or([0; 4])
        };
        let a = f32::from(color[3]) / 255.0 * alpha.clamp(0.0, 1.0);
        [
            // Match Chrome/Skia and Noto CBDT output. CPAL's linear-light
            // interpolation/compositing diverges from these references.
            f32::from(color[0]) / 255.0 * a,
            f32::from(color[1]) / 255.0 * a,
            f32::from(color[2]) / 255.0 * a,
            a,
        ]
    }
    fn sample(&self, stops: &[ColorStop], t: f32, extend: Extend) -> [f32; 4] {
        let t = match extend {
            Extend::Repeat => t.rem_euclid(1.0),
            Extend::Reflect => 1.0 - (t.rem_euclid(2.0) - 1.0).abs(),
            _ => t.clamp(0.0, 1.0),
        };
        let Some(first) = stops.first() else {
            return [0.0; 4];
        };
        let mut previous = first;
        for stop in stops.iter().skip(1) {
            if t < stop.offset {
                let a = self.color(previous.palette_index, previous.alpha);
                let b = self.color(stop.palette_index, stop.alpha);
                let f = ((t - previous.offset) / (stop.offset - previous.offset)).clamp(0.0, 1.0);
                return std::array::from_fn(|i| a[i] + (b[i] - a[i]) * f);
            }
            previous = stop;
        }
        self.color(previous.palette_index, previous.alpha)
    }
    fn brush(&self, brush: &Brush<'_>, p: Point) -> [f32; 4] {
        match brush {
            Brush::Solid {
                palette_index,
                alpha,
            } => self.color(*palette_index, *alpha),
            Brush::LinearGradient {
                p0,
                p1,
                color_stops,
                extend,
            } => {
                let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
                let t = ((p.x - p0.x) * dx + (p.y - p0.y) * dy) / (dx * dx + dy * dy);
                self.sample(color_stops, t, *extend)
            }
            Brush::RadialGradient {
                c0,
                r0,
                c1,
                r1,
                color_stops,
                extend,
            } => {
                let (dx, dy, dr) = (c1.x - c0.x, c1.y - c0.y, r1 - r0);
                let (x, y) = (p.x - c0.x, p.y - c0.y);
                let a = dx * dx + dy * dy - dr * dr;
                let b = -2.0 * (x * dx + y * dy + r0 * dr);
                let c = x * x + y * y - r0 * r0;
                let roots = if a.abs() < 1e-6 {
                    [-c / b; 2]
                } else {
                    let discriminant = b * b - 4.0 * a * c;
                    if discriminant < 0.0 {
                        return [0.0; 4];
                    }
                    [
                        (-b - discriminant.sqrt()) / (2.0 * a),
                        (-b + discriminant.sqrt()) / (2.0 * a),
                    ]
                };
                let t = roots
                    .into_iter()
                    .filter(|t| t.is_finite() && r0 + t * dr >= 0.0)
                    .max_by(f32::total_cmp);
                t.map(|t| self.sample(color_stops, t, *extend))
                    .unwrap_or([0.0; 4])
            }
            Brush::SweepGradient {
                c0,
                start_angle,
                end_angle,
                color_stops,
                extend,
            } => {
                // Skrifa supplies clockwise degrees in the original y-up font space.
                let angle = (-(p.y - c0.y).atan2(p.x - c0.x).to_degrees()).rem_euclid(360.0);
                let t = if start_angle == end_angle {
                    if angle < *start_angle { 0.0 } else { 1.0 }
                } else {
                    (angle - start_angle) / (end_angle - start_angle)
                };
                self.sample(color_stops, t, *extend)
            }
        }
    }
}
impl ColorPainter for Painter<'_> {
    fn push_transform(&mut self, t: skrifa::color::Transform) {
        self.transforms.push(
            self.transform()
                .pre_concat(Transform::from_row(t.xx, t.yx, t.xy, t.yy, t.dx, t.dy)),
        );
    }
    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 {
            self.transforms.pop();
        }
    }
    fn push_clip_glyph(&mut self, glyph: GlyphId) {
        self.clip(glyph_path(&self.font, glyph));
    }
    fn push_clip_box(&mut self, b: BoundingBox<f32>) {
        self.clip(Rect::from_ltrb(b.x_min, b.y_min, b.x_max, b.y_max).map(PathBuilder::from_rect));
    }
    fn pop_clip(&mut self) {
        self.clips.pop();
    }
    fn fill(&mut self, brush: Brush<'_>) {
        if self.failed {
            return;
        }
        let Some(inverse) = self.transform().invert() else {
            return;
        };
        let Some(mut image) = Pixmap::new(self.image.width(), self.image.height()) else {
            self.failed = true;
            return;
        };
        for y in 0..image.height() {
            for x in 0..image.width() {
                let i = (y * image.width() + x) as usize;
                if self.clips.last().is_some_and(|m| m.data()[i] == 0) {
                    continue;
                }
                let mut p = Point::from_xy(x as f32 + 0.5, y as f32 + 0.5);
                inverse.map_point(&mut p);
                let c = self.brush(&brush, p);
                for (byte, value) in image.data_mut()[i * 4..i * 4 + 4].iter_mut().zip(c) {
                    *byte = (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        }
        self.image.draw_pixmap(
            0,
            0,
            image.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            self.clips.last(),
        );
    }
    fn push_layer(&mut self, mode: CompositeMode) {
        if self.layers.len() >= 64 {
            self.failed = true;
            return;
        }
        let Some(layer) = Pixmap::new(self.image.width(), self.image.height()) else {
            self.failed = true;
            return;
        };
        // Skrifa wraps each composite in a SrcOver isolation layer. Its
        // inherited clip belongs to the combined result, not to both inputs.
        let clips = matches!(mode, CompositeMode::SrcOver).then(|| std::mem::take(&mut self.clips));
        self.layers.push((
            std::mem::replace(&mut self.image, layer),
            blend_mode(mode),
            clips,
        ));
    }
    fn pop_layer(&mut self) {
        if let Some((mut parent, mode, clips)) = self.layers.pop() {
            if let Some(clips) = clips {
                self.clips = clips;
            }
            parent.draw_pixmap(
                0,
                0,
                self.image.as_ref(),
                &PixmapPaint {
                    blend_mode: mode,
                    ..Default::default()
                },
                Transform::identity(),
                if mode == BlendMode::SourceOver {
                    self.clips.last()
                } else {
                    None
                },
            );
            self.image = parent;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FONT: &[u8] = include_bytes!("../test-assets/Noto-COLRv1-subset.ttf");

    fn painter() -> Painter<'static> {
        Painter {
            font: FontRef::new(FONT).unwrap(),
            colors: vec![[255, 0, 0, 255], [0, 0, 255, 255]],
            foreground: [0, 0, 0, 255],
            transforms: vec![Transform::identity()],
            clips: vec![],
            layers: vec![],
            image: Pixmap::new(8, 8).unwrap(),
            failed: false,
        }
    }

    #[test]
    fn overflowing_bounds_union_fails_instead_of_panicking() {
        let mut p = BoundsPainter {
            font: FontRef::new(FONT).unwrap(),
            transforms: vec![Transform::identity()],
            clips: vec![],
            bounds: None,
            failed: false,
        };
        for (left, right) in [(-3.0e38, -2.0e38), (2.0e38, 3.0e38)] {
            p.clips
                .push(Some(Rect::from_ltrb(left, 0.0, right, 1.0).unwrap()));
            p.fill(Brush::Solid {
                palette_index: 0,
                alpha: 1.0,
            });
            p.pop_clip();
        }
        assert!(p.failed);
    }

    fn font_with_colr(table: &[u8]) -> Vec<u8> {
        let mut font = FONT.to_vec();
        let count = u16::from_be_bytes(font[4..6].try_into().unwrap()) as usize;
        let at = (0..count)
            .map(|i| 12 + i * 16)
            .find(|i| &font[*i..*i + 4] == b"COLR")
            .unwrap();
        let offset = font.len() as u32;
        font[at + 8..at + 12].copy_from_slice(&offset.to_be_bytes());
        font[at + 12..at + 16].copy_from_slice(&(table.len() as u32).to_be_bytes());
        font.extend_from_slice(table);
        font
    }
    fn table(root: &[u8]) -> Vec<u8> {
        let mut table = vec![0; 34];
        table[..2].copy_from_slice(&1u16.to_be_bytes());
        table[14..18].copy_from_slice(&34u32.to_be_bytes());
        table.extend_from_slice(&1u32.to_be_bytes());
        table.extend_from_slice(&1u16.to_be_bytes());
        table.extend_from_slice(&10u32.to_be_bytes());
        table.extend_from_slice(root);
        table
    }
    #[test]
    fn expanded_dag_and_cycles_are_bounded_before_traversal() {
        for levels in [1usize, 12] {
            let mut data = table(&[1, 2, 0, 0, 0, 0]);
            let list = data.len();
            data[18..22].copy_from_slice(&(list as u32).to_be_bytes());
            data.extend_from_slice(&((levels * 2) as u32).to_be_bytes());
            data.resize(data.len() + levels * 8, 0);
            for level in 0..levels {
                let child = data.len();
                for i in 0..2 {
                    let at = list + 4 + (level * 2 + i) * 4;
                    data[at..at + 4].copy_from_slice(&((child - list) as u32).to_be_bytes());
                }
                if level + 1 == levels {
                    data.extend_from_slice(&[2, 0, 0, 0x40, 0]);
                } else {
                    data.extend_from_slice(&[1, 2]);
                    data.extend_from_slice(&((level as u32 + 1) * 2).to_be_bytes());
                }
            }
            if levels == 1 {
                assert_eq!(validate_graph(&data, 1), Some((3, false)));
            } else {
                assert!(validate_graph(&data, 1).is_none());
            }
            if levels > 1 {
                assert!(render(&font_with_colr(&data), 0, 1, 14.0).is_none());
                // Duplicate glyph records must select the same root as skrifa:
                // three benign records followed by the expensive DAG.
                data.splice(44..44, [0; 18]);
                data[18..22].copy_from_slice(&((list + 18) as u32).to_be_bytes());
                data[34..38].copy_from_slice(&4u32.to_be_bytes());
                let benign = data.len();
                data.extend_from_slice(&[2, 0, 0, 0x40, 0]);
                for i in 0..4 {
                    let at = 38 + i * 6;
                    data[at..at + 2].copy_from_slice(&1u16.to_be_bytes());
                    let offset = if i == 3 { 28 } else { benign - 34 };
                    data[at + 2..at + 6].copy_from_slice(&(offset as u32).to_be_bytes());
                }
                assert!(validate_graph(&data, 1).is_none());
            }
        }
        // A linear chain also expands exponentially inside skrifa's
        // fill_glyph optimization, even without repeated layer references.
        let mut nested = Vec::new();
        for _ in 0..16 {
            nested.extend_from_slice(&[10, 0, 0, 6, 0, 1]);
        }
        nested.extend_from_slice(&[2, 0, 0, 0x40, 0]);
        assert!(validate_graph(&table(&nested), 1).is_none());
        // PaintColrGlyph references its own base glyph.
        let cycle = table(&[11, 0, 1]);
        assert!(validate_graph(&cycle, 1).is_none());
        assert!(render(&font_with_colr(&cycle), 0, 1, 14.0).is_none());
    }
    #[test]
    fn clipless_translated_outline_is_not_clipped_to_head_bounds() {
        let font = FontRef::new(FONT).unwrap();
        let component = (0..font.maxp().unwrap().num_glyphs())
            .find(|id| {
                let mut pen = Pen(PathBuilder::new());
                font.outline_glyphs()
                    .get(GlyphId::new(u32::from(*id)))
                    .is_some_and(|g| {
                        g.draw(
                            DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                            &mut pen,
                        )
                        .is_ok()
                            && pen.0.finish().is_some()
                    })
            })
            .unwrap();
        let mut root = vec![10, 0, 0, 6];
        root.extend_from_slice(&component.to_be_bytes());
        root.extend_from_slice(&[2, 0, 0, 0x40, 0]);
        let plain = font_with_colr(&table(&root));
        let mut translated = vec![14, 0, 0, 8];
        translated.extend_from_slice(&5000i16.to_be_bytes());
        translated.extend_from_slice(&0i16.to_be_bytes());
        translated.extend_from_slice(&root);
        let translated = font_with_colr(&table(&translated));
        let a = render(&plain, 0, 1, 28.0).unwrap();
        let b = render(&translated, 0, 1, 28.0).unwrap();
        let delta = 5000.0 * 28.0 / f32::from(font.head().unwrap().units_per_em());
        assert!((b.placement.left - a.placement.left - delta.round() as i32).abs() <= 1);
        assert!((b.placement.width as i32 - a.placement.width as i32).abs() <= 1);
    }
    #[test]
    fn gradient_and_compositing_match_skia_srgb_reference() {
        let p = painter();
        let midpoint = p.sample(&stops(), 0.5, Extend::Pad);
        assert_eq!((midpoint[0] * 255.0).round() as u8, 128);
        assert_eq!((midpoint[2] * 255.0).round() as u8, 128);
        let mut p = painter();
        p.fill(Brush::Solid {
            palette_index: 0,
            alpha: 1.0,
        });
        p.fill(Brush::Solid {
            palette_index: 1,
            alpha: 0.5,
        });
        let pixel = p.image.pixel(0, 0).unwrap();
        assert!((i16::from(pixel.red()) - 128).abs() <= 1);
        assert!((i16::from(pixel.blue()) - 128).abs() <= 1);
    }
    #[test]
    fn shared_antialiased_clip_applies_after_composite() {
        for mode in [CompositeMode::SrcOut, CompositeMode::SrcIn] {
            let mut p = painter();
            p.push_clip_box(BoundingBox {
                x_min: 0.5,
                y_min: 0.0,
                x_max: 7.5,
                y_max: 8.0,
            });
            let coverage = p.clips.last().unwrap().data()[0];
            assert!(coverage > 0 && coverage < 255);
            p.push_layer(CompositeMode::SrcOver);
            p.fill(Brush::Solid {
                palette_index: 0,
                alpha: 1.0,
            });
            p.push_layer(mode);
            p.fill(Brush::Solid {
                palette_index: 1,
                alpha: 1.0,
            });
            p.pop_layer();
            p.pop_layer();
            if matches!(mode, CompositeMode::SrcOut) {
                assert!(p.image.data().iter().all(|b| *b == 0));
            } else {
                assert_eq!(p.image.pixel(0, 0).unwrap().alpha(), coverage);
            }
        }
    }
    #[test]
    fn foreground_paints_use_run_color_and_alpha() {
        let mut p = painter();
        p.foreground = [255, 255, 255, 128];
        p.fill(Brush::Solid {
            palette_index: 0xffff,
            alpha: 1.0,
        });
        let pixel = p.image.pixel(2, 2).unwrap();
        assert_eq!(pixel.red(), 128);
        assert_eq!(pixel.green(), 128);
        assert_eq!(pixel.alpha(), 128);
    }

    fn stops() -> [ColorStop; 2] {
        [
            ColorStop {
                offset: 0.0,
                palette_index: 0,
                alpha: 1.0,
            },
            ColorStop {
                offset: 1.0,
                palette_index: 1,
                alpha: 1.0,
            },
        ]
    }
    fn close(a: [f32; 4], b: [f32; 4]) {
        for (a, b) in a.into_iter().zip(b) {
            assert!((a - b).abs() < 0.001, "{a} != {b}");
        }
    }
    #[test]
    fn noto_colrv1_glyphs_have_color_and_scaled_bearings() {
        let font = FontRef::new(FONT).unwrap();
        for ch in ['🚀', '🔥', '🎉', '❤', '✅', '🐕', '🌈', '🎨'] {
            let id = font.charmap().map(ch).unwrap().to_u32();
            assert!(has_glyph(FONT, 0, id));
            let small = render(FONT, 0, id, 14.0).unwrap();
            let large = render(FONT, 0, id, 28.0).unwrap();
            assert!(small.placement.width > 0 && small.placement.height > 0);
            assert!(large.placement.width >= small.placement.width * 2 - 2);
            assert!(large.placement.height >= small.placement.height * 2 - 2);
            assert!((large.placement.top - small.placement.top * 2).abs() <= 2);
            assert!((large.placement.left - small.placement.left * 2).abs() <= 2);
            assert!(
                small
                    .data
                    .chunks_exact(4)
                    .any(|p| p[0] != p[1] || p[1] != p[2])
            );
            for p in small.data.chunks_exact(4) {
                assert!(p[..3].iter().all(|v| *v <= p[3]));
            }
        }
    }
    #[test]
    fn rejects_bad_input_and_excessive_allocations() {
        assert!(render(&[], 0, 0, 14.0).is_none());
        assert!(render(FONT, 999, 0, 14.0).is_none());
        let id = FontRef::new(FONT)
            .unwrap()
            .charmap()
            .map('🚀')
            .unwrap()
            .to_u32();
        for size in [0.0, -1.0, f32::NAN, f32::INFINITY, 1e9] {
            assert!(render(FONT, 0, id, size).is_none());
        }
    }
    #[test]
    fn sweep_samples_clockwise_degrees_without_tessellation() {
        let painter = painter();
        let stops = stops();
        let brush = Brush::SweepGradient {
            c0: skrifa::raw::types::Point::new(0.0, 0.0),
            start_angle: 0.0,
            end_angle: 360.0,
            color_stops: &stops,
            extend: Extend::Pad,
        };
        close(
            painter.brush(&brush, Point::from_xy(1.0, 0.0)),
            [1.0, 0.0, 0.0, 1.0],
        );
        close(
            painter.brush(&brush, Point::from_xy(0.0, -1.0)),
            [0.75, 0.0, 0.25, 1.0],
        );
        close(
            painter.brush(&brush, Point::from_xy(-1.0, 0.0)),
            [0.5, 0.0, 0.5, 1.0],
        );
        close(
            painter.brush(&brush, Point::from_xy(0.0, 1.0)),
            [0.25, 0.0, 0.75, 1.0],
        );
        close(
            painter.sample(&stops, 1.25, Extend::Repeat),
            [0.75, 0.0, 0.25, 1.0],
        );
        close(
            painter.sample(&stops, 1.25, Extend::Reflect),
            [0.25, 0.0, 0.75, 1.0],
        );
    }
    #[test]
    fn radial_two_circle_and_linear_gradients() {
        let painter = painter();
        let stops = stops();
        let brush = Brush::RadialGradient {
            c0: skrifa::raw::types::Point::new(0.0, 0.0),
            r0: 2.0,
            c1: skrifa::raw::types::Point::new(0.0, 0.0),
            r1: 6.0,
            color_stops: &stops,
            extend: Extend::Pad,
        };
        close(
            painter.brush(&brush, Point::from_xy(4.0, 0.0)),
            [0.5, 0.0, 0.5, 1.0],
        );
        let brush = Brush::LinearGradient {
            p0: skrifa::raw::types::Point::new(2.0, 0.0),
            p1: skrifa::raw::types::Point::new(6.0, 0.0),
            color_stops: &stops,
            extend: Extend::Pad,
        };
        close(
            painter.brush(&brush, Point::from_xy(4.0, 100.0)),
            [0.5, 0.0, 0.5, 1.0],
        );
    }
    #[test]
    fn clipping_transform_and_composite_layers() {
        let mut painter = painter();
        painter.push_clip_box(BoundingBox {
            x_min: 0.0,
            y_min: 0.0,
            x_max: 4.0,
            y_max: 8.0,
        });
        painter.fill(Brush::Solid {
            palette_index: 0,
            alpha: 0.5,
        });
        assert_eq!(painter.image.pixel(2, 2).unwrap().alpha(), 128);
        assert_eq!(painter.image.pixel(6, 2).unwrap().alpha(), 0);
        painter.pop_clip();
        painter.push_layer(CompositeMode::SrcIn);
        painter.fill(Brush::Solid {
            palette_index: 1,
            alpha: 1.0,
        });
        painter.pop_layer();
        assert_eq!(painter.image.pixel(2, 2).unwrap().blue(), 128);
        assert_eq!(painter.image.pixel(6, 2).unwrap().alpha(), 0);
        painter.push_transform(skrifa::color::Transform {
            dx: 3.0,
            dy: 2.0,
            ..Default::default()
        });
        let mut p = Point::from_xy(1.0, 1.0);
        painter.transform().map_point(&mut p);
        assert_eq!(p, Point::from_xy(4.0, 3.0));
        painter.pop_transform();
        assert_eq!(painter.transform(), Transform::identity());
    }
}
fn blend_mode(mode: CompositeMode) -> BlendMode {
    match mode {
        CompositeMode::Clear => BlendMode::Clear,
        CompositeMode::Src => BlendMode::Source,
        CompositeMode::Dest => BlendMode::Destination,
        CompositeMode::SrcOver => BlendMode::SourceOver,
        CompositeMode::DestOver => BlendMode::DestinationOver,
        CompositeMode::SrcIn => BlendMode::SourceIn,
        CompositeMode::DestIn => BlendMode::DestinationIn,
        CompositeMode::SrcOut => BlendMode::SourceOut,
        CompositeMode::DestOut => BlendMode::DestinationOut,
        CompositeMode::SrcAtop => BlendMode::SourceAtop,
        CompositeMode::DestAtop => BlendMode::DestinationAtop,
        CompositeMode::Xor => BlendMode::Xor,
        CompositeMode::Plus => BlendMode::Plus,
        CompositeMode::Screen => BlendMode::Screen,
        CompositeMode::Overlay => BlendMode::Overlay,
        CompositeMode::Darken => BlendMode::Darken,
        CompositeMode::Lighten => BlendMode::Lighten,
        CompositeMode::ColorDodge => BlendMode::ColorDodge,
        CompositeMode::ColorBurn => BlendMode::ColorBurn,
        CompositeMode::HardLight => BlendMode::HardLight,
        CompositeMode::SoftLight => BlendMode::SoftLight,
        CompositeMode::Difference => BlendMode::Difference,
        CompositeMode::Exclusion => BlendMode::Exclusion,
        CompositeMode::Multiply => BlendMode::Multiply,
        CompositeMode::HslHue => BlendMode::Hue,
        CompositeMode::HslSaturation => BlendMode::Saturation,
        CompositeMode::HslColor => BlendMode::Color,
        CompositeMode::HslLuminosity => BlendMode::Luminosity,
        _ => BlendMode::SourceOver,
    }
}
