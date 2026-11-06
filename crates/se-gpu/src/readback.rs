//! Reading a replayed frame's pixels back to the CPU.
//!
//! Split out of `replay.rs` to keep that file under the project's per-file
//! line limit. The buffer this module waits on and unpacks is filled by the
//! `copy_texture_to_buffer` at the end of `replay::Gpu::replay_inner`.

use crate::{Gpu, GPU_BUDGET};
use anyhow::Result;
use std::time::{Duration, Instant};

impl Gpu {
    /// The rendered image, RGBA8. Offscreen for now — a window is the same
    /// replay against a surface texture.
    ///
    /// Adds its elapsed time to `last_cost` rather than replacing it, so
    /// after a `replay()` then `pixels()` call pair, `last_cost()` reflects
    /// both.
    pub fn pixels(&self) -> Result<Vec<u8>> {
        let start = Instant::now();
        let result = self.pixels_with_budget(GPU_BUDGET);
        self.last_cost.set(self.last_cost.get() + start.elapsed());
        result
    }

    /// Cost of the most recent `replay()` + `pixels()` call pair. Resets at
    /// the start of each `replay()` call, so this is never a running total
    /// across many frames.
    pub fn last_cost(&self) -> Duration {
        self.last_cost.get()
    }

    /// `pixels()`'s body, with the wall-clock budget as a parameter. A test
    /// can pass `Duration::ZERO` to force `wgpu::PollError::Timeout`
    /// deterministically — a submission just queued cannot be seen complete
    /// in a zero-length wait — without blocking for the real `GPU_BUDGET`.
    pub fn pixels_with_budget(&self, budget: Duration) -> Result<Vec<u8>> {
        let slice = self.target.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(budget),
            })
            .map_err(|e| {
                if matches!(e, wgpu::PollError::Timeout) {
                    anyhow::Error::new(e)
                        .context(format!("GPU replay exceeded its {budget:?} budget"))
                } else {
                    e.into()
                }
            })?;
        let padded = slice.get_mapped_range().to_vec();
        self.target.readback.unmap();
        // Drop the row padding the copy required.
        let w = (self.target.width * 4) as usize;
        let stride = self.target.row_bytes as usize;
        let mut out = Vec::with_capacity(w * self.target.height as usize);
        for y in 0..self.target.height as usize {
            out.extend_from_slice(&padded[y * stride..y * stride + w]);
        }
        Ok(out)
    }
}
