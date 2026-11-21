use crate::rdp::{HEIGHT, WIDTH};
use font8x8::{BASIC_FONTS, UnicodeFonts};
use smithay::utils::{Logical, Point, Rectangle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Window(u64),
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub target: Target,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
}

impl Region {
    pub fn contains(self, x: u16, y: u16) -> bool {
        (self.left..self.left + self.width).contains(&usize::from(x))
            && (self.top..self.top + self.height).contains(&usize::from(y))
    }
}

pub struct Frame {
    pub pixels: Vec<u8>,
    pub regions: Vec<Region>,
}

const BAR_HEIGHT: usize = 46;

pub struct Overview {
    pixels: Vec<u8>,
    columns: usize,
    rows: usize,
    inset: usize,
    regions: Vec<Region>,
}

impl Overview {
    pub fn new(windows: usize, bar: Option<(&str, &str)>) -> Self {
        let columns = windows.max(1).isqrt();
        let columns = if windows > columns * columns {
            columns + 1
        } else {
            columns
        };
        let rows = windows.max(1).div_ceil(columns);
        let mut overview = Self {
            pixels: vec![20; WIDTH * HEIGHT * 4],
            columns,
            rows,
            inset: if bar.is_some() { BAR_HEIGHT } else { 0 },
            regions: Vec::new(),
        };
        if let Some((title, active)) = bar {
            overview.fill(0, 0, WIDTH, BAR_HEIGHT, [36, 41, 47]);
            let back = Region {
                target: Target::Back,
                left: 8,
                top: 7,
                width: 156,
                height: 32,
            };
            overview.fill(back.left, back.top, back.width, back.height, [61, 84, 103]);
            overview.text(18, 15, 9, "< Tiles", [245, 248, 251]);
            overview.text(181, 15, 34, title, [245, 248, 251]);
            overview.text(760, 15, 31, active, [130, 210, 255]);
            overview.regions.push(back);
        }
        overview
    }

    pub fn tile(
        &mut self,
        index: usize,
        target: Target,
        area: Rectangle<i32, Logical>,
        rgb: &[u8],
        active: bool,
        pointer: Option<(Point<i32, Logical>, bool)>,
    ) {
        let cell_width = WIDTH / self.columns;
        let cell_height = (HEIGHT - self.inset) / self.rows;
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
        let top = self.inset + index / self.columns * cell_height + (cell_height - height) / 2;
        if self.inset == 0 {
            self.regions.push(Region {
                target,
                left: left - 3,
                top: top - 3,
                width: width + 6,
                height: height + 6,
            });
        }
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

    fn fill(&mut self, left: usize, top: usize, width: usize, height: usize, color: [u8; 3]) {
        for y in top..top + height {
            for x in left..left + width {
                self.pixel(x, y, color);
            }
        }
    }

    fn text(&mut self, left: usize, top: usize, limit: usize, text: &str, color: [u8; 3]) {
        for (index, ch) in text.chars().take(limit).enumerate() {
            let glyph = BASIC_FONTS
                .get(ch)
                .or_else(|| BASIC_FONTS.get('?'))
                .unwrap_or_default();
            for (y, row) in glyph.into_iter().enumerate() {
                for x in (0..8).filter(|x| row & (1 << x) != 0) {
                    self.fill(left + index * 16 + x * 2, top + y * 2, 2, 2, color);
                }
            }
        }
    }

    pub fn finish(self) -> Frame {
        Frame {
            pixels: self.pixels,
            regions: self.regions,
        }
    }
}
