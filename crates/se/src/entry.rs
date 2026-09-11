//! The six entry points, as macros.
//!
//! Every one of these expands to a `#[no_mangle] extern "C"` function the host
//! looks up by name. Writing them by hand is possible and never necessary.

/// Stamp the ABI version into the image. The host reads this first and refuses
/// the module outright if the major disagrees.
#[macro_export]
macro_rules! abi_version {
    () => {
        #[no_mangle]
        pub extern "C" fn se_abi_version() -> $crate::AbiVersion {
            $crate::AbiVersion::CURRENT
        }
    };
}

/// `bundle/data.rs` — the component half of the contract.
///
/// ```ignore
/// se::layouts!(Transform, Body);
/// ```
///
/// This names components declared elsewhere. `se::components!` declares and
/// names them in one act, which is the spelling that cannot fall out of step;
/// it expands to this macro.
#[macro_export]
macro_rules! layouts {
    ($($t:ty),* $(,)?) => {
        #[no_mangle]
        pub extern "C" fn se_register_layouts() -> *const $crate::Registered {
            static HELD: ::std::sync::OnceLock<$crate::Held> = ::std::sync::OnceLock::new();
            HELD.get_or_init(|| {
                $crate::hold(::std::vec![$(<$t as $crate::Schema>::layout()),*])
            })
            .as_ptr()
        }
    };
}

/// `bundle/buffer.rs` — the render-target half of the contract.
///
/// ```ignore
/// se::buffers!(
///     se::BufferDesc::screen("scene", se::Format::Rgba8Unorm),
///     se::BufferDesc::screen("depth", se::Format::Depth32Float),
/// );
/// ```
#[macro_export]
macro_rules! buffers {
    ($($b:expr),* $(,)?) => {
        #[no_mangle]
        pub extern "C" fn se_register_buffers() -> *const $crate::Registered {
            static HELD: ::std::sync::OnceLock<$crate::Held> = ::std::sync::OnceLock::new();
            HELD.get_or_init(|| $crate::hold(::std::vec![$($b),*])).as_ptr()
        }
    };
}

/// `asset/*.so` — name to bytes, and nothing else. Content never writes data.
///
/// ```ignore
/// se::assets! {
///     "bunny.obj" => include_bytes!("bunny/model.obj"),
/// }
/// ```
#[macro_export]
macro_rules! assets {
    ($($name:literal => $bytes:expr),* $(,)?) => {
        $crate::abi_version!();

        #[no_mangle]
        pub extern "C" fn se_register_assets() -> *const $crate::Registered {
            static HELD: ::std::sync::OnceLock<$crate::Held> = ::std::sync::OnceLock::new();
            HELD.get_or_init(|| {
                $crate::hold(::std::vec![$($crate::AssetDesc::new($name, $bytes)),*])
            })
            .as_ptr()
        }
    };
}

/// `process/*.so` — the stage list, collected rather than written.
///
/// `#[se::stage]` puts one `StageEntry` into the `se_stages` link section for
/// every function it marks, and this walks that section. That is the whole of
/// "declaring a stage registers it": there is no list to keep in step, and a
/// stage cannot be left out by being forgotten.
///
/// The two halves have to agree on the section name and on the element type,
/// so the type lives here, beside the walk, rather than inside the expansion.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct StageEntry(*const crate::StageSpec);

// SAFETY: an entry only ever points at a `'static` spec in its own image, and
// nothing writes one after link time.
unsafe impl Sync for StageEntry {}

impl StageEntry {
    /// Called by `#[se::stage]`. There is no other way to fill the section.
    pub const fn new(spec: &'static crate::StageSpec) -> StageEntry {
        StageEntry(spec)
    }
}

#[cfg(not(target_arch = "wasm32"))]
extern "C" {
    /// Linker-defined, because `se_stages` is spelled as a C identifier. They
    /// bound what `#[se::stage]` contributed, in whatever order the link chose.
    static __start_se_stages: StageEntry;
    static __stop_se_stages: StageEntry;
}

/// Keeps the section — and so `__start`/`__stop` — present in every image this
/// function reaches, including one that links it and declares no stage at all.
/// Skipped by the walk.
#[cfg(not(target_arch = "wasm32"))]
#[used]
#[link_section = "se_stages"]
static ANCHOR: StageEntry = StageEntry(core::ptr::null());

/// Walks the link section a `.so` collects stages in.
///
/// wasm has no such section — a custom section there must be a plain run of
/// bytes, and a pointer is a relocation — so on that backend `#[se::stage]`
/// exports each stage under an `se_stage__` name and the loader enumerates
/// them. This entry point still exists there and answers empty, so the symbol
/// table looks the same from outside.
#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn se_register_stages() -> *const crate::Registered {
    static HELD: std::sync::OnceLock<crate::Held> = std::sync::OnceLock::new();
    HELD.get_or_init(|| {
        // A real load, so the anchor's object comes out of the rlib alongside
        // this one: `#[used]` keeps a static in its object, only a reference
        // keeps the object in the link.
        let _ = unsafe { core::ptr::read_volatile(&ANCHOR) };

        let mut out: Vec<crate::StageSpec> = Vec::new();
        // SAFETY: the linker bounds the section, and every non-null entry
        // points at a `'static` spec in this image.
        unsafe {
            let mut p = core::ptr::addr_of!(__start_se_stages);
            let stop = core::ptr::addr_of!(__stop_se_stages);
            while p < stop {
                if let Some(spec) = (*p).0.as_ref() {
                    out.push(*spec);
                }
                p = p.add(1);
            }
        }
        crate::hold(out)
    })
    .as_ptr()
}

/// `render/*.so` — nodes and edges.
///
/// ```ignore
/// se::graph!(|g| g
///     .present("scene")
///     .pass("bodies", |p| p
///         .shader(format!("{}{}", wgsl::MESH_VS, shade::FS))
///         .color(&["scene"])
///         .depth("depth")
///         .uniform_of("Camera")
///         .instanced("Transform", "model.obj")));
/// ```
#[macro_export]
macro_rules! graph {
    ($name:literal, $build:expr) => {
        $crate::abi_version!();

        #[no_mangle]
        pub extern "C" fn se_register_graph() -> *const $crate::Registered {
            static HELD: ::std::sync::OnceLock<$crate::Held> = ::std::sync::OnceLock::new();
            HELD.get_or_init(|| {
                let build: fn($crate::GraphBuilder) -> $crate::GraphBuilder = $build;
                build($crate::GraphBuilder::new($name)).into_held()
            })
            .as_ptr()
        }
    };
}

/// `game/*.so` — control. Slots say which module fills which position; `tick`
/// is the only place in the whole engine that knows what time it is.
///
/// ```ignore
/// se::control! {
///     name: "game1",
///     slots: [se::SlotBind::new(se::Slot::Render, "graph1")],
///     start: start,
///     tick: tick,
/// }
/// ```
#[macro_export]
macro_rules! control {
    (
        name: $name:literal,
        slots: [$($s:expr),* $(,)?],
        $(start: $start:path,)?
        tick: $tick:path
        $(, stop: $stop:path)?
        $(,)?
    ) => {
        $crate::abi_version!();

        static __SE_SLOTS: &[$crate::SlotBind] = &[$($s),*];

        #[no_mangle]
        pub extern "C" fn se_register_control() -> *const $crate::Registered {
            unsafe extern "C" fn __se_tick(ctl: *mut $crate::Ctl, f: *const $crate::Frame) {
                $tick(&mut *ctl, &*f)
            }
            $(
                unsafe extern "C" fn __se_start(ctl: *mut $crate::Ctl) {
                    $start(&mut *ctl)
                }
            )?
            $(
                unsafe extern "C" fn __se_stop(ctl: *mut $crate::Ctl) {
                    $stop(&mut *ctl)
                }
            )?

            #[allow(unused_mut)]
            let mut start: Option<unsafe extern "C" fn(*mut $crate::Ctl)> = None;
            $( { let _ = stringify!($start); start = Some(__se_start); } )?
            #[allow(unused_mut)]
            let mut stop: Option<unsafe extern "C" fn(*mut $crate::Ctl)> = None;
            $( { let _ = stringify!($stop); stop = Some(__se_stop); } )?

            static HELD: ::std::sync::OnceLock<$crate::Held> = ::std::sync::OnceLock::new();
            HELD.get_or_init(|| {
                $crate::hold(::std::vec![$crate::ControlSpec {
                    name: $crate::Str::new($name),
                    slots: $crate::Slice::new(__SE_SLOTS),
                    start,
                    tick: __se_tick,
                    stop,
                }])
            })
            .as_ptr()
        }
    };
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn se_register_stages() -> *const crate::Registered {
    static EMPTY: crate::Held = crate::Held(crate::Registered::EMPTY);
    EMPTY.as_ptr()
}

/// Scratch memory a wasm host can hand a stage its data in.
///
/// The `.so` backend gives a stage raw pointers into the host's own columns. A
/// wasm module cannot hold those — it reaches nothing but its own linear
/// memory — so the host copies the columns in, runs the stage, and copies the
/// written ones back. Measured at ~6 us a frame for a thousand entities, which
/// is 0.03% of a 60Hz budget; sharing one memory across a bundle's modules
/// would avoid it, but only by making every module import that memory and stop
/// being self-contained.
///
/// # Safety
/// The host must pass the same `n` to `se_free` that it passed here.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn se_alloc(n: u32) -> *mut u8 {
    // 16 covers every alignment in the ABI, including the `[f32; 4]` in a
    // clear colour and any `u64`.
    match core::alloc::Layout::from_size_align(n as usize, 16) {
        Ok(l) if n > 0 => unsafe { std::alloc::alloc(l) },
        _ => core::ptr::null_mut(),
    }
}

/// # Safety
/// `p` and `n` must be what `se_alloc` returned and was asked for.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn se_free(p: *mut u8, n: u32) {
    if let Ok(l) = core::alloc::Layout::from_size_align(n as usize, 16) {
        std::alloc::dealloc(p, l);
    }
}
