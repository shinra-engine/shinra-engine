//! Render on its own thread, wired into `run.rs`/`live.rs`/`swap.rs` by
//! task-247.
//!
//! `Gpu` moves into the thread closure for good: nothing outside this file
//! ever touches it again. The main thread hands over a point-in-time
//! snapshot — `Meta`/`World` cloned, `Vm::bytes()` copied, commands already
//! decoded — never a live view into memory the next `tick()` will mutate.
//! A render-module swap or an asset hot-reload reaches the same `Gpu` the
//! same way, through `Replace`/`SetResource`, since nothing outside this
//! thread may hold the `Gpu` itself.

use se_gpu::command::Command;
use se_gpu::resources::Resource;
use se_gpu::Gpu;
use se_vm::world::{Meta, World};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::JoinHandle;

pub enum RenderRequest {
    Snapshot { meta: Meta, world: World, mem: Vec<u8>, cmds: Vec<Command> },
    Resize(u32, u32),
    /// A render-module swap builds the replacement `Gpu` on the main thread
    /// (it needs no device the worker doesn't also have) and hands it over
    /// whole; the worker never gives one back.
    Replace(Gpu),
    /// An asset hot-reload's decoded (CPU-side) mesh/texture, for the worker
    /// to upload — `Gpu::set_resource` is where bytes actually become device
    /// resources, and that only ever happens on the thread that owns `Gpu`.
    SetResource(u32, Resource),
}

pub enum RenderReply {
    Picture(Vec<u8>),
    TimedOut,
    Failed(String),
}

/// Owns the render thread for the rest of the program's life. Dropping it
/// drops `tx`, which ends the thread's `recv()` loop; nothing else signals
/// shutdown.
pub struct RenderWorker {
    tx: SyncSender<RenderRequest>,
    rx: Receiver<RenderReply>,
    /// Never joined — dropping `tx` ends the thread on its own. Kept so a
    /// future graceful-shutdown path has it to join.
    #[allow(dead_code)]
    handle: JoinHandle<()>,
}

fn reply_for(gpu: &mut Gpu, meta: &Meta, world: &World, mem: &[u8], cmds: &[Command]) -> RenderReply {
    let rendered = gpu.replay(meta, world, mem, cmds).and_then(|_| gpu.pixels());
    match rendered {
        Ok(p) => RenderReply::Picture(p),
        Err(e) if matches!(e.downcast_ref::<wgpu::PollError>(), Some(wgpu::PollError::Timeout)) => {
            RenderReply::TimedOut
        }
        Err(e) => RenderReply::Failed(format!("{e:#}")),
    }
}

impl RenderWorker {
    /// Moves `gpu` onto a new thread and starts its request/reply loop.
    pub fn spawn(gpu: Gpu) -> RenderWorker {
        Self::spawn_loop(gpu, std::time::Duration::ZERO)
    }

    /// Test-only seam: behaves exactly like `spawn`, but sleeps `delay` before
    /// replying to each snapshot — standing in for a slow (but within
    /// story-241's `GPU_BUDGET`) render without relying on real GPU/wgpu
    /// timing, which is too fast and machine-dependent to drive a
    /// deterministic test.
    #[cfg(test)]
    pub fn spawn_with_delay(gpu: Gpu, delay: std::time::Duration) -> RenderWorker {
        Self::spawn_loop(gpu, delay)
    }

    fn spawn_loop(mut gpu: Gpu, delay: std::time::Duration) -> RenderWorker {
        let (tx, work_rx) = std::sync::mpsc::sync_channel::<RenderRequest>(1);
        let (reply_tx, rx) = std::sync::mpsc::sync_channel::<RenderReply>(1);
        let handle = std::thread::spawn(move || {
            while let Ok(req) = work_rx.recv() {
                match req {
                    RenderRequest::Resize(w, h) => gpu.resize(w, h),
                    RenderRequest::Replace(g) => gpu = g,
                    RenderRequest::SetResource(id, r) => gpu.set_resource(id, r),
                    RenderRequest::Snapshot { meta, world, mem, cmds } => {
                        if !delay.is_zero() {
                            std::thread::sleep(delay);
                        }
                        let reply = reply_for(&mut gpu, &meta, &world, &mem, &cmds);
                        if reply_tx.send(reply).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        RenderWorker { tx, rx, handle }
    }

    /// `true` if the request was accepted, `false` if the worker is still
    /// busy with a previous one (the bound-1 channel was full) — the caller's
    /// cue to skip this iteration, the same as a stale picture staying on
    /// screen. Builds the request itself, so a caller never constructs
    /// `RenderRequest` directly.
    pub fn try_send_snapshot(&self, meta: Meta, world: World, mem: Vec<u8>, cmds: Vec<Command>) -> bool {
        match self.tx.try_send(RenderRequest::Snapshot { meta, world, mem, cmds }) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Non-blocking poll for the worker's last finished reply.
    pub fn try_recv_reply(&self) -> Option<RenderReply> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }

    /// A viewport resize. Rare and user/terminal-driven, so this blocks
    /// rather than risking a dropped resize the way a full snapshot can be
    /// dropped every frame.
    pub fn resize(&self, w: u32, h: u32) {
        let _ = self.tx.send(RenderRequest::Resize(w, h));
    }

    /// Hand the worker a freshly built `Gpu` — a render-module swap's result.
    pub fn replace(&self, gpu: Gpu) {
        let _ = self.tx.send(RenderRequest::Replace(gpu));
    }

    /// Hand the worker a decoded asset to upload — an asset hot-reload's
    /// result.
    pub fn set_resource(&self, id: u32, resource: Resource) {
        let _ = self.tx.send(RenderRequest::SetResource(id, resource));
    }
}

#[cfg(test)]
mod tests;
