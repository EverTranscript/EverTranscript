//! How much of a model to put on the accelerator, and how hard to push.
//!
//! M4's ticket claims the sidecar is "spawned at reduced priority and with a
//! layers-that-fit calculation". It was not: the load used
//! `LlamaModelParams::default()`, which offloads **every** layer, and nothing
//! anywhere touched scheduling priority. At 491 MB that was invisible. The
//! registered model is about to be five times larger, on machines that share
//! one pool of memory between the accelerator, whisper, and a recording in
//! progress — so the claim has to become true before it starts to matter.
//!
//! The decision is a pure function of three numbers, so it can be run by a
//! test rather than reasoned about. Probing the machine is a thin edge around
//! it.

/// Fraction of the machine's memory a Summary may plan to occupy.
///
/// **Deliberately not most of it.** A Summary is the lowest-priority thing
/// this product does — it runs after a Meeting, it is not an Anchor, and
/// ADR-0019 says losing it must never cost the recording. Whisper's model,
/// the Core, the Client and the operating system are all entitled to memory
/// while a Summary is generating, and on a Mac the accelerator draws from
/// the same pool rather than a separate card.
const MEMORY_BUDGET: f64 = 0.55;

/// Room to leave beyond the weights themselves.
///
/// The KV cache scales with context, and the context budget grew with the
/// model. Reserving a flat share is cruder than computing it from the
/// context size and the model's head geometry, and it is deliberately the
/// cruder thing: an estimate that is wrong in the safe direction costs some
/// offload, while one wrong in the other direction costs the machine.
const OVERHEAD: f64 = 1.35;

/// How many of a model's layers to put on the accelerator.
///
/// All of them when the model comfortably fits, none when it cannot fit at
/// all, and a proportional share in between — llama.cpp will keep the
/// remainder on the CPU, which is slower and does not fail.
///
/// `model_bytes` is the size of the file on disk, which for a GGUF is the
/// size of the weights. It is taken rather than read from the loaded model
/// because this has to be decided *before* the load it configures.
pub fn layers_that_fit(model_bytes: u64, layers: u32, total_memory_bytes: u64) -> u32 {
    if layers == 0 {
        return 0;
    }
    let budget = (total_memory_bytes as f64 * MEMORY_BUDGET) / OVERHEAD;
    if budget <= 0.0 {
        return 0;
    }
    if (model_bytes as f64) <= budget {
        return layers;
    }
    // Proportional: the share of the model that fits is the share of the
    // layers we ask for. Rounded down, because the last layer that nearly
    // fits is the one that costs the machine.
    let share = budget / model_bytes as f64;
    let fitting = (layers as f64 * share).floor();
    fitting.max(0.0).min(layers as f64) as u32
}

/// What the machine has, or `0` when it cannot be determined.
///
/// Zero is a deliberate answer rather than an error: [`layers_that_fit`]
/// reads it as "offload nothing", which runs on the CPU. A probe that fails
/// should cost speed, never the feature.
pub fn total_memory_bytes() -> u64 {
    use sysinfo::System;
    let mut system = System::new();
    system.refresh_memory();
    system.total_memory()
}

/// Asks the operating system to schedule this process behind everything else.
///
/// **A Summary is the lowest-priority thing this product does.** It runs
/// after a Meeting, it is not an Anchor, and ADR-0019 makes losing it
/// cheaper than losing a recording — so when a Summary and a recording want
/// the same core, the recording should win without either of them having to
/// know about the other.
///
/// Best-effort by design: a platform that refuses is not a reason to fail,
/// it is a reason to run at normal priority. The result is reported so the
/// difference is visible rather than assumed.
pub fn lower_priority() -> bool {
    #[cfg(unix)]
    {
        // 10 of a possible 19: clearly behind interactive work, without
        // being so far back that a Summary never finishes on a busy machine.
        // SAFETY: setpriority with PRIO_PROCESS and pid 0 addresses this
        // process and touches nothing else.
        unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 10) == 0 }
    }
    #[cfg(windows)]
    {
        use windows::Win32::System::Threading::BELOW_NORMAL_PRIORITY_CLASS;
        use windows::Win32::System::Threading::GetCurrentProcess;
        use windows::Win32::System::Threading::SetPriorityClass;
        // SAFETY: a pseudo-handle to this process, and a documented class.
        unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS).is_ok() }
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes in a gibibyte, so the cases below read as machines.
    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn a_small_model_on_a_large_machine_goes_entirely_to_the_accelerator() {
        // The 0.5B on any modern laptop: nothing to think about.
        assert_eq!(layers_that_fit(491 * GIB / 1024, 24, 32 * GIB), 24);
    }

    #[test]
    fn a_four_billion_parameter_model_fits_a_sixteen_gigabyte_machine() {
        // 2.5 GB of weights against a 16 GB budget — comfortably inside even
        // after the overhead allowance, so all 36 layers go over.
        assert_eq!(layers_that_fit(2_546_341_152, 36, 16 * GIB), 36);
    }

    #[test]
    fn the_same_model_is_split_on_a_machine_that_cannot_hold_it() {
        // 4 GB total: the budget lands under the model, so only a share of
        // the layers are offloaded and llama.cpp keeps the rest on the CPU.
        let layers = layers_that_fit(2_546_341_152, 36, 4 * GIB);
        assert!(
            layers > 0 && layers < 36,
            "a marginal machine should get a partial offload, got {layers}"
        );
    }

    #[test]
    fn a_machine_too_small_to_hold_any_of_it_offloads_nothing() {
        // Rather than offloading one layer and thrashing. Zero is a valid
        // answer: llama.cpp runs the whole model on the CPU, slowly.
        assert_eq!(layers_that_fit(2_546_341_152, 36, 128 * 1024 * 1024), 0);
    }

    #[test]
    fn the_answer_never_exceeds_the_layers_the_model_has() {
        // The failure that would matter most: asking for more layers than
        // exist is how a "fitting" calculation becomes a crash.
        for memory in [GIB, 8 * GIB, 64 * GIB, 512 * GIB] {
            let layers = layers_that_fit(1_000_000, 36, memory);
            assert!(layers <= 36, "got {layers} layers for {memory} bytes");
        }
    }

    #[test]
    fn a_model_with_no_layers_asks_for_none() {
        assert_eq!(layers_that_fit(2_546_341_152, 0, 64 * GIB), 0);
    }

    #[test]
    fn a_machine_reporting_no_memory_offloads_nothing_rather_than_dividing_by_it() {
        // A probe that fails answers zero, and zero must not become a panic
        // or an accidental "everything".
        assert_eq!(layers_that_fit(2_546_341_152, 36, 0), 0);
    }
}
