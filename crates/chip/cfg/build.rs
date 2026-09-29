//! Embed every tracked chip profile, so a build script that selects a chip
//! reads its properties without knowing where the repository lies.

use std::{env, fmt::Write as _, fs, path::PathBuf};

fn main() {
    let platform = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by Cargo"))
        .join("../../../platform")
        .canonicalize()
        .expect("the repository's platform directory");
    println!("cargo::rerun-if-changed={}", platform.display());
    let mut profiles = Vec::new();
    for entry in fs::read_dir(&platform).expect("readable platform directory") {
        let entry = entry.expect("readable platform entry");
        let profile = entry.path().join("chip.toml");
        if profile.is_file() {
            println!("cargo::rerun-if-changed={}", profile.display());
            profiles.push((entry.file_name().to_string_lossy().into_owned(), profile));
        }
    }
    profiles.sort();
    let mut source = String::from("pub(crate) const PROFILES: &[(&str, &str)] = &[\n");
    for (id, path) in profiles {
        writeln!(
            source,
            "    ({id:?}, include_str!({:?})),",
            path.display().to_string()
        )
        .expect("formatting to a string");
    }
    source.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("set by Cargo")).join("profiles.rs"),
        source,
    )
    .expect("writable OUT_DIR");
}
