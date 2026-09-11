//! The WGSL the engine prepends to every pass.
//!
//! A pass shader declares no bindings and no vertex layout. It is handed frame
//! globals, one texture per buffer it said it reads, a `SeInstance` matching
//! the component it said it draws, and a `SeUniform` matching the component it
//! said to read. Every one of those is generated from something the module
//! already declared, so a shader and its bindings cannot drift apart.

use crate::attrs::{Instancing, Uniform};
use se_host::registry::PassDef;

pub const VS_FULLSCREEN: &str = "se_vs_fullscreen";
pub const VS_MAIN: &str = "vs_main";
pub const FS_ENTRY: &str = "fs_main";

const HEAD: &str = r#"
// ---- injected by shinra ------------------------------------------------
struct SeGlobals {
    resolution : vec2<f32>,
    time       : f32,
    dt         : f32,
    frame      : f32,
    _pad0      : f32,
    _pad1      : f32,
    _pad2      : f32,
};
@group(0) @binding(0) var<uniform> se : SeGlobals;

struct SeVsOut {
    @builtin(position) pos : vec4<f32>,
    @location(0)       uv  : vec2<f32>,
};

// One oversized triangle. No vertex buffer, no draw state.
@vertex
fn se_vs_fullscreen(@builtin(vertex_index) vi : u32) -> SeVsOut {
    var out : SeVsOut;
    let x = f32((vi << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vi & 2u) * 2.0 - 1.0;
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    out.uv  = vec2<f32>((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}

// Mesh vertices, as `asset/*.so` delivered them.
struct SeVertex {
    @location(0) pos    : vec3<f32>,
    @location(1) normal : vec3<f32>,
    @location(2) uv     : vec2<f32>,
};

// Rotate v by unit quaternion q (xyzw).
fn se_qrot(q : vec4<f32>, v : vec3<f32>) -> vec3<f32> {
    let t = 2.0 * cross(q.xyz, v);
    return v + q.w * t + cross(q.xyz, t);
}

fn se_perspective(fov_y : f32, aspect : f32, near : f32, far : f32) -> mat4x4<f32> {
    let f = 1.0 / tan(fov_y * 0.5);
    return mat4x4<f32>(
        vec4<f32>(f / aspect, 0.0, 0.0,                        0.0),
        vec4<f32>(0.0,        f,   0.0,                        0.0),
        vec4<f32>(0.0,        0.0, far / (near - far),        -1.0),
        vec4<f32>(0.0,        0.0, near * far / (near - far),  0.0),
    );
}

fn se_look_at(eye : vec3<f32>, at : vec3<f32>, up : vec3<f32>) -> mat4x4<f32> {
    let f = normalize(at - eye);
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    return mat4x4<f32>(
        vec4<f32>(s.x, u.x, -f.x, 0.0),
        vec4<f32>(s.y, u.y, -f.y, 0.0),
        vec4<f32>(s.z, u.z, -f.z, 0.0),
        vec4<f32>(-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0),
    );
}

// Orthographic, for 2D. Units are world units, origin at the centre.
fn se_ortho(half_w : f32, half_h : f32, near : f32, far : f32) -> mat4x4<f32> {
    return mat4x4<f32>(
        vec4<f32>(1.0 / half_w, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 1.0 / half_h, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0 / (near - far), 0.0),
        vec4<f32>(0.0, 0.0, near / (near - far), 1.0),
    );
}
"#;

/// Build the full source for one pass.
pub fn compose(pass: &PassDef, inst: Option<&Instancing>, uni: Option<&Uniform>) -> String {
    use crate::image_asset::shader_name;
    let mut s = String::with_capacity(HEAD.len() + pass.shader.len() + 512);
    s.push_str(HEAD);

    if !pass.reads.is_empty() || !pass.textures.is_empty() {
        s.push_str("@group(1) @binding(0) var se_sampler : sampler;\n");
        for (i, name) in pass.reads.iter().enumerate() {
            // Previous-frame contents, so a pass may name a buffer it writes.
            s.push_str(&format!(
                "@group(1) @binding({}) var {} : texture_2d<f32>;\n",
                i + 1,
                name
            ));
        }
        // Asset images follow the buffer reads in the same group, sharing the
        // one sampler. Named after the asset, minus its extension.
        for (i, name) in pass.textures.iter().enumerate() {
            s.push_str(&format!(
                "@group(1) @binding({}) var {} : texture_2d<f32>;\n",
                pass.reads.len() + i + 1,
                shader_name(name)
            ));
        }
    }
    if let Some(i) = inst {
        s.push_str(&i.wgsl);
    }
    if let Some(u) = uni {
        s.push_str(&u.wgsl);
        s.push_str("@group(2) @binding(0) var<uniform> u : SeUniform;\n");
    }
    s.push_str("// ---- module ----------------------------------------------------------\n");
    s.push_str(&pass.shader);
    s
}
