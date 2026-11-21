use crate::rdp::{HEIGHT, WIDTH};
use smithay::utils::{Logical, Point, Rectangle};

pub struct Overview {
    pixels: Vec<u8>,
    columns: usize,
    rows: usize,
}

impl Overview {
    pub fn new(windows: usize) -> Self {
        let columns = windows.max(1).isqrt();
        let columns = if windows > columns * columns {
            columns + 1
        } else {
            columns
        };
        let rows = windows.max(1).div_ceil(columns);
        Self {
            pixels: vec![20; WIDTH * HEIGHT * 4],
            columns,
            rows,
        }
    }

    pub fn tile(
        &mut self,
        index: usize,
        area: Rectangle<i32, Logical>,
        rgb: &[u8],
        active: bool,
        pointer: Option<(Point<i32, Logical>, bool)>,
    ) {
        let cell_width = WIDTH / self.columns;
        let cell_height = HEIGHT / self.rows;
        let margin = 6;
        if cell_width <= margin * 2 || cell_height <= margin * 2 {
            return;
        }
        let source_width = area.size.w as usize;
        let source_height = area.size.h as usize;
        let max_width = cell_width - margin * 2;
        let max_height = cell_height - margin * 2;
        let width = max_width
            .min(max_height * source_width / source_height)
            .max(1);
        let height = (width * source_height / source_width).clamp(1, max_height);
        let left = index % self.columns * cell_width + (cell_width - width) / 2;
        let top = index / self.columns * cell_height + (cell_height - height) / 2;
        if active {
            for y in top - 3..top + height + 3 {
                for x in left - 3..left + width + 3 {
                    self.pixel(x, y, [95, 197, 255]);
                }
            }
        }
        for y in 0..height {
            for x in 0..width {
                let source =
                    (y * source_height / height * source_width + x * source_width / width) * 3;
                self.pixel(
                    left + x,
                    top + y,
                    [rgb[source], rgb[source + 1], rgb[source + 2]],
                );
            }
        }
        if let Some((position, clicked)) = pointer.filter(|(position, _)| area.contains(*position))
        {
            let x = left + (position.x - area.loc.x) as usize * width / source_width;
            let y = top + (position.y - area.loc.y) as usize * height / source_height;
            let color = if clicked {
                [255, 152, 56]
            } else {
                [255, 255, 255]
            };
            for offset in -6..=6 {
                let horizontal = x.saturating_add_signed(offset);
                let vertical = y.saturating_add_signed(offset);
                if (left..left + width).contains(&horizontal) {
                    self.pixel(horizontal, y, color);
                }
                if (top..top + height).contains(&vertical) {
                    self.pixel(x, vertical, color);
                }
            }
        }
    }

    fn pixel(&mut self, x: usize, y: usize, color: [u8; 3]) {
        let offset = (y * WIDTH + x) * 4;
        self.pixels[offset..offset + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
    }

    pub fn finish(self) -> Vec<u8> {
        self.pixels
    }
}
