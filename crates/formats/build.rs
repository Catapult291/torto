//! Persisted source IR must never outlive a change to its parser or model,
//! including development builds which keep the same release version.
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

fn hash_sources(root: &Path, hash: &mut Sha256) {
    let mut entries = fs::read_dir(root)
        .expect("reading parser sources")
        .map(|entry| entry.expect("reading source entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            hash_sources(&path, hash);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let bytes = fs::read(&path).expect("reading parser source");
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
    }
}

fn main() {
    let mut hash = Sha256::new();
    for root in ["src", "../html/src", "../publication/src"] {
        println!("cargo:rerun-if-changed={root}");
        hash_sources(Path::new(root), &mut hash);
    }
    for path in [
        "build.rs",
        "Cargo.toml",
        "../html/Cargo.toml",
        "../publication/Cargo.toml",
        "../../Cargo.lock",
    ] {
        println!("cargo:rerun-if-changed={path}");
        hash.update(fs::read(path).expect("reading parser build inputs"));
    }
    println!(
        "cargo:rustc-env=TORTO_SOURCE_IR_BUILD={:x}",
        hash.finalize()
    );
}
