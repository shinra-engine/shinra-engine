//! `contract.wasm` — a wasm module that is nothing but a custom section.
//!
//! design.md settles on `.wasm` for every artifact, and wasm already has a
//! container format, so inventing one would be rebuilding what the file
//! already has. A module with no functions is still a valid module: eight
//! bytes of header and one custom section. The host reads the section without
//! instantiating anything, and `wasm-objdump` can still open the file.

use anyhow::{bail, Context, Result};

fn leb(v: &mut Vec<u8>, mut n: u32) {
    loop {
        let mut b = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            b |= 0x80;
        }
        v.push(b);
        if n == 0 {
            return;
        }
    }
}

/// A wasm module carrying `payload` under `name`, and nothing else.
pub fn wrap(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(name.len() + payload.len() + 8);
    leb(&mut body, name.len() as u32);
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(payload);

    let mut out = Vec::with_capacity(body.len() + 16);
    out.extend_from_slice(b"\0asm");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.push(0x00); // custom section
    leb(&mut out, body.len() as u32);
    out.extend_from_slice(&body);
    out
}

fn read_leb(b: &[u8], i: &mut usize) -> Result<u32> {
    let (mut r, mut s) = (0u32, 0u32);
    loop {
        let x = *b.get(*i).context("truncated LEB128")?;
        *i += 1;
        r |= ((x & 0x7f) as u32) << s;
        if x & 0x80 == 0 {
            return Ok(r);
        }
        s += 7;
        if s > 31 {
            bail!("LEB128 too long")
        }
    }
}

/// One named custom section, without a wasm engine. Thirty lines, in exchange
/// for a format every wasm tool already understands.
pub fn section<'a>(module: &'a [u8], want: &str) -> Result<&'a [u8]> {
    if module.len() < 8 || &module[..4] != b"\0asm" {
        bail!("not a wasm module")
    }
    let mut i = 8usize;
    while i < module.len() {
        let id = module[i];
        i += 1;
        let size = read_leb(module, &mut i)? as usize;
        let end = i.checked_add(size).context("section runs past the file")?;
        if end > module.len() {
            bail!("section runs past the file")
        }
        if id == 0 {
            let mut j = i;
            let n = read_leb(module, &mut j)? as usize;
            let name = std::str::from_utf8(module.get(j..j + n).context("truncated name")?)?;
            if name == want {
                return Ok(&module[j + n..end]);
            }
        }
        i = end;
    }
    bail!("this module has no `{want}` section")
}

/// Every custom section name, for reporting what an artifact carries.
pub fn section_names(module: &[u8]) -> Result<Vec<(String, usize)>> {
    let mut out = Vec::new();
    let mut i = 8usize;
    if module.len() < 8 {
        bail!("not a wasm module")
    }
    while i < module.len() {
        let id = module[i];
        i += 1;
        let size = read_leb(module, &mut i)? as usize;
        let end = i + size;
        if id == 0 {
            let mut j = i;
            let n = read_leb(module, &mut j)? as usize;
            if let Some(s) = module.get(j..j + n).and_then(|b| std::str::from_utf8(b).ok()) {
                out.push((s.to_string(), end - (j + n)));
            }
        }
        i = end;
    }
    Ok(out)
}
