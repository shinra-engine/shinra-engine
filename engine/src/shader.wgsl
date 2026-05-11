struct Camera { view_proj: mat4x4<f32>, };
struct Object { model: mat4x4<f32>, };

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> object: Object;

struct VsIn  { @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32> };
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) world_normal: vec3<f32> };

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world_pos = object.model * vec4<f32>(in.pos, 1.0);
    out.clip = camera.view_proj * world_pos;
    out.world_normal = (object.model * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let light_dir = normalize(vec3<f32>(0.4, 0.7, 0.6));
    let n = normalize(in.world_normal);
    let lambert = max(dot(n, light_dir), 0.15);
    let base = vec3<f32>(0.85, 0.78, 0.62);
    return vec4<f32>(base * lambert, 1.0);
}
