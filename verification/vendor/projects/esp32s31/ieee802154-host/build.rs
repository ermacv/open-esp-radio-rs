//! Compile the pinned public ESP-IDF IEEE 802.15.4 driver for the host.
//!
//! Every vendor file is read from `OER_ESP_IDF_DIR` and must match the SHA-256
//! recorded in `vendor-sources.toml`. The build generates a recording
//! replacement for every `ieee802154_ll_*` accessor declared by the real
//! common LL header, so the compiled driver keeps its own control flow while
//! each register-layer call reaches the Rust recorder.

use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

const SOURCE_ENV: &str = "OER_ESP_IDF_DIR";
const COMMON_LL: &str = "components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h";

const INCLUDE_DIRS: &[&str] = &[
    "components/ieee802154/include",
    "components/ieee802154/private_include",
    "components/esp_hal_ieee802154/include",
    "components/esp_hal_ieee802154/esp32s31/include",
    "components/soc/esp32s31/include",
    "components/soc/esp32s31/register",
    "components/esp_coex/include",
];

struct Source {
    path: String,
    sha256: String,
}

/// Read the `[[source]]` entries of the ledger. The ledger is a flat list of
/// `path`/`sha256` pairs; any other key is rejected rather than ignored.
fn ledger(text: &str) -> (String, Vec<Source>) {
    let mut revision = None;
    let mut sources = Vec::new();
    let mut path = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line == "[[source]]" {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("malformed ledger line: {line}"));
        let value = value.trim().trim_matches('"').to_owned();
        match key.trim() {
            "revision" => revision = Some(value),
            "path" => path = Some(value),
            "sha256" => sources.push(Source {
                path: path.take().expect("sha256 must follow its path"),
                sha256: value,
            }),
            other => panic!("unknown ledger key: {other}"),
        }
    }
    assert!(path.is_none(), "ledger path without sha256");
    (revision.expect("ledger revision"), sources)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
        out
    })
}

struct Accessor {
    returns: String,
    name: String,
    parameters: Vec<(String, String)>,
}

/// Parse the one-line `FORCE_INLINE_ATTR`/`static inline` accessor
/// signatures of the common LL header.
fn accessors(header: &str) -> Vec<Accessor> {
    let mut found = Vec::new();
    for line in header.lines() {
        let Some(signature) = line
            .strip_prefix("FORCE_INLINE_ATTR ")
            .or_else(|| line.strip_prefix("static inline "))
        else {
            continue;
        };
        let (Some(paren), Some(close)) = (signature.find('('), signature.rfind(')')) else {
            continue;
        };
        // The accessor name is the last token before the parameter list; the
        // return type may itself be an `ieee802154_ll_*` typedef.
        let head = signature[..paren].trim_end();
        let split = head.rfind([' ', '*']).expect("accessor return type");
        let name = head[split + 1..].to_owned();
        if !name.starts_with("ieee802154_ll_") {
            continue;
        }
        let returns = head[..=split].trim().to_owned();
        let list = signature[paren + 1..close].trim();
        let parameters = if list == "void" || list.is_empty() {
            Vec::new()
        } else {
            list.split(',')
                .map(|parameter| {
                    let parameter = parameter.trim();
                    let split = parameter.rfind([' ', '*']).expect("typed parameter");
                    (
                        parameter[..=split].trim().to_owned(),
                        parameter[split + 1..].trim().to_owned(),
                    )
                })
                .collect()
        };
        found.push(Accessor {
            returns,
            name,
            parameters,
        });
    }
    assert!(!found.is_empty(), "no LL accessors found in {COMMON_LL}");
    found
}

fn generate(accessors: &[Accessor], out: &Path) -> PathBuf {
    let include = out.join("include");
    fs::create_dir_all(&include).expect("generated include directory");
    let mut rename = String::from("/* Generated: move vendor LL bodies aside. */\n");
    let mut restore = String::from("/* Generated: release the vendor LL names. */\n");
    let mut declarations = String::from(
        "/* Generated: recording LL accessors with the vendor signatures. */\n#pragma once\n",
    );
    let mut bodies = String::from(
        "/* Generated: forward every LL accessor to the Rust recorder. */\n\
         #include <stdint.h>\n#include \"hal/ieee802154_common_ll.h\"\n\
         uint64_t oer_host_record(const char *name, const uint64_t *arguments, uint32_t count,\n                        uint32_t pointers);\n",
    );
    for accessor in accessors {
        let parameters = if accessor.parameters.is_empty() {
            "void".to_owned()
        } else {
            accessor
                .parameters
                .iter()
                .map(|(ty, name)| format!("{ty} {name}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let name = &accessor.name;
        writeln!(rename, "#define {name} oer_vendor_{name}").unwrap();
        writeln!(restore, "#undef {name}").unwrap();
        writeln!(declarations, "{} {name}({parameters});", accessor.returns).unwrap();
        let arguments = accessor
            .parameters
            .iter()
            .map(|(ty, name)| {
                if ty.contains('*') {
                    format!("(uint64_t)(uintptr_t){name}")
                } else {
                    format!("(uint64_t)(int64_t){name}")
                }
            })
            .collect::<Vec<_>>();
        // Bit `n` marks argument `n` as a buffer address, which the recorder
        // labels symbolically so traces do not depend on address layout.
        let pointers = accessor
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, (ty, _))| ty.contains('*'))
            .fold(0u32, |mask, (index, _)| mask | 1 << index);
        let call = if arguments.is_empty() {
            format!("oer_host_record(\"{name}\", 0, 0, 0)")
        } else {
            writeln!(
                bodies,
                "{} {name}({parameters})\n{{\n    uint64_t arguments[{}] = {{ {} }};",
                accessor.returns,
                arguments.len(),
                arguments.join(", ")
            )
            .unwrap();
            format!(
                "oer_host_record(\"{name}\", arguments, {}, {pointers:#x})",
                arguments.len()
            )
        };
        if arguments.is_empty() {
            writeln!(bodies, "{} {name}({parameters})\n{{", accessor.returns).unwrap();
        }
        if accessor.returns == "void" {
            writeln!(bodies, "    (void){call};\n}}").unwrap();
        } else {
            writeln!(bodies, "    return ({}){call};\n}}", accessor.returns).unwrap();
        }
    }
    fs::write(include.join("oer_ll_rename.h"), rename).unwrap();
    fs::write(include.join("oer_ll_restore.h"), restore).unwrap();
    fs::write(include.join("oer_ll_recorder.h"), declarations).unwrap();
    let recorder = out.join("oer_ll_recorder.c");
    fs::write(&recorder, bodies).unwrap();
    recorder
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!("cargo::rerun-if-env-changed={SOURCE_ENV}");
    println!("cargo::rerun-if-changed=vendor-sources.toml");
    println!("cargo::rerun-if-changed=shim");

    let idf = env::var_os(SOURCE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "{SOURCE_ENV} must name an ESP-IDF checkout at the ledger revision; \
             see verification/vendor/projects/esp32s31/ieee802154-host/README.md"
            )
        });
    let (revision, sources) =
        ledger(&fs::read_to_string(manifest.join("vendor-sources.toml")).unwrap());
    for source in &sources {
        let path = idf.join(&source.path);
        println!("cargo::rerun-if-changed={}", path.display());
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("{}: {error} (ESP-IDF {revision})", path.display()));
        let digest = hex(&Sha256::digest(&bytes));
        assert_eq!(
            digest, source.sha256,
            "{} does not match ESP-IDF {revision}",
            source.path
        );
    }

    let recorder = generate(
        &accessors(&fs::read_to_string(idf.join(COMMON_LL)).unwrap()),
        &out,
    );

    let mut build = cc::Build::new();
    build
        .std("gnu11")
        .warnings(false)
        .flag("-include")
        .flag("esp_intr_alloc.h")
        .include(out.join("include"))
        .include(manifest.join("shim/include"));
    for dir in INCLUDE_DIRS {
        build.include(idf.join(dir));
    }
    for source in sources.iter().filter(|source| source.path.ends_with(".c")) {
        build.file(idf.join(&source.path));
    }
    build
        .file(recorder)
        .file(manifest.join("shim/src/host.c"))
        .compile("oer_esp32s31_ieee802154_vendor");
}
