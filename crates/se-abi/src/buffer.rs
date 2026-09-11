//! `bundle.so` half two: render targets, from `buffer.rs`.
//!
//! A buffer declared `sampled` may be read by a pass as a texture — but always
//! at its *previous frame* contents. That is the whole trick that keeps the
//! render graph a DAG: a mirror showing a mirror is not a cycle, it is a one
//! frame delay, and deep recursion just looks like slow light.

use crate::prim::{Slice, Str};

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Rgba8Unorm = 0,
    Rgba8UnormSrgb = 1,
    Rgba16Float = 2,
    R32Float = 3,
    Depth32Float = 4,
}

impl Format {
    pub const fn is_depth(self) -> bool {
        matches!(self, Format::Depth32Float)
    }
}

/// `Screen` tracks the presentation size; `Fixed` does not.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extent {
    Screen = 0,
    Fixed = 1,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BufferDesc {
    pub name: Str,
    pub extent: Extent,
    /// Used when `extent == Fixed`; a divisor of the screen when `Screen`
    /// (1 = full size, 2 = half, ...).
    pub width: u32,
    pub height: u32,
    pub format: Format,
    /// Array layers. 1 for an ordinary target, 6 for a cube, N for a cascade.
    pub count: u32,
    /// Readable as a texture by a later frame.
    pub sampled: bool,
}

impl BufferDesc {
    pub const fn screen(name: &'static str, format: Format) -> BufferDesc {
        BufferDesc {
            name: Str::new(name),
            extent: Extent::Screen,
            width: 1,
            height: 1,
            format,
            count: 1,
            sampled: false,
        }
    }
    pub const fn fixed(name: &'static str, w: u32, h: u32, format: Format) -> BufferDesc {
        BufferDesc {
            name: Str::new(name),
            extent: Extent::Fixed,
            width: w,
            height: h,
            format,
            count: 1,
            sampled: false,
        }
    }
    pub const fn sampled(mut self) -> BufferDesc {
        self.sampled = true;
        self
    }
    pub const fn count(mut self, n: u32) -> BufferDesc {
        self.count = n;
        self
    }
}


/// `asset/*.so`: name → bytes. Content, never data.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AssetDesc {
    pub name: Str,
    pub bytes: Slice<u8>,
}

impl AssetDesc {
    pub const fn new(name: &'static str, bytes: &'static [u8]) -> AssetDesc {
        AssetDesc { name: Str::new(name), bytes: Slice::new(bytes) }
    }
}

