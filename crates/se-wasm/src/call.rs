//! Handing a wasm stage its data.
//!
//! The `.so` backend passes a stage raw pointers into the host's columns and
//! the stage writes through them. A wasm module reaches nothing but its own
//! linear memory, so the data has to go in and come back:
//!
//!   1. ask the module for scratch, through the `se_alloc` `se` exports;
//!   2. copy each column's rows in, and build a `StageCall` whose pointers are
//!      offsets into that memory rather than host addresses;
//!   3. call `se_stage_run__<name>`;
//!   4. copy back the columns the stage's signature says it writes.
//!
//! Step 4 is why `Access` is on a `ParamDef` at all. A stage that takes
//! `&Transform` cannot have changed it, so nothing is copied back for it —
//! the signature is the query on the `.so` side and it is the sync list here.
//!
//! Measured at roughly 6 us a frame for a thousand entities, against a 16,667
//! us budget at 60Hz. The alternative — one linear memory shared across a
//! bundle — costs every module its self-containment, and buys nothing until a
//! game is an order of magnitude larger than the ones this engine is for.

use crate::mem::Mem;
use anyhow::{bail, Context, Result};
use se_abi::{Access, ParamKind};
use se_host::registry::ParamDef;

/// wasm32 sizes for the two structs the host writes into module memory.
const COLUMN: u32 = 8; // { base: u32, stride: u32 }
const STAGE_CALL: u32 = 28; // cols,n_cols,rows,n_rows,res,n_res,dt

/// One column's bytes, as the host holds them.
pub struct ColumnData<'a> {
    pub bytes: &'a [u8],
    pub stride: u32,
}

/// What the host wants run, and with what.
pub struct Call<'a> {
    pub params: &'a [ParamDef],
    /// One per component parameter, in parameter order.
    pub columns: Vec<ColumnData<'a>>,
    /// `n_rows * n_cols` dense indices, row-major.
    pub rows: &'a [u32],
    pub n_rows: u32,
    /// The `Input` resource, already encoded for wasm32 by the caller.
    pub input: &'a [u8],
    pub dt: f32,
}

/// Where each piece landed inside the scratch block.
pub struct Plan {
    pub total: u32,
    pub call_at: u32,
    pub cols_at: u32,
    pub rows_at: u32,
    pub res_at: u32,
    pub input_at: u32,
    /// Offset and length of each column's copied bytes.
    pub column_at: Vec<(u32, u32)>,
}

fn align_to(n: u32, a: u32) -> u32 {
    n.div_ceil(a) * a
}

impl Call<'_> {
    /// Lay the scratch block out. One allocation: a stage runs once a frame
    /// and a call per piece would cost more than the copying does.
    pub fn plan(&self) -> Result<Plan> {
        let n_cols = self.columns.len() as u32;
        let mut at = 0u32;
        let call_at = at;
        at = align_to(at + STAGE_CALL, 8);
        let cols_at = at;
        at = align_to(at + n_cols * COLUMN, 8);
        let rows_at = at;
        at = align_to(at + self.rows.len() as u32 * 4, 8);
        let res_at = at;
        // One pointer per resource parameter; only `Input` exists today.
        let n_res = self.params.iter().filter(|p| p.kind == ParamKind::Resource).count() as u32;
        at = align_to(at + n_res * 4, 16);
        let input_at = at;
        at = align_to(at + self.input.len() as u32, 16);

        let mut column_at = Vec::with_capacity(self.columns.len());
        for c in &self.columns {
            column_at.push((at, c.bytes.len() as u32));
            at = align_to(at + c.bytes.len() as u32, 16);
        }
        if at == 0 {
            bail!("a stage call needs at least a frame");
        }
        Ok(Plan { total: at, call_at, cols_at, rows_at, res_at, input_at, column_at })
    }

    /// Write the frame and every table it points at into `dst`, which the
    /// caller has placed at `base` in the module's memory.
    pub fn encode(&self, p: &Plan, base: u32, dst: &mut [u8]) -> Result<()> {
        let put32 = |d: &mut [u8], at: u32, v: u32| {
            d[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
        };

        for (i, c) in self.columns.iter().enumerate() {
            let (off, len) = p.column_at[i];
            dst[off as usize..(off + len) as usize].copy_from_slice(c.bytes);
            let e = p.cols_at + i as u32 * COLUMN;
            put32(dst, e, base + off);
            put32(dst, e + 4, c.stride);
        }

        for (i, r) in self.rows.iter().enumerate() {
            put32(dst, p.rows_at + i as u32 * 4, *r);
        }

        let n_res = self.params.iter().filter(|p| p.kind == ParamKind::Resource).count() as u32;
        if n_res > 0 {
            dst[p.input_at as usize..p.input_at as usize + self.input.len()]
                .copy_from_slice(self.input);
            for i in 0..n_res {
                put32(dst, p.res_at + i * 4, base + p.input_at);
            }
        }

        let c = p.call_at;
        put32(dst, c, base + p.cols_at);
        put32(dst, c + 4, self.columns.len() as u32);
        put32(dst, c + 8, base + p.rows_at);
        put32(dst, c + 12, self.n_rows);
        put32(dst, c + 16, if n_res > 0 { base + p.res_at } else { 0 });
        put32(dst, c + 20, n_res);
        put32(dst, c + 24, self.dt.to_bits());
        Ok(())
    }

    /// Which component parameters the signature says are written, and so which
    /// columns have to come back.
    pub fn written(&self) -> Vec<usize> {
        self.params
            .iter()
            .filter(|p| p.kind == ParamKind::Component)
            .enumerate()
            .filter(|(_, p)| p.access == Access::Write)
            .map(|(i, _)| i)
            .collect()
    }
}

/// Read one column's bytes back out after the stage ran.
pub fn read_back<'a>(m: &Mem<'a>, p: &Plan, i: usize, base: u32) -> Result<&'a [u8]> {
    let (off, len) = *p.column_at.get(i).context("no such column")?;
    m.at(base + off, len as usize)
}
