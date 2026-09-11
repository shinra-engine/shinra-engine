//! The one that matters: a stage that actually changes data, run on both
//! backends, must produce the same bytes.
//!
//! Everything before this compared *descriptions*. This compares the result of
//! running, which is the only thing a game cares about — and it is where the
//! wasm backend's copy-in/copy-out marshalling either works or quietly
//! produces plausible garbage.

mod common;
use common::{build_both, ensure_se_for_wasm, have_wasm_target};

use se_abi::{Column, Input, StageCall};
use se_wasm::call::{Call, ColumnData};

/// `drift` moves each position by its velocity; `Body` is read-only, so only
/// `Transform` may come back changed.
const SOURCE: &str = r#"
se::abi_version!();

se::components! {
    Transform { pos: [f32; 3] }
    Body { vel: [f32; 3] }
}

#[se::stage]
fn drift(t: &mut Transform, b: &Body, dt: f32) {
    t.pos[0] += b.vel[0] * dt;
    t.pos[1] += b.vel[1] * dt;
    t.pos[2] += b.vel[2] * dt;
}
"#;

const N: usize = 6;
const STRIDE: u32 = 12; // [f32; 3]

fn seed() -> (Vec<u8>, Vec<u8>) {
    let mut t = Vec::new();
    let mut b = Vec::new();
    for i in 0..N {
        for k in 0..3 {
            t.extend_from_slice(&((i * 3 + k) as f32).to_le_bytes());
            b.extend_from_slice(&(((i + 1) as f32) * 0.25 + k as f32).to_le_bytes());
        }
    }
    (t, b)
}

fn floats(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}

#[test]
fn a_stage_that_writes_produces_the_same_bytes_on_both_backends() {
    if !have_wasm_target() {
        eprintln!("skipping: wasm32-unknown-unknown is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("se_wasm_run_stage");
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("process_root.rs");
    std::fs::write(&root, SOURCE).unwrap();

    ensure_se_for_wasm();
    let built = build_both(&root, "run_stage", &dir);

    let rows: Vec<u32> = (0..N as u32).flat_map(|i| [i, i]).collect();
    let dt = 0.5f32;

    // --- the .so side: the stage writes through host pointers -------------
    let (mut t_so, b_so) = seed();
    {
        let m = se_host::Module::open(&built.so).unwrap();
        let stages = unsafe { se_host::sink::stages(&m) }.unwrap();
        let s = stages.iter().find(|s| s.name == "drift").expect("drift not registered");

        let mut b = b_so.clone();
        let cols = [
            Column { base: t_so.as_mut_ptr(), stride: STRIDE },
            Column { base: b.as_mut_ptr(), stride: STRIDE },
        ];
        let call = StageCall {
            cols: cols.as_ptr(),
            n_cols: 2,
            rows: rows.as_ptr(),
            n_rows: N as u32,
            res: core::ptr::null(),
            n_res: 0,
            dt,
        };
        unsafe { (s.run)(&call) };
    }

    // --- the wasm side: the stage writes into its own memory --------------
    let (t_wasm, b_wasm) = seed();
    let engine = se_wasm::engine().unwrap();
    let mut w = se_wasm::WasmModule::open(&engine, &built.wasm).unwrap();
    let info = w.stages().unwrap();
    let drift = info.iter().find(|s| s.name == "drift").expect("drift not exported");

    let c = Call {
        params: &drift.params,
        columns: vec![
            ColumnData { bytes: &t_wasm, stride: STRIDE },
            ColumnData { bytes: &b_wasm, stride: STRIDE },
        ],
        rows: &rows,
        n_rows: N as u32,
        input: &[],
        dt,
    };
    let back = w.run_stage("drift", &c).unwrap();

    // Only `Transform` is `&mut`, so only it comes back.
    assert_eq!(back.len(), 1, "a read-only column must not be copied back");
    assert_eq!(back[0].0, 0, "the written column is the first parameter");

    assert_eq!(floats(&back[0].1), floats(&t_so));

    // And it must actually have moved, or the two agreeing means nothing.
    let (before, _) = seed();
    assert_ne!(floats(&t_so), floats(&before), "the stage did not run at all");

    let _ = Input::NAME;
}
