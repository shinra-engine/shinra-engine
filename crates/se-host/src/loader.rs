//! Opening a module, and letting go of one.
//!
//! # Two backends, one source
//!
//! A bundle is built either native or wasm, and never mixed — a game repo
//! chooses at build time and the same module source serves both. That choice
//! is what makes the hard part tractable: within one bundle every module
//! shares an address space, so the host can hand out pointers into component
//! columns and the per-entity loop stays inside the module.
//!
//! Native gets that from `dlopen`. Wasm gets it from every module in the
//! bundle importing one linear memory the host owns, which makes a pointer an
//! offset every module agrees on. Neither can reach into the other, and
//! nothing needs them to.
//!
//! What differs is only *how an entry point is reached*: native looks a symbol
//! up and calls it with a sink; a wasm guest cannot call a host function
//! pointer handed to it as data, so it calls an import by name instead. That
//! difference lives behind `register` below and inside `se`'s entry macros —
//! never in a module's source.
//!
//! Two things make hot swap work here. The host copies every string and byte
//! out of the image at registration time, so nothing it keeps points into a
//! library it might drop. And each load goes through a fresh file on disk,
//! because `dlopen` hands back the *same* handle for a path it already has
//! open — reload the same path and you get the old code back, silently.

use anyhow::{anyhow, bail, Context, Result};
use libloading::{Library, Symbol};
use se_abi::{AbiVersion, Registered};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// A loaded `.so`, alive for exactly as long as anything the host copied out
/// of it still holds a function pointer into it.
pub struct Module {
    /// Module name: the `.rs` file stem it was built from.
    pub name: String,
    /// Where the build put it — what gets watched for changes.
    pub origin: PathBuf,
    /// The private copy actually handed to `dlopen`.
    shadow: PathBuf,
    pub mtime: SystemTime,
    /// Dropped last. Declared last so it is destroyed after `shadow`'s path
    /// is no longer needed.
    lib: Library,
}

impl Module {
    pub fn open(origin: &Path) -> Result<Module> {
        let name = origin
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("{} has no module name", origin.display()))?
            .to_string();

        let mtime = std::fs::metadata(origin)
            .with_context(|| format!("stat {}", origin.display()))?
            .modified()?;

        let gen = GENERATION.fetch_add(1, Ordering::Relaxed);
        let hot = origin
            .parent()
            .ok_or_else(|| anyhow!("{} has no parent", origin.display()))?
            .join(".hot");
        std::fs::create_dir_all(&hot)?;
        let shadow = hot.join(format!("{name}-{gen}.so"));
        std::fs::copy(origin, &shadow)
            .with_context(|| format!("shadow {} -> {}", origin.display(), shadow.display()))?;

        // SAFETY: running a module's initialisers is the point. A module that
        // misbehaves here can take the process down; that is the same trust
        // boundary as any plugin system.
        let lib = unsafe { Library::new(&shadow) }
            .with_context(|| format!("dlopen {}", shadow.display()))?;

        let m = Module { name, origin: origin.to_path_buf(), shadow, mtime, lib };
        m.check_abi()?;
        Ok(m)
    }

    fn check_abi(&self) -> Result<()> {
        let f: Symbol<unsafe extern "C" fn() -> AbiVersion> = unsafe {
            self.lib
                .get(se_abi::sym::ABI_VERSION)
                .with_context(|| format!("`{}` exports no se_abi_version", self.name))?
        };
        let theirs = unsafe { f() };
        if !AbiVersion::CURRENT.accepts(theirs) {
            bail!(
                "`{}` was built against ABI {}.{}, host speaks {}.{}",
                self.name,
                theirs.major,
                theirs.minor,
                AbiVersion::CURRENT.major,
                AbiVersion::CURRENT.minor
            );
        }
        Ok(())
    }

    /// Run one registration entry point and read back what it registered.
    ///
    /// Every `se_register_*` has the same shape — nothing in, one pointer out
    /// — which is what lets a wasm backend implement the same call. There the
    /// returned integer is an offset into the module's exported linear memory
    /// rather than an address, and only this function has to know the
    /// difference.
    ///
    /// # Safety
    /// The module must stay loaded for as long as the result is read; the
    /// caller must cast to the element type `sym` registers, and `stride` is
    /// checked against it in [`Module::items`].
    pub unsafe fn registered_ptr(&self, sym: &[u8]) -> Result<*const Registered> {
        let f: Symbol<unsafe extern "C" fn() -> *const Registered> =
            self.lib.get(sym).with_context(|| {
                format!("`{}` exports no {}", self.name, String::from_utf8_lossy(sym))
            })?;
        Ok(f())
    }

    /// The registered items, as a slice.
    ///
    /// # Safety
    /// `T` must be the type `sym` registers, and `self` must stay loaded.
    pub unsafe fn items<T>(&self, sym: &[u8]) -> Result<&[T]> {
        let r = self.registered_ptr(sym)?;
        crate::read::items(r, &String::from_utf8_lossy(sym), &self.name)
    }

    /// Whether `sym` is present at all — some entry points are optional.
    pub fn has(&self, sym: &[u8]) -> bool {
        unsafe { self.lib.get::<*const ()>(sym).is_ok() }
    }

    /// True when the file on disk has moved on from what is loaded.
    pub fn stale(&self) -> bool {
        std::fs::metadata(&self.origin)
            .and_then(|m| m.modified())
            .map(|t| t != self.mtime)
            .unwrap_or(false)
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // Unlinking a mapped image is fine on Linux: the inode survives until
        // `lib` is dropped a moment later and the last mapping goes away.
        let _ = std::fs::remove_file(&self.shadow);
    }
}

/// `bundle.so` is the one module with two entry points and no `se_abi_version`
/// of its own — the generated root supplies it. Everything else is uniform.
pub fn is_bundle(name: &str) -> bool {
    name == "bundle"
}
