//! The driver as `RUSTC_WRAPPER` of one firmware-target crate: its facts name
//! each indirect call's candidates by symbol.

use std::path::{Path, PathBuf};
use std::process::Command;

fn sysroot() -> PathBuf {
    let output = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .unwrap();
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
}

fn facts(sample: &Path) -> serde_json::Value {
    let sysroot = sysroot();
    let directory = tempfile_dir();
    let status = Command::new(env!("CARGO_BIN_EXE_oer-mir-facts"))
        .arg(sysroot.join("bin/rustc"))
        .args([
            "--crate-type",
            "lib",
            "--edition",
            "2024",
            "-O",
            "--emit",
            "metadata",
        ])
        .args(["--target", "riscv32imafc-unknown-none-elf"])
        .arg("--out-dir")
        .arg(&directory)
        .arg(sample)
        .env("OER_MIR_FACTS_DIR", &directory)
        .env("OER_MIR_FACTS_TARGET", "riscv32imafc-unknown-none-elf")
        .env("LD_LIBRARY_PATH", sysroot.join("lib"))
        .status()
        .unwrap();
    assert!(status.success());
    let file = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("the crate's facts");
    let value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&directory);
    value
}

fn tempfile_dir() -> PathBuf {
    let directory = std::env::temp_dir().join(format!("oer-mir-facts-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn symbols_containing<'a>(set: &'a serde_json::Value, part: &str) -> Vec<&'a str> {
    set.as_array()
        .unwrap()
        .iter()
        .filter_map(|symbol| symbol.as_str())
        .filter(|symbol| symbol.contains(part))
        .collect()
}

#[test]
fn a_crate_s_facts_name_the_candidates_of_its_pointer_and_dyn_calls() {
    let facts = facts(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.rs"));
    let calls = facts["calls"].as_object().unwrap();
    let field = calls
        .iter()
        .find(|(symbol, _)| symbol.contains("call_field"))
        .unwrap()
        .1;
    assert_eq!(field[0]["fn_pointer"], "fn(u32) -> u32");
    let dyn_call = calls
        .iter()
        .find(|(symbol, _)| symbol.contains("call_dyn"))
        .unwrap()
        .1;
    assert_eq!(dyn_call[0]["dyn"]["trait"], "sample::Speak");
    let entry = dyn_call[0]["dyn"]["entry"].as_u64().unwrap().to_string();
    let pointers = &facts["fn_pointers"]["fn(u32) -> u32"];
    assert_eq!(symbols_containing(pointers, "sample::double").len(), 1);
    assert_eq!(symbols_containing(pointers, "sample::triple").len(), 1);
    let speak = &facts["vtables"]["sample::Speak"][entry.as_str()];
    assert_eq!(symbols_containing(speak, "Speak>::speak").len(), 2);
    // Entry 0 is each type's drop glue.
    assert_eq!(
        facts["vtables"]["sample::Speak"]["0"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

/// Run the driver on one crate of `directory`, with `extra` arguments.
fn compile(directory: &Path, source: &Path, extra: &[&std::ffi::OsStr]) {
    let sysroot = sysroot();
    let status = Command::new(env!("CARGO_BIN_EXE_oer-mir-facts"))
        .arg(sysroot.join("bin/rustc"))
        .args([
            "--crate-type",
            "lib",
            "--edition",
            "2024",
            "-O",
            "--emit",
            "metadata",
        ])
        .args(["--target", "riscv32imafc-unknown-none-elf"])
        .arg("--out-dir")
        .arg(directory)
        .args(extra)
        .arg(source)
        .env("OER_MIR_FACTS_DIR", directory)
        .env("OER_MIR_FACTS_TARGET", "riscv32imafc-unknown-none-elf")
        .env("LD_LIBRARY_PATH", sysroot.join("lib"))
        .status()
        .unwrap();
    assert!(status.success());
}

fn crate_facts(directory: &Path, krate: &str) -> serde_json::Value {
    let file = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!("{krate}-")) && name.ends_with(".json")
                })
        })
        .unwrap();
    serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap()
}

/// A type and a trait print alike in the crate that defines them and in a
/// crate that names them through a re-export: their full defining paths.
#[test]
fn a_type_and_a_trait_key_alike_in_every_crate() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let directory = std::env::temp_dir().join(format!("oer-mir-facts-two-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    compile(&directory, &tests.join("defining.rs"), &[]);
    let mut extern_defining = std::ffi::OsString::from("defining=");
    extern_defining.push(directory.join("libdefining.rmeta"));
    compile(
        &directory,
        &tests.join("using.rs"),
        &["--extern".as_ref(), extern_defining.as_os_str()],
    );
    let defining = crate_facts(&directory, "defining");
    let using = crate_facts(&directory, "using");
    let call = |name: &str| {
        using["calls"]
            .as_object()
            .unwrap()
            .iter()
            .find(|(symbol, _)| symbol.ends_with(name))
            .unwrap()
            .1[0]
            .clone()
    };
    let pointer = call("drop_packet")["fn_pointer"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        defining["fn_pointers"].get(&pointer).is_some(),
        "{pointer} not among {}",
        defining["fn_pointers"]
    );
    let trait_name = call("send")["dyn"]["trait"].as_str().unwrap().to_owned();
    assert_eq!(trait_name, "defining::api::Driver");
    assert!(
        defining["vtables"].get(&trait_name).is_some(),
        "{}",
        defining["vtables"]
    );
    let _ = std::fs::remove_dir_all(&directory);
}
