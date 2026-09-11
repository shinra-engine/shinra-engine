//! Reading wasm32 structs out of a module's linear memory.
//!
//! The host cannot reinterpret these bytes as the Rust types it already has.
//! On wasm32 a pointer is four bytes and `usize` is four bytes, so every
//! struct in the ABI that holds a `Str` or a `Slice` has a different size and
//! different field offsets than the same struct compiled for the host. They
//! have to be parsed, field by field, at the offsets wasm32 gives them.
//!
//! That is the whole cost of the wasm backend, and it is paid once here rather
//! than anywhere a module can see. Every offset below is checked against the
//! `.so` path in `tests/parity.rs`: the same source compiled both ways must
//! produce the same `LayoutDef`s, which is a far better check than arithmetic
//! done twice by the same person.

use anyhow::{bail, Context, Result};

/// A module's linear memory, with bounds checks that produce errors rather
/// than panics — the bytes come from a module, and a module can be wrong.
pub struct Mem<'a> {
    pub bytes: &'a [u8],
}

impl<'a> Mem<'a> {
    pub fn at(&self, off: u32, len: usize) -> Result<&'a [u8]> {
        let a = off as usize;
        self.bytes
            .get(a..a + len)
            .with_context(|| format!("read of {len} bytes at {off:#x} runs past linear memory"))
    }

    pub fn u32(&self, off: u32) -> Result<u32> {
        Ok(u32::from_le_bytes(self.at(off, 4)?.try_into().unwrap()))
    }

    pub fn u64(&self, off: u32) -> Result<u64> {
        Ok(u64::from_le_bytes(self.at(off, 8)?.try_into().unwrap()))
    }

    pub fn bool(&self, off: u32) -> Result<bool> {
        Ok(self.at(off, 1)?[0] != 0)
    }

    /// `Str { ptr: u32, len: u32 }` — 8 bytes on wasm32.
    pub fn str(&self, off: u32) -> Result<String> {
        let (p, n) = (self.u32(off)?, self.u32(off + 4)? as usize);
        if n == 0 {
            return Ok(String::new());
        }
        Ok(std::str::from_utf8(self.at(p, n)?)
            .context("a module registered a name that is not UTF-8")?
            .to_string())
    }

    /// `Slice<T> { ptr: u32, len: u32 }` — the same 8 bytes. Returns where the
    /// elements start and how many there are; the caller knows their stride.
    pub fn slice(&self, off: u32) -> Result<(u32, u32)> {
        Ok((self.u32(off)?, self.u32(off + 4)?))
    }

    /// A `#[repr(u32)]` enum, refused rather than transmuted when out of range.
    pub fn enum_u32<T: TryFrom<u32>>(&self, off: u32, what: &str) -> Result<T> {
        let v = self.u32(off)?;
        T::try_from(v).map_err(|_| anyhow::anyhow!("{what}: {v} is not a valid discriminant"))
    }
}

/// Where a module said its registrations are.
///
/// `Registered { ptr: u32, len: u32, stride: u32 }` — 12 bytes on wasm32.
pub struct Registered {
    pub ptr: u32,
    pub len: u32,
    pub stride: u32,
}

impl Registered {
    pub fn read(m: &Mem, at: u32) -> Result<Registered> {
        Ok(Registered { ptr: m.u32(at)?, len: m.u32(at + 4)?, stride: m.u32(at + 8)? })
    }

    /// Byte offset of element `i`, with the stride the module reported checked
    /// against what this host expects to parse.
    pub fn elem(&self, i: u32, want_stride: u32, what: &str) -> Result<u32> {
        if self.stride != want_stride {
            bail!(
                "{what}: module registered {}-byte items, this host parses {want_stride}-byte ones \
                 — rebuild it against this engine",
                self.stride
            );
        }
        Ok(self.ptr + i * self.stride)
    }
}
