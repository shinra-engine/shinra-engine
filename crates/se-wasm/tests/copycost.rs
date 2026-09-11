//! How much does it cost to hand a wasm stage its data by copying?
//!
//! The `.so` backend gives a stage raw pointers into the host's columns. A
//! wasm module cannot hold those — it can only reach its own linear memory —
//! so either every module in a bundle shares one memory (which means imports,
//! and a module that is no longer self-contained) or the host copies the
//! columns in and out around each stage.
//!
//! This is not a test of correctness; it exists so the choice is made against
//! a number rather than an intuition. Run with `--nocapture`.

use std::time::Instant;
use wasmtime::{Engine, Instance, Linker, Module, Store};

/// A module that owns a megabyte and does nothing, so the timing is the copy
/// and not the work.
const WAT: &str = r#"
(module
  (memory (export "memory") 32)
  (func (export "noop"))
)
"#;

fn bytes_per_sec(n: usize, secs: f64) -> String {
    let mb = (n as f64) / (1024.0 * 1024.0);
    format!("{:.0} MB/s", mb / secs)
}

#[test]
fn copy_cost_into_and_out_of_a_linear_memory() {
    let engine = Engine::default();
    let module = Module::new(&engine, WAT).unwrap();
    let mut store = Store::new(&engine, ());
    let instance: Instance = Linker::new(&engine).instantiate(&mut store, &module).unwrap();
    let mem = instance.get_memory(&mut store, "memory").unwrap();

    // A column of `Transform`-sized records, at a few plausible entity counts.
    const STRIDE: usize = 40;
    for entities in [8usize, 1_000, 10_000, 50_000] {
        let n = entities * STRIDE;
        let src = vec![0xABu8; n];
        let mut back = vec![0u8; n];

        let reps = 200;
        let t = Instant::now();
        for _ in 0..reps {
            mem.write(&mut store, 0, &src).unwrap();
            mem.read(&store, 0, &mut back).unwrap();
        }
        let secs = t.elapsed().as_secs_f64();
        let per_frame_us = secs / reps as f64 * 1e6;
        println!(
            "{entities:>6} entities ({:>7} B)  round trip {:>8.2} us/frame   {}",
            n,
            per_frame_us,
            bytes_per_sec(n * 2 * reps, secs)
        );
    }

}


