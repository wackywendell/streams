//! Embed a digest of the sources this binary was built from.
//!
//! The Pi extension computes the same digest over its checkout and refuses a
//! binary built from other sources (`checkoutSource` in
//! `extensions/stream-checkpoint/streams-bin.ts`).
//! Both sides hash `Cargo.toml`, `Cargo.lock`, `build.rs` and every file under
//! `src/`, sorted by `/`-separated relative path, each as
//! `path NUL length NUL bytes`.

use sha2::{Digest, Sha256};
use std::{fs, path::Path};

fn files(root: &Path, relative: &str, out: &mut Vec<String>) {
    for entry in fs::read_dir(root.join(relative)).expect("read source directory") {
        let entry = entry.expect("read source entry");
        let name = format!(
            "{relative}/{}",
            entry.file_name().to_str().expect("UTF-8 source path")
        );
        if entry.file_type().expect("source file type").is_dir() {
            files(root, &name, out);
        } else {
            out.push(name);
        }
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = vec![
        "Cargo.toml".to_string(),
        "Cargo.lock".into(),
        "build.rs".into(),
    ];
    files(root, "src", &mut paths);
    paths.sort();
    let mut hash = Sha256::new();
    for path in &paths {
        let bytes = fs::read(root.join(path)).expect("read source file");
        hash.update(format!("{path}\0{}\0", bytes.len()));
        hash.update(&bytes);
    }
    // A directory is scanned recursively, so added and removed files rerun this too.
    for path in ["Cargo.toml", "Cargo.lock", "build.rs", "src"] {
        println!("cargo:rerun-if-changed={path}");
    }
    let digest: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    println!("cargo:rustc-env=STREAMS_SOURCE_DIGEST={digest}");
}
