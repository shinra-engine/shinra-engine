//! Proves `advance_steps` — and so the `Clock` sequence `Vm::frame` sees — is
//! unaffected by whatever `should_skip_render` decides. Render pacing and
//! simulation pacing are two independent reads of the same `dt`; this test
//! is what stops a future "optimization" from coupling them.

use super::*;

/// Drives `advance_steps` over a fixed `dt` sequence and records the
/// `Clock` each simulated step would see, exactly as `run`'s loop does: a
/// frame index that only ever increments once per step taken. `last_costs`
/// is read only to call `should_skip_render`, whose result is discarded —
/// mirroring that nothing feeds it back into `acc` or the frame index.
fn simulate(dts: &[f32], last_costs: &[Duration]) -> Vec<se_vm::clock::Clock> {
    let mut acc = 0f32;
    let mut index = 0u32;
    let mut clocks = Vec::new();
    for (&dt, &last_cost) in dts.iter().zip(last_costs) {
        let (steps, new_acc) = advance_steps(acc, dt);
        acc = new_acc;
        let _ = should_skip_render(Some(TARGET), last_cost);
        for _ in 0..steps {
            clocks.push(se_vm::clock::Clock::at(index));
            index += 1;
        }
    }
    clocks
}

#[test]
fn render_skip_does_not_change_the_simulation_clock_sequence() {
    let dts = [0.016, 0.033, 0.05, 0.016, 0.1, 0.016, 0.0, 0.2, 0.016];

    let never_over_budget = vec![Duration::ZERO; dts.len()];
    let baseline = simulate(&dts, &never_over_budget);

    // Forced high enough on alternating iterations to trip should_skip_render
    // every other step — never zero steps taken in this run, so the forcing
    // below is exercised, not vacuous.
    let sometimes_over_budget: Vec<Duration> = (0..dts.len())
        .map(|i| if i % 2 == 0 { TARGET + Duration::from_secs(1) } else { Duration::ZERO })
        .collect();
    let with_skips = simulate(&dts, &sometimes_over_budget);

    assert_eq!(baseline, with_skips, "frame indices, t and dt must be identical with or without a skipped render");

    // Sanity: the forced costs actually do and don't trip the decision, so
    // the comparison above isn't accidentally comparing two identical runs.
    assert!(should_skip_render(Some(TARGET), TARGET + Duration::from_secs(1)));
    assert!(!should_skip_render(Some(TARGET), Duration::ZERO));
}

#[test]
fn advance_steps_caps_at_four_and_keeps_the_remainder() {
    // dt is clamped to 0.25s before stepping, so a 10s stall still only
    // drains 4 * STEP from it; the rest is retained, not discarded.
    let (steps, acc) = advance_steps(0.0, 10.0);
    assert_eq!(steps, 4);
    assert!((acc - (0.25 - 4.0 * se_vm::clock::STEP)).abs() < 1e-6);
}

#[test]
fn advance_steps_carries_a_partial_step_to_the_next_call() {
    let (steps, acc) = advance_steps(0.0, se_vm::clock::STEP * 1.5);
    assert_eq!(steps, 1);
    let (steps2, _) = advance_steps(acc, 0.0);
    assert_eq!(steps2, 0, "half a step held over should not round up on its own");
}
