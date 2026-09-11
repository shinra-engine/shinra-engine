//! Framebuffer to terminal cells — ported from the original engine's
//! `textart.rs`, which is where the IDE's look actually came from.
//!
//! Three ways to spend a character cell:
//!
//! | mode | subpixels | glyphs |
//! |---|---|---|
//! | `Quadrant` | 2x2 | `▘▝▀▖▌▞▛…█` — solid, blocky, reads at a glance |
//! | `Braille`  | 2x4 | `U+2800` dots — twice the vertical detail, dimmer |
//! | `Mixed`    | 2x4 | braille where dim, quadrants where bright |
//!
//! `Mixed` is the default because a scene has both: silhouettes want the
//! resolution of braille, lit surfaces want the solidity of a block.

//! Convert a rendered RGBA frame into colored Unicode text-art cells, so a
//! terminal can show the viewport as text instead of solid color blocks.
//!
//! Glyph sets:
//! - `Quadrant` — 2x2 subpixels per cell, 16 block glyphs (` ▘▝▀▖▌▞▛▗▚▐▜▄▙▟█`).
//! - `Braille`  — 2x4 dots per cell, 256 glyphs (U+2800..=U+28FF), 8x the
//!   resolution of one character cell and reads as dots rather than blocks.
//! - `Mixed`    — brightness picks the set per cell: dim cells use sparse
//!   braille dots, bright cells use solid quadrant blocks, so ink density
//!   doubles as shading.
//!
//! Pure CPU, no wgpu — testable without a GPU adapter.


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextArtMode {
    Quadrant,
    Braille,
    Mixed,
}

impl TextArtMode {
    /// Subpixels covered by one character cell, as (x, y).
    pub fn cell_resolution(self) -> (u32, u32) {
        match self {
            Self::Quadrant => (2, 2),
            Self::Braille | Self::Mixed => (2, 4),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextCell {
    pub ch: char,
    /// Average color of the lit subpixels; [0, 0, 0] when none are lit.
    pub rgb: [u8; 3],
}

/// Luminance above which a subpixel counts as "lit". The readback is
/// sRGB-encoded (Rgba8UnormSrgb target), so the linear clear color
/// (0.05, 0.05, 0.07) arrives as ~(63, 63, 73) — luminance ~64 — while the
/// darkest shaded geometry (Lambert floor 0.15 × base 0.85) is ~97.
pub const DEFAULT_THRESHOLD: u8 = 70;

/// In `Mixed` mode, cells whose average lit luminance reaches this value are
/// drawn as solid quadrant blocks; dimmer cells stay as braille dots. Shaded
/// geometry spans ~97 (Lambert floor) to ~235 (full light) in sRGB bytes.
pub const MIXED_BLOCK_THRESHOLD: u8 = 170;

/// Quadrant glyphs indexed by subpixel bits: UL=1, UR=2, LL=4, LR=8.
const QUADRANTS: [char; 16] = [
    ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
];

/// Braille dot bit for subpixel (dx, dy), per the U+2800 block layout
/// (dots 1-3 + 7 in the left column, 4-6 + 8 in the right).
const BRAILLE_BITS: [[u32; 2]; 4] = [
    [0x01, 0x08],
    [0x02, 0x10],
    [0x04, 0x20],
    [0x40, 0x80],
];

fn luminance(r: u8, g: u8, b: u8) -> u8 {
    ((2126 * r as u32 + 7152 * g as u32 + 722 * b as u32) / 10000) as u8
}

/// Collapse 2x4 braille dot bits into 2x2 quadrant bits: a quadrant lights
/// up if either of the two braille rows it covers has its dot set.
fn braille_to_quadrant_bits(bits: u32) -> u32 {
    let mut q = 0;
    if bits & (0x01 | 0x02) != 0 {
        q |= 1; // upper-left
    }
    if bits & (0x08 | 0x10) != 0 {
        q |= 2; // upper-right
    }
    if bits & (0x04 | 0x40) != 0 {
        q |= 4; // lower-left
    }
    if bits & (0x20 | 0x80) != 0 {
        q |= 8; // lower-right
    }
    q
}

/// Decode the packed u32 cells produced by the GPU pass
/// (`textart_gpu::TextArtGpu`, packing `bits << 24 | r << 16 | g << 8 | b`)
/// into glyphs. The GPU always emits 2x4 braille dot bits; `Quadrant` and
/// bright `Mixed` cells collapse them to 2x2 block glyphs here.
pub fn packed_to_cells(packed: &[u32], cols: usize, mode: TextArtMode) -> Vec<Vec<TextCell>> {
    if cols == 0 {
        return Vec::new();
    }
    packed
        .chunks(cols)
        .map(|row| {
            row.iter()
                .map(|&p| {
                    let bits = p >> 24;
                    let rgb = [(p >> 16) as u8, (p >> 8) as u8, p as u8];
                    TextCell {
                        ch: glyph_for(bits, rgb, mode),
                        rgb,
                    }
                })
                .collect()
        })
        .collect()
}

/// Pick a glyph for a cell given its braille dot bits and sRGB color.
fn glyph_for(bits: u32, rgb: [u8; 3], mode: TextArtMode) -> char {
    if bits == 0 {
        return match mode {
            TextArtMode::Braille => '\u{2800}',
            _ => ' ',
        };
    }
    match mode {
        TextArtMode::Quadrant => QUADRANTS[braille_to_quadrant_bits(bits) as usize],
        TextArtMode::Braille => char::from_u32(0x2800 + bits).unwrap(),
        TextArtMode::Mixed => {
            if luminance(rgb[0], rgb[1], rgb[2]) >= MIXED_BLOCK_THRESHOLD {
                QUADRANTS[braille_to_quadrant_bits(bits) as usize]
            } else {
                char::from_u32(0x2800 + bits).unwrap()
            }
        }
    }
}

/// CPU reference implementation of the GPU pass — kept for tests and as
/// documentation of the cell semantics. Downsamples `img` into character
/// cells; trailing pixels that don't fill a whole cell are dropped.
/// A tightly packed 8-bit RGBA framebuffer.
pub struct Frame<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

impl Frame<'_> {
    #[inline]
    fn px(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y.min(self.height.saturating_sub(1)) * self.width
            + x.min(self.width.saturating_sub(1)))
            * 4) as usize;
        match self.rgba.get(i..i + 4) {
            Some(p) => [p[0], p[1], p[2], p[3]],
            None => [0, 0, 0, 255],
        }
    }
}

pub fn image_to_cells(img: &Frame, mode: TextArtMode, threshold: u8) -> Vec<Vec<TextCell>> {
    let (sx, sy) = mode.cell_resolution();
    let cols = img.width / sx;
    let rows = img.height / sy;

    let mut out = Vec::with_capacity(rows as usize);
    for cy in 0..rows {
        let mut row = Vec::with_capacity(cols as usize);
        for cx in 0..cols {
            let mut bits: u32 = 0;
            let mut sum = [0u32; 3];
            let mut lit = 0u32;
            for dy in 0..sy {
                for dx in 0..sx {
                    let p = img.px(cx * sx + dx, cy * sy + dy);
                    if luminance(p[0], p[1], p[2]) > threshold {
                        bits |= match mode {
                            TextArtMode::Quadrant => 1 << (dy * 2 + dx),
                            TextArtMode::Braille | TextArtMode::Mixed => {
                                BRAILLE_BITS[dy as usize][dx as usize]
                            }
                        };
                        sum[0] += p[0] as u32;
                        sum[1] += p[1] as u32;
                        sum[2] += p[2] as u32;
                        lit += 1;
                    }
                }
            }
            let ch = match mode {
                TextArtMode::Quadrant => QUADRANTS[bits as usize],
                TextArtMode::Braille => char::from_u32(0x2800 + bits).unwrap(),
                TextArtMode::Mixed => {
                    let avg_lum = if lit > 0 {
                        luminance(
                            (sum[0] / lit) as u8,
                            (sum[1] / lit) as u8,
                            (sum[2] / lit) as u8,
                        )
                    } else {
                        0
                    };
                    if lit == 0 {
                        ' '
                    } else if avg_lum >= MIXED_BLOCK_THRESHOLD {
                        QUADRANTS[braille_to_quadrant_bits(bits) as usize]
                    } else {
                        char::from_u32(0x2800 + bits).unwrap()
                    }
                }
            };
            let rgb = if lit > 0 {
                [
                    (sum[0] / lit) as u8,
                    (sum[1] / lit) as u8,
                    (sum[2] / lit) as u8,
                ]
            } else {
                [0, 0, 0]
            };
            row.push(TextCell { ch, rgb });
        }
        out.push(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A framebuffer, not an `image::RgbaImage`. The port took `Frame` — a
    /// width, a height and borrowed RGBA bytes — precisely so the TUI need not
    /// depend on an image library, and these tests had been left behind
    /// speaking the old dialect against a dependency the crate does not have.
    struct Buf {
        w: u32,
        h: u32,
        px: Vec<u8>,
    }

    impl Buf {
        fn frame(&self) -> Frame<'_> {
            Frame { width: self.w, height: self.h, rgba: &self.px }
        }
        fn put(&mut self, x: u32, y: u32, c: [u8; 4]) {
            let i = ((y * self.w + x) * 4) as usize;
            self.px[i..i + 4].copy_from_slice(&c);
        }
    }

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Buf {
        Buf { w, h, px: c.repeat((w * h) as usize) }
    }

    #[test]
    fn cell_grid_dimensions() {
        let img = solid(8, 8, [0, 0, 0, 255]);
        let q = image_to_cells(&img.frame(), TextArtMode::Quadrant, DEFAULT_THRESHOLD);
        assert_eq!((q.len(), q[0].len()), (4, 4));
        let b = image_to_cells(&img.frame(), TextArtMode::Braille, DEFAULT_THRESHOLD);
        assert_eq!((b.len(), b[0].len()), (2, 4));
    }

    #[test]
    fn black_image_is_blank() {
        let img = solid(4, 8, [0, 0, 0, 255]);
        let q = image_to_cells(&img.frame(), TextArtMode::Quadrant, DEFAULT_THRESHOLD);
        assert!(q.iter().flatten().all(|c| c.ch == ' '));
        let b = image_to_cells(&img.frame(), TextArtMode::Braille, DEFAULT_THRESHOLD);
        assert!(b.iter().flatten().all(|c| c.ch == '\u{2800}'));
    }

    #[test]
    fn white_image_is_full() {
        let img = solid(4, 8, [255, 255, 255, 255]);
        let q = image_to_cells(&img.frame(), TextArtMode::Quadrant, DEFAULT_THRESHOLD);
        assert!(q.iter().flatten().all(|c| c.ch == '█' && c.rgb == [255, 255, 255]));
        let b = image_to_cells(&img.frame(), TextArtMode::Braille, DEFAULT_THRESHOLD);
        assert!(b.iter().flatten().all(|c| c.ch == '\u{28FF}'));
    }

    #[test]
    fn quadrant_single_subpixel_upper_left() {
        let mut img = solid(2, 2, [0, 0, 0, 255]);
        img.put(0, 0, [255, 0, 0, 255]);
        // Pure red has luminance ~54; use an explicit lower threshold.
        let cells = image_to_cells(&img.frame(), TextArtMode::Quadrant, 40);
        assert_eq!(cells[0][0].ch, '▘');
        assert_eq!(cells[0][0].rgb, [255, 0, 0]);
    }

    #[test]
    fn braille_dot_positions() {
        // dot 1 = (0,0), dot 8 = (1,3)
        let mut img = solid(2, 4, [0, 0, 0, 255]);
        img.put(0, 0, [255, 255, 255, 255]);
        let cells = image_to_cells(&img.frame(), TextArtMode::Braille, DEFAULT_THRESHOLD);
        assert_eq!(cells[0][0].ch, '\u{2801}');

        let mut img = solid(2, 4, [0, 0, 0, 255]);
        img.put(1, 3, [255, 255, 255, 255]);
        let cells = image_to_cells(&img.frame(), TextArtMode::Braille, DEFAULT_THRESHOLD);
        assert_eq!(cells[0][0].ch, '\u{2880}');
    }

    #[test]
    fn mixed_dim_cells_use_braille() {
        // Luminance ~97 (Lambert floor on the warm base) — lit but dim.
        let img = solid(2, 4, [97, 97, 97, 255]);
        let cells = image_to_cells(&img.frame(), TextArtMode::Mixed, DEFAULT_THRESHOLD);
        assert_eq!(cells[0][0].ch, '\u{28FF}');
    }

    #[test]
    fn mixed_bright_cells_use_quadrants() {
        let img = solid(2, 4, [230, 230, 230, 255]);
        let cells = image_to_cells(&img.frame(), TextArtMode::Mixed, DEFAULT_THRESHOLD);
        assert_eq!(cells[0][0].ch, '█');
    }

    #[test]
    fn mixed_bright_partial_cell_collapses_to_quadrant() {
        // Light only the left column (dots 1,2,3,7) of a bright cell — the
        // quadrant collapse must produce the left-half block.
        let mut img = solid(2, 4, [0, 0, 0, 255]);
        for y in 0..4 {
            img.put(0, y, [255, 255, 255, 255]);
        }
        let cells = image_to_cells(&img.frame(), TextArtMode::Mixed, DEFAULT_THRESHOLD);
        assert_eq!(cells[0][0].ch, '▌');
    }

    #[test]
    fn mixed_unlit_cell_is_space() {
        let img = solid(2, 4, [0, 0, 0, 255]);
        let cells = image_to_cells(&img.frame(), TextArtMode::Mixed, DEFAULT_THRESHOLD);
        assert_eq!(cells[0][0].ch, ' ');
    }

    #[test]
    fn dark_pixels_stay_below_threshold() {
        // The sRGB-encoded clear color ≈ (63, 63, 73) must not light up.
        let img = solid(2, 2, [63, 63, 73, 255]);
        let cells = image_to_cells(&img.frame(), TextArtMode::Quadrant, DEFAULT_THRESHOLD);
        assert_eq!(cells[0][0].ch, ' ');
    }

    fn pack(bits: u32, [r, g, b]: [u8; 3]) -> u32 {
        (bits << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
    }

    #[test]
    fn packed_decode_braille_and_color() {
        let cells = packed_to_cells(&[pack(0x01, [200, 10, 10])], 1, TextArtMode::Braille);
        assert_eq!(cells[0][0].ch, '\u{2801}');
        assert_eq!(cells[0][0].rgb, [200, 10, 10]);
    }

    #[test]
    fn packed_decode_quadrant_collapses_braille_bits() {
        // Left column fully lit (dots 1,2,3,7) -> left-half block.
        let left = 0x01 | 0x02 | 0x04 | 0x40;
        let cells = packed_to_cells(&[pack(left, [255, 255, 255])], 1, TextArtMode::Quadrant);
        assert_eq!(cells[0][0].ch, '▌');
    }

    #[test]
    fn packed_decode_mixed_picks_set_by_luminance() {
        let full = 0xFF;
        let dim = packed_to_cells(&[pack(full, [97, 97, 97])], 1, TextArtMode::Mixed);
        assert_eq!(dim[0][0].ch, '\u{28FF}');
        let bright = packed_to_cells(&[pack(full, [230, 230, 230])], 1, TextArtMode::Mixed);
        assert_eq!(bright[0][0].ch, '█');
    }

    #[test]
    fn packed_decode_unlit_cells() {
        let cells = packed_to_cells(&[0, 0], 2, TextArtMode::Mixed);
        assert_eq!(cells[0][0].ch, ' ');
        let cells = packed_to_cells(&[0], 1, TextArtMode::Braille);
        assert_eq!(cells[0][0].ch, '\u{2800}');
    }

    #[test]
    fn packed_rows_split_by_cols() {
        let cells = packed_to_cells(&[0, 0, 0, 0, 0, 0], 3, TextArtMode::Quadrant);
        assert_eq!((cells.len(), cells[0].len()), (2, 3));
    }
}
