//! HERDR: file drags keep their source machine; remote paths are never local files.

use gpui::{Context, IntoElement, ParentElement, Pixels, Point, Render, Styled, Window, div, px};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct DraggedPaths {
    pub paths: Vec<String>,
    /// None means this computer; otherwise a host-owned machine name.
    pub on: Option<String>,
}

pub(crate) struct DraggedPathsPreview {
    pub paths: DraggedPaths,
    pub cursor_offset: Point<Pixels>,
}

impl Render for DraggedPathsPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = gpui_base::Theme::global(cx).tokens.colors;
        let first = self.paths.paths.first().map(String::as_str).unwrap_or("");
        let name = first.rsplit(['/', '\\']).next().unwrap_or(first);
        let label = if self.paths.paths.len() <= 1 { name.to_owned() } else { format!("{name} (+{})", self.paths.paths.len() - 1) };
        // GPUI subtracts the source-relative grab offset. Undo it for this compact
        // badge, which is not a copy of the (potentially much wider) source row.
        div().child(div().relative().left(self.cursor_offset.x + px(12.)).top(self.cursor_offset.y + px(12.))
            .px(px(8.)).py(px(4.)).rounded(px(6.)).border_1().border_color(colors.border)
            .bg(colors.surface).text_color(colors.surface_foreground).child(label))
    }
}
