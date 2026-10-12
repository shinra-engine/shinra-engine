//! A frame of pixels handed to the viewport.

/// One frame of tightly packed 8-bit RGBA.
pub struct Pixels<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}
