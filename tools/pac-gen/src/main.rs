use std::{
    env,
    error::Error,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

use svd2rust::{
    config::{Config, RustEdition},
    Target,
};

const USAGE: &str = "usage: cargo pac-gen [--check]";

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("pac-gen must remain under tools/pac-gen")
        .to_owned()
}

fn format_generated(source: &str) -> Result<String, Box<dyn Error>> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2021"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("piped rustfmt stdin must exist")
        .write_all(source.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(format!("rustfmt exited with {}", output.status).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn run() -> Result<(), Box<dyn Error>> {
    let check = match env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [argument] if argument == "--check" => true,
        _ => return Err(USAGE.into()),
    };

    let root = repository_root();
    let svd_path = root.join("svd/esp32s31-radio.svd");
    let output_path = root.join("crates/open-esp-radio-svd-esp32s31/src/lib.rs");
    let input = fs::read_to_string(&svd_path)?;

    let mut config = Config::default();
    config.edition = RustEdition::E2021;
    config.target = Target::None;
    config.strict = true;
    let generated = format_generated(&svd2rust::generate(&input, &config)?.lib_rs)?;

    if check {
        let checked_in = fs::read_to_string(&output_path)?;
        if checked_in != generated {
            return Err(format!(
                "{} differs from {}; run `cargo pac-gen`",
                output_path.display(),
                svd_path.display()
            )
            .into());
        }
    } else {
        fs::create_dir_all(
            output_path
                .parent()
                .expect("generated source path must have a parent"),
        )?;
        fs::write(&output_path, generated)?;
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("PAC generation failed: {error}");
            ExitCode::FAILURE
        }
    }
}
