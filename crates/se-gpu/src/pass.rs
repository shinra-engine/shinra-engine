//! Splitting a frame's command list into its `BEGIN_PASS..END_PASS` runs.

use crate::command::{Command, BEGIN_PASS, DRAW_FULLSCREEN, DRAW_INSTANCED, END_PASS, FLAG_DEPTH};
use anyhow::{bail, Result};
use std::ops::Range;

/// One `BEGIN_PASS..END_PASS` run: the target and clear it names, and the
/// slice of the frame's draws (indices into the frame-wide, in-order draw
/// list) it owns. A pass's draws are always contiguous in command order, so a
/// half-open range into that one list is enough to say which are its own.
pub struct PassGroup {
    pub target: u32,
    pub clear: [f32; 4],
    pub draws: Range<usize>,
    /// `BEGIN_PASS.render_flags & FLAG_DEPTH != 0` — this pass gets a depth
    /// attachment.
    pub depth: bool,
}

/// Splits a frame's commands into one group per pass plus the frame-wide, in
/// command order, draw list every group's range indexes into. A draw outside
/// any open pass, or a `BEGIN_PASS` before the previous one's `END_PASS`, is
/// refused rather than silently merged into the wrong pass.
pub fn group_passes(cmds: &[Command]) -> Result<(Vec<PassGroup>, Vec<&Command>)> {
    let mut groups: Vec<PassGroup> = Vec::new();
    let mut draws: Vec<&Command> = Vec::new();
    let mut open = false;
    for c in cmds {
        match c.kind {
            BEGIN_PASS => {
                if open {
                    bail!("a pass began before the previous one's END_PASS")
                }
                open = true;
                groups.push(PassGroup {
                    target: c.target,
                    clear: c.clear,
                    draws: draws.len()..draws.len(),
                    depth: c.render_flags & FLAG_DEPTH != 0,
                });
            }
            DRAW_INSTANCED | DRAW_FULLSCREEN => {
                if !open {
                    bail!("a draw was recorded outside any open pass")
                }
                draws.push(c);
                groups.last_mut().unwrap().draws.end = draws.len();
            }
            END_PASS => open = false,
            k => {
                bail!("render.wasm recorded command kind {k}, which this host has no rule for")
            }
        }
    }
    Ok((groups, draws))
}

/// Each group's depth attachment (if any), and which pipeline variant
/// (depth on/off) each draw needs, by draw index. Call right after the
/// present/offscreen `views` are resolved, so a depth-flagged group's target
/// size is already known to `Gpu::target_view`/`offscreen`.
pub(crate) fn resolve_depth(
    gpu: &mut crate::Gpu,
    groups: &[PassGroup],
    draw_count: usize,
) -> (Vec<Option<wgpu::TextureView>>, Vec<bool>) {
    let depths = groups
        .iter()
        .map(|g| g.depth.then(|| gpu.depth_view(g.target).clone()))
        .collect();
    let mut draw_depth = vec![false; draw_count];
    for g in groups {
        for k in g.draws.clone() {
            draw_depth[k] = g.depth;
        }
    }
    (depths, draw_depth)
}
