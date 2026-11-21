//! One contract harness avoids recompiling/linking the same dependencies per file.
//! Git publication traces remain a separate gate.

mod support {
    pub mod hc;
    pub mod herdr;
    pub mod pty;
    pub mod record_io;
}

mod application_records;
mod card_watch;
mod cli;
mod cli_debug;
mod codec;
mod config;
mod enum_values;
mod frontend;
mod git_process;
mod process;
mod record_local;
mod record_updates;
mod runtime_conversations;
mod runtime_inference;
mod runtime_native;
mod runtime_registry;
mod runtime_revisions;
mod runtime_state;
mod tui_pty;

#[test]
fn every_root_test_file_is_registered() {
    let manifest = include_str!("../Cargo.toml");
    let harness = include_str!("contracts.rs");
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "rs") {
            let name = path.file_stem().unwrap().to_str().unwrap();
            let file = path.file_name().unwrap().to_str().unwrap();
            let in_harness = file != "contracts.rs"
                && harness.lines().any(|line| line == format!("mod {name};"));
            let in_manifest = manifest
                .lines()
                .any(|line| line == format!("path = \"tests/{file}\""));
            assert_ne!(
                in_harness, in_manifest,
                "test file must be registered exactly once: {file}"
            );
        }
    }
}
