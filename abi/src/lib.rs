// gametok C ABI — shared between the runner (shinra) and every game .so.
// Only #[repr(C)] POD types live here. No allocations cross the boundary;
// pointers handed back to the runner are valid until the next tick.

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct InputFrame {
    pub move_x: f32,      // a/d        → -1.0 / +1.0
    pub move_z: f32,      // w/s        → -1.0 / +1.0
    pub rot_yaw: f32,     // arrow ←/→  → -1.0 / +1.0
    pub rot_pitch: f32,   // arrow ↑/↓  → -1.0 / +1.0
    pub scale_delta: f32, // j/k        → +1.0 / -1.0
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct Drawable {
    pub mesh_id: u32,     // index into meshes the game declared at init
    pub _pad: u32,        // align model to 8 bytes
    pub model: [f32; 16], // column-major mat4
}

// FFI symbols every game cdylib must export. Documentation only — Rust does
// not let us declare extern blocks for symbols we will dlsym at runtime.
//
//   extern "C" fn meshes_count() -> u32;
//   extern "C" fn meshes_path(i: u32, out: *mut u8, cap: u32) -> u32;
//   extern "C" fn tick(dt: f32, input: *const InputFrame);
//   extern "C" fn drawables_ptr() -> *const Drawable;
//   extern "C" fn drawables_len() -> u32;
