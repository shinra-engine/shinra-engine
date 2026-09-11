//! `render/*.so`: nodes and edges. Pure — data plus assets in, buffers out.

use crate::prim::{Slice, Str};

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawKind {
    /// Three vertices, no buffers; the shader builds the triangle. Used for
    /// clears, blits, post and anything that is a function of the targets.
    Fullscreen = 0,
    /// One instance per entity carrying `instance_of`, whose component bytes
    /// arrive as instance-step vertex data.
    Instanced = 1,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Draw {
    pub kind: DrawKind,
    /// Component driving instancing (`DrawKind::Instanced`), else empty.
    pub instance_of: Str,
    /// Mesh asset name, or empty for a unit quad.
    pub mesh: Str,
}

impl Draw {
    pub const FULLSCREEN: Draw = Draw {
        kind: DrawKind::Fullscreen,
        instance_of: Str::EMPTY,
        mesh: Str::EMPTY,
    };
    pub const fn instanced(component: &'static str, mesh: &'static str) -> Draw {
        Draw {
            kind: DrawKind::Instanced,
            instance_of: Str::new(component),
            mesh: Str::new(mesh),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Pass {
    pub name: Str,
    /// WGSL source. Entry points `vs_main` / `fs_main`.
    pub shader: Str,
    /// Buffer names written this frame.
    pub color: Slice<Str>,
    /// Depth buffer name, or empty.
    pub depth: Str,
    /// Sampled buffer names read as textures — always previous-frame content,
    /// which is why reading a buffer you also write is legal.
    pub reads: Slice<Str>,
    /// Asset names to bind as textures — an image an `asset/*.so` published,
    /// as opposed to `reads`, which names buffers this engine rendered. Unlike
    /// a buffer read, this is current: an asset does not change mid-frame.
    pub textures: Slice<Str>,
    /// A component whose first entity is uploaded as a uniform for this pass.
    /// Empty for none. This is how a camera reaches a shader without the
    /// render side being handed the world: it names one component, and the
    /// host resolves it.
    pub uniform_of: Str,
    pub clear: [f32; 4],
    /// Skip the clear and blend over what is there.
    pub load: bool,
    pub draw: Draw,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Edge {
    pub from: Str,
    pub to: Str,
}

impl Edge {
    pub const fn new(from: &'static str, to: &'static str) -> Edge {
        Edge { from: Str::new(from), to: Str::new(to) }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GraphDesc {
    pub name: Str,
    pub passes: Slice<Pass>,
    pub edges: Slice<Edge>,
    /// Buffer whose contents are shown. The one place the graph touches the
    /// outside world.
    pub present: Str,
}

