//! Owned copies of everything a module declares.
//!
//! The rule that makes unloading safe: no `Str` or `Slice` survives
//! registration. Names become `String`, asset bodies become `Vec<u8>`, and the
//! only thing still pointing into the image is a function pointer — which is
//! why a `Module` and the specs taken from it are always dropped together.

use se_abi::{
    AssetDesc, BufferDesc, DrawKind, Extent, Format, GraphDesc, Layout,
    Pass, ScalarTy, StageCall, StageSpec,
};

#[derive(Clone, Debug)]
pub struct FieldDef {
    pub name: String,
    pub ty: ScalarTy,
    pub offset: u32,
    pub count: u32,
}

#[derive(Clone, Debug)]
pub struct LayoutDef {
    pub name: String,
    pub size: u32,
    pub align: u32,
    pub fields: Vec<FieldDef>,
    pub hash: u64,
}

impl LayoutDef {
    /// # Safety
    /// `l` must come from a library that is still loaded.
    pub unsafe fn from_abi(l: &Layout) -> LayoutDef {
        LayoutDef {
            name: l.name.as_str().to_string(),
            size: l.size,
            align: l.align,
            fields: l
                .fields
                .as_slice()
                .iter()
                .map(|f| FieldDef {
                    name: f.name.as_str().to_string(),
                    ty: f.ty,
                    offset: f.offset,
                    count: f.count,
                })
                .collect(),
            hash: l.hash,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BufferDef {
    pub name: String,
    pub extent: Extent,
    pub width: u32,
    pub height: u32,
    pub format: Format,
    pub count: u32,
    pub sampled: bool,
}

impl BufferDef {
    /// # Safety
    /// `b` must come from a library that is still loaded.
    pub unsafe fn from_abi(b: &BufferDesc) -> BufferDef {
        BufferDef {
            name: b.name.as_str().to_string(),
            extent: b.extent,
            width: b.width,
            height: b.height,
            format: b.format,
            count: b.count,
            sampled: b.sampled,
        }
    }

    /// Pixel size given the current presentation size.
    pub fn resolve(&self, screen: (u32, u32)) -> (u32, u32) {
        match self.extent {
            Extent::Fixed => (self.width.max(1), self.height.max(1)),
            Extent::Screen => (
                (screen.0 / self.width.max(1)).max(1),
                (screen.1 / self.height.max(1)).max(1),
            ),
        }
    }
}

pub struct AssetDef {
    pub name: String,
    pub bytes: Vec<u8>,
}

impl AssetDef {
    /// # Safety
    /// `a` must come from a library that is still loaded.
    pub unsafe fn from_abi(a: &AssetDesc) -> AssetDef {
        AssetDef {
            name: a.name.as_str().to_string(),
            bytes: a.bytes.as_slice().to_vec(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ParamDef {
    pub name: String,
    pub kind: se_abi::ParamKind,
    pub access: se_abi::Access,
    pub size: u32,
    pub hash: u64,
}

pub struct StageDef {
    pub name: String,
    pub params: Vec<ParamDef>,
    pub run: unsafe extern "C" fn(*const StageCall),
}

impl StageDef {
    /// # Safety
    /// `s` must come from a library that is still loaded.
    pub unsafe fn from_abi(s: &StageSpec) -> StageDef {
        StageDef {
            name: s.name.as_str().to_string(),
            params: s
                .params
                .as_slice()
                .iter()
                .map(|p| ParamDef {
                    name: p.name.as_str().to_string(),
                    kind: p.kind,
                    access: p.access,
                    size: p.size,
                    hash: p.hash,
                })
                .collect(),
            run: s.run,
        }
    }

    /// Component parameters in order — the query, and the column list.
    pub fn components(&self) -> impl Iterator<Item = &ParamDef> {
        self.params
            .iter()
            .filter(|p| p.kind == se_abi::ParamKind::Component)
    }
}

#[derive(Clone, Debug)]
pub struct DrawDef {
    pub kind: DrawKind,
    pub instance_of: Option<String>,
    pub mesh: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PassDef {
    pub name: String,
    pub shader: String,
    pub color: Vec<String>,
    pub depth: Option<String>,
    pub reads: Vec<String>,
    pub textures: Vec<String>,
    pub uniform_of: Option<String>,
    pub clear: [f32; 4],
    pub load: bool,
    pub draw: DrawDef,
}

#[derive(Clone, Debug)]
pub struct GraphDef {
    pub name: String,
    pub passes: Vec<PassDef>,
    pub edges: Vec<(String, String)>,
    pub present: String,
}

/// # Safety
/// `s` must come from a library that is still loaded.
unsafe fn opt(s: se_abi::Str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.as_str().to_string())
    }
}

impl GraphDef {
    /// # Safety
    /// `g` must come from a library that is still loaded.
    pub unsafe fn from_abi(g: &GraphDesc) -> GraphDef {
        GraphDef {
            name: g.name.as_str().to_string(),
            passes: g.passes.as_slice().iter().map(|p| pass_of(p)).collect(),
            edges: g
                .edges
                .as_slice()
                .iter()
                .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
                .collect(),
            present: g.present.as_str().to_string(),
        }
    }
}

unsafe fn pass_of(p: &Pass) -> PassDef {
    PassDef {
        name: p.name.as_str().to_string(),
        shader: p.shader.as_str().to_string(),
        color: p
            .color
            .as_slice()
            .iter()
            .map(|s| s.as_str().to_string())
            .collect(),
        depth: opt(p.depth),
        reads: p
            .reads
            .as_slice()
            .iter()
            .map(|s| s.as_str().to_string())
            .collect(),
        textures: p
            .textures
            .as_slice()
            .iter()
            .map(|s| s.as_str().to_string())
            .collect(),
        uniform_of: opt(p.uniform_of),
        clear: p.clear,
        load: p.load,
        draw: DrawDef {
            kind: p.draw.kind,
            instance_of: opt(p.draw.instance_of),
            mesh: opt(p.draw.mesh),
        },
    }
}


