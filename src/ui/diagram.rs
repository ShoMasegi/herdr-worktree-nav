//! Drawing a predicted tab layout as a box diagram.
//!
//! Panes are drawn so that neighbours share a border, and the shared cells are resolved into
//! the right junction — `┬` where a vertical edge meets a horizontal one, and so on. herdr
//! draws its own pane borders the same way; without it the diagram is a row of separate
//! boxes rather than one divided rectangle.

use crate::port::LayoutRect;

/// Which border segments meet in one cell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Edges {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
}

impl Edges {
    /// The box-drawing character for this combination, or a space when nothing meets here.
    pub fn glyph(self) -> char {
        match (self.up, self.down, self.left, self.right) {
            (false, false, false, false) => ' ',
            (true, true, true, true) => '\u{253c}',   // ┼
            (true, true, true, false) => '\u{2524}',  // ┤
            (true, true, false, true) => '\u{251c}',  // ├
            (true, false, true, true) => '\u{2534}',  // ┴
            (false, true, true, true) => '\u{252c}',  // ┬
            (true, false, true, false) => '\u{2518}', // ┘
            (true, false, false, true) => '\u{2514}', // └
            (false, true, true, false) => '\u{2510}', // ┐
            (false, true, false, true) => '\u{250c}', // ┌
            (true, true, false, false) => '\u{2502}', // │
            (false, false, true, true) => '\u{2500}', // ─
            // A stub with only one arm: draw it as the line it continues.
            (true, false, false, false) | (false, true, false, false) => '\u{2502}',
            (false, false, true, false) | (false, false, false, true) => '\u{2500}',
        }
    }
}

/// A grid of border segments, sized in cells.
pub struct Frame {
    width: usize,
    height: usize,
    cells: Vec<Edges>,
}

impl Frame {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![Edges::default(); width * height],
        }
    }

    /// Add one rectangle's border. `rect` is inclusive of its own border, so a neighbour
    /// sharing an edge adds segments to the same cells and the junction resolves itself.
    pub fn add(&mut self, rect: LayoutRect) {
        if rect.width < 2 || rect.height < 2 {
            return;
        }
        let (x0, y0) = (rect.x as usize, rect.y as usize);
        let x1 = x0 + rect.width as usize - 1;
        let y1 = y0 + rect.height as usize - 1;

        for x in x0..=x1 {
            if x > x0 {
                self.mark(x, y0, |e| e.left = true);
                self.mark(x, y1, |e| e.left = true);
            }
            if x < x1 {
                self.mark(x, y0, |e| e.right = true);
                self.mark(x, y1, |e| e.right = true);
            }
        }
        for y in y0..=y1 {
            if y > y0 {
                self.mark(x0, y, |e| e.up = true);
                self.mark(x1, y, |e| e.up = true);
            }
            if y < y1 {
                self.mark(x0, y, |e| e.down = true);
                self.mark(x1, y, |e| e.down = true);
            }
        }
    }

    fn mark(&mut self, x: usize, y: usize, f: impl Fn(&mut Edges)) {
        if x < self.width && y < self.height {
            f(&mut self.cells[y * self.width + x]);
        }
    }

    pub fn glyph_at(&self, x: usize, y: usize) -> char {
        if x >= self.width || y >= self.height {
            return ' ';
        }
        self.cells[y * self.width + x].glyph()
    }
}

/// Fit a rectangle from the tab's coordinate space into the canvas, keeping the proportions
/// so the diagram is shaped like the tab it stands for.
pub struct Fit {
    area: LayoutRect,
    width: usize,
    height: usize,
    offset_x: usize,
    offset_y: usize,
}

impl Fit {
    pub fn new(area: LayoutRect, canvas_width: usize, canvas_height: usize) -> Option<Self> {
        if area.width == 0 || area.height == 0 || canvas_width < 4 || canvas_height < 3 {
            return None;
        }
        // Both dimensions are already in cells, so one scale keeps the shape on screen.
        let by_width = canvas_width * 1000 / area.width as usize;
        let by_height = canvas_height * 1000 / area.height as usize;
        let scale = by_width.min(by_height);
        let width = (area.width as usize * scale / 1000)
            .max(2)
            .min(canvas_width);
        let height = (area.height as usize * scale / 1000)
            .max(2)
            .min(canvas_height);
        Some(Self {
            area,
            width,
            height,
            // Left-aligned so the diagram lines up with the caption above it; centred
            // vertically so it is not pinned to the top of a tall panel.
            offset_x: 0,
            offset_y: (canvas_height - height) / 2,
        })
    }

    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub fn offset(&self) -> (usize, usize) {
        (self.offset_x, self.offset_y)
    }

    /// Map a pane rectangle in. Edges are mapped rather than positions and sizes, so two
    /// panes that touched in the tab still touch in the diagram.
    pub fn map(&self, rect: LayoutRect) -> LayoutRect {
        let left = self.scale_x(rect.x);
        let top = self.scale_y(rect.y);
        let right = self.scale_x(rect.x.saturating_add(rect.width));
        let bottom = self.scale_y(rect.y.saturating_add(rect.height));
        LayoutRect {
            x: left as u16,
            y: top as u16,
            // Inclusive of the shared border, so neighbours overlap by exactly one cell.
            width: (right.saturating_sub(left) + 1) as u16,
            height: (bottom.saturating_sub(top) + 1) as u16,
        }
    }

    fn scale_x(&self, x: u16) -> usize {
        let offset = x.saturating_sub(self.area.x) as usize;
        (offset * (self.width - 1) / self.area.width as usize).min(self.width - 1)
    }

    fn scale_y(&self, y: u16) -> usize {
        let offset = y.saturating_sub(self.area.y) as usize;
        (offset * (self.height - 1) / self.area.height as usize).min(self.height - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u16, y: u16, width: u16, height: u16) -> LayoutRect {
        LayoutRect {
            x,
            y,
            width,
            height,
        }
    }

    /// Render a frame to text, for tests.
    fn draw(frame: &Frame, width: usize, height: usize) -> String {
        (0..height)
            .map(|y| (0..width).map(|x| frame.glyph_at(x, y)).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_single_rectangle_is_a_plain_box() {
        let mut frame = Frame::new(6, 3);
        frame.add(rect(0, 0, 6, 3));
        assert_eq!(draw(&frame, 6, 3), "┌────┐\n│    │\n└────┘");
    }

    #[test]
    fn two_rectangles_sharing_an_edge_meet_at_a_junction() {
        // The shared column is one cell wide, so the corners have to resolve to ┬ and ┴
        // rather than each box drawing its own.
        let mut frame = Frame::new(9, 3);
        frame.add(rect(0, 0, 5, 3));
        frame.add(rect(4, 0, 5, 3));
        assert_eq!(draw(&frame, 9, 3), "┌───┬───┐\n│   │   │\n└───┴───┘");
    }

    #[test]
    fn a_pane_split_downwards_meets_at_the_sides() {
        let mut frame = Frame::new(5, 5);
        frame.add(rect(0, 0, 5, 3));
        frame.add(rect(0, 2, 5, 3));
        assert_eq!(draw(&frame, 5, 5), "┌───┐\n│   │\n├───┤\n│   │\n└───┘");
    }

    #[test]
    fn four_rectangles_meeting_at_a_point_resolve_to_a_cross() {
        let mut frame = Frame::new(9, 5);
        for r in [
            rect(0, 0, 5, 3),
            rect(4, 0, 5, 3),
            rect(0, 2, 5, 3),
            rect(4, 2, 5, 3),
        ] {
            frame.add(r);
        }
        assert_eq!(
            draw(&frame, 9, 5),
            "┌───┬───┐\n│   │   │\n├───┼───┤\n│   │   │\n└───┴───┘"
        );
    }

    #[test]
    fn fitting_keeps_the_shape_of_the_tab_it_stands_for() {
        // A 250x79 tab is about 3:1; the diagram must be too, not stretched to the canvas.
        let fit = Fit::new(rect(0, 0, 250, 79), 100, 50).unwrap();
        let (width, height) = fit.size();
        assert!(height < 50, "not stretched to the full canvas height");
        let tab_ratio = 250.0 / 79.0;
        let drawn_ratio = width as f64 / height as f64;
        assert!(
            (drawn_ratio - tab_ratio).abs() < 0.35,
            "{drawn_ratio} should be close to {tab_ratio}"
        );
    }

    #[test]
    fn panes_that_touched_in_the_tab_still_touch_in_the_diagram() {
        let fit = Fit::new(rect(0, 0, 100, 40), 40, 12).unwrap();
        let left = fit.map(rect(0, 0, 50, 40));
        let right = fit.map(rect(50, 0, 50, 40));
        assert_eq!(
            left.x + left.width - 1,
            right.x,
            "they should share exactly one column"
        );
    }

    #[test]
    fn a_canvas_too_small_to_say_anything_produces_no_fit() {
        assert!(Fit::new(rect(0, 0, 100, 40), 3, 10).is_none());
        assert!(Fit::new(rect(0, 0, 100, 40), 40, 2).is_none());
        assert!(Fit::new(rect(0, 0, 0, 40), 40, 10).is_none());
    }
}
