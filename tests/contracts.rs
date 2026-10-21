//! One contract harness avoids recompiling/linking the same dependencies per file.
//! Git publication traces and the catalog verification cascade remain separate gates.

mod support {
    pub mod git;
    pub mod memory;
}

mod agent_capture;
mod agent_context;
mod agent_preview;
mod agent_requests;
mod application;
mod catalog;
mod catalog_conformance;
mod cli;
mod cli_debug;
mod cli_revisions;
mod codec;
mod config;
mod enum_values;
mod envelopes;
mod frontend;
mod frontend_launch;
mod git_process;
mod inspection;
mod local_faults;
mod local_git_adoption;
mod main_alignment;
mod native_open;
mod process;
mod reconciliation;
mod runtime_arrangements;
mod runtime_conversations;
mod runtime_inference;
mod runtime_launch;
mod runtime_native;
mod runtime_registry;
mod runtime_revisions;
mod runtime_state;
mod state;
mod storage_conformance;
mod transaction;

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
