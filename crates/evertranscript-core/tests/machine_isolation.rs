//! A Core built for a test reads the paths it was given, not the machine's.
//!
//! History and settings were already scoped this way, and `tray_control`
//! states the reason: a Core that reads the machine "makes the test pass or
//! fail depending on who ran the app here". Models were the one path left
//! global, and the asymmetry was invisible until a machine that had actually
//! fetched a model ran the suite: `open_transcriber` found 874 MB of
//! large-v3-turbo, and tests documented as using "a script, so no hardware
//! is involved" began doing real inference inside `stop`.

use evertranscript_core::Core;
use evertranscript_protocol::ModelAvailability;

/// Writes something at the model's expected filename. Wrong size on purpose:
/// `Corrupted` proves the file was *found*, which `Missing` cannot.
fn plant_a_decoy(models_dir: &std::path::Path) {
    std::fs::create_dir_all(models_dir).expect("models dir");
    std::fs::write(
        models_dir.join("ggml-large-v3-turbo-q8_0.bin"),
        b"not a model",
    )
    .expect("decoy");
}

fn availability(core: &Core) -> ModelAvailability {
    core.models_status()
        .expect("models status")
        .models
        .first()
        .expect("the registry is not empty")
        .state
}

#[test]
fn a_core_reads_the_models_directory_it_was_given() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stocked = dir.path().join("stocked");
    plant_a_decoy(&stocked);

    let seeing = Core::with_paths_and_models(
        dir.path().join("HistoryA"),
        dir.path().join("a.json"),
        stocked,
    )
    .expect("core");
    let blind = Core::with_paths_and_models(
        dir.path().join("HistoryB"),
        dir.path().join("b.json"),
        dir.path().join("empty"),
    )
    .expect("core");

    // Same machine, same moment: the only difference is the directory each
    // was handed, so anything but disagreement means one of them ignored it.
    assert_eq!(availability(&seeing), ModelAvailability::Corrupted);
    assert_eq!(availability(&blind), ModelAvailability::Missing);
}

#[test]
fn a_scoped_core_cannot_see_a_model_this_machine_has_fetched() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Nothing is planted: the machine's own models directory is read-only as
    // far as this suite is concerned, and on a machine that has fetched the
    // model it is already the decoy this needs.
    let core = Core::with_history_dir_acknowledged(dir.path().join("History")).expect("core");

    assert_eq!(
        availability(&core),
        ModelAvailability::Missing,
        "a test Core must start with no model, whatever this machine has fetched"
    );
}
