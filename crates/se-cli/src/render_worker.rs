//! Render on its own thread, built but not yet wired into `run.rs` (task-247).
//!
//! `Gpu` moves into the thread closure for good: nothing outside this file
//! ever touches it again. The main thread hands over a point-in-time
//! snapshot — `Meta`/`World` cloned, `Vm::bytes()` copied, commands already
//! decoded — never a live view into memory the next `tick()` will mutate.
//!
//! Nothing in the rest of the crate calls into this module yet, so a plain
//! build reports its public surface as unused; `#![allow(dead_code)]` defers
//! that to task-247, which wires it into `run.rs`/`live.rs`.
#![allow(dead_code)]

use se_gpu::command::Command;
use se_gpu::Gpu;
use se_vm::world::{Meta, World};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::JoinHandle;

pub enum RenderRequest {
    Snapshot { meta: Meta, world: World, mem: Vec<u8>, cmds: Vec<Command> },
    /// Not yet sent by anything — `run.rs` still resizes `Live::gpu` directly
    /// until task-247 moves it behind this worker.
    Resize(u32, u32),
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
    pub fn spawn(mut gpu: Gpu) -> RenderWorker {
        let (tx, work_rx) = std::sync::mpsc::sync_channel::<RenderRequest>(1);
        let (reply_tx, rx) = std::sync::mpsc::sync_channel::<RenderReply>(1);
        let handle = std::thread::spawn(move || {
            while let Ok(req) = work_rx.recv() {
                match req {
                    RenderRequest::Resize(w, h) => gpu.resize(w, h),
                    RenderRequest::Snapshot { meta, world, mem, cmds } => {
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
}

#[cfg(test)]
mod tests;
