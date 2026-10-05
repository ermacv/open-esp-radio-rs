//! `cargo fw`, the dev kit: build an example's image, flash it or any
//! image bundle to an attached board, monitor a board's console and list the
//! attached boards.
//!
//! It uses the image pipeline (`oer-image`), the devices
//! library (`oer-devices`: discovery, the device lock, the flash writer,
//! consoles), the chip model and the foundation; it knows nothing of the
//! HIL stand. A board busy with another process (a HIL lease, another
//! `cargo fw`) is refused naming its holder, or waited for with `--wait`.

mod examples;

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use clap::{Parser, Subcommand, ValueEnum};
use oer_chip_profile::Profile;
use oer_image_bundle::ImageBundle;

use crate::examples::Example;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Parser)]
#[command(
    name = "cargo fw",
    about = "Build, flash and monitor firmware on attached boards"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build examples into image bundles below `target/firmware`.
    Build {
        /// Examples by name (`station`) or `<chip>/<name>`.
        #[arg(required_unless_present = "all")]
        examples: Vec<String>,
        /// Every example of every chip.
        #[arg(long, conflicts_with = "examples")]
        all: bool,
        #[command(flatten)]
        options: BuildOptions,
        /// Only `cargo check` the runtime with the image's target, features
        /// and compiler flags: no image, so no image check can run on it.
        #[arg(long, conflicts_with = "checks")]
        type_check: bool,
    },
    /// Write an image to an attached board and start it: an example (built
    /// first), an image bundle's directory or an ESP-IDF catalog image
    /// (built first).
    Flash {
        #[arg(value_name = "EXAMPLE|BUNDLE|CATALOG-IMAGE")]
        image: String,
        #[command(flatten)]
        options: BuildOptions,
        #[command(flatten)]
        device: DeviceOptions,
        /// Monitor the console after the write.
        #[arg(long)]
        monitor: bool,
    },
    /// Read a board's console.
    Monitor {
        #[command(flatten)]
        device: DeviceOptions,
        /// Reset the board into its application first.
        #[arg(long)]
        reset: bool,
        #[command(flatten)]
        capture: CaptureOptions,
    },
    /// The attached boards: port, MAC, known chip and the holder of each.
    Devices {
        /// Connect to each free board's ROM to learn its chip.
        #[arg(long)]
        probe: bool,
        #[arg(long)]
        json: bool,
    },
    /// The examples of every chip.
    List,
    /// Compare two linked ELF images function by function, modulo
    /// placement (a binary analysis: runs with the `checks` build).
    Compare {
        old: PathBuf,
        new: PathBuf,
        /// Reviewed rename applied to both images, FROM=TO (e.g. a moved path).
        #[arg(long = "alias")]
        aliases: Vec<String>,
        /// Reviewed function whose code may differ (a scheduling tie).
        #[arg(long = "allow")]
        allowed: Vec<String>,
        /// Print the instruction diff of differing functions whose name
        /// contains this.
        #[arg(long)]
        show: Vec<String>,
    },
}

#[derive(clap::Args, Clone, Default)]
struct BuildOptions {
    #[arg(long, value_delimiter = ',')]
    features: Vec<String>,
    #[arg(long)]
    no_default_features: bool,
    /// Checks of the built image: binary analyses with the chip's pinned
    /// ROM. None by default.
    #[arg(long = "check", value_enum, value_delimiter = ',')]
    checks: Vec<CheckArg>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CheckArg {
    Stack,
    Placement,
    Interrupts,
    All,
}

#[derive(clap::Args, Clone, Default)]
struct DeviceOptions {
    /// The board: its MAC or port; default the only attached board.
    #[arg(long, value_name = "MAC|PORT")]
    device: Option<String>,
    /// Wait while another process holds the board instead of failing.
    #[arg(long)]
    wait: bool,
}

#[derive(clap::Args, Clone, Default)]
struct CaptureOptions {
    /// Stop after this long (`30s`, `2m`); default until interrupted.
    #[arg(long = "for", value_name = "DURATION", value_parser = parse_duration)]
    duration: Option<Duration>,
    /// Stop at the first line containing TEXT; fails when it never appears.
    #[arg(long, value_name = "TEXT")]
    until: Option<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("cargo fw: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    if oer_command_tree::requested() {
        use clap::CommandFactory as _;
        let tree = oer_command_tree::command_tree(&Cli::command(), &[String::from("fw")]);
        println!("{}", oer_command_tree::json(&tree));
        return Ok(ExitCode::SUCCESS);
    }
    let cli = Cli::parse();
    let root = oer_process::Checkout::discover("cargo fw")?.root;
    if !cfg!(feature = "checks") && cli.command.checks_requested() {
        return with_checks(&root);
    }
    match cli.command {
        Command::Build {
            examples,
            all,
            options,
            type_check,
        } => {
            let selected = if all {
                examples::all(&root)?
            } else {
                examples
                    .iter()
                    .map(|name| examples::find(&root, name))
                    .collect::<Result<Vec<_>>>()?
            };
            let mut failed = Vec::new();
            for example in &selected {
                let result = if type_check {
                    type_check_example(&root, example, &options)
                } else {
                    build_example(&root, example, &options).map(drop)
                };
                if let Err(error) = result {
                    eprintln!("{}: {error}", example.qualified());
                    failed.push(example.qualified());
                }
            }
            if !failed.is_empty() {
                return Err(format!("failed: {}", failed.join(", ")).into());
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Flash {
            image,
            options,
            device,
            monitor,
        } => flash(&root, &image, &options, &device, monitor),
        Command::Monitor {
            device,
            reset,
            capture,
        } => {
            let opened = oer_devices::device::find(device.device.as_deref())?
                .open(&command_line(), device.wait)?;
            let console = if reset {
                opened.reset()?
            } else {
                opened.console()?
            };
            monitor(&root, console, &capture)
        }
        Command::Devices { probe, json } => {
            let mut devices = oer_devices::devices();
            if probe {
                let chips = Profile::all(&root)?;
                for device in &mut devices {
                    if device.holder.is_some() {
                        continue;
                    }
                    match device.clone().open(&command_line(), false) {
                        Ok(mut opened) => match opened.probe(&chips) {
                            Ok(chip) => device.chip = Some(chip),
                            Err(error) => eprintln!("{}: {error}", device.mac),
                        },
                        Err(error) => eprintln!("{}: {error}", device.mac),
                    }
                }
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&devices)?);
            } else if devices.is_empty() {
                println!("no board is attached");
            } else {
                for device in devices {
                    println!(
                        "{:<17} {:<10} {:<60} {}",
                        device.mac,
                        device.chip.as_deref().unwrap_or("?"),
                        device.port.display(),
                        device.holder.map_or_else(
                            || String::from("free"),
                            |holder| format!("busy: {holder}")
                        )
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Compare {
            old,
            new,
            aliases,
            allowed,
            show,
        } => {
            compare(&old, &new, aliases, allowed, show)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::List => {
            for example in examples::all(&root)? {
                println!("{:<24} {}", example.qualified(), example.package);
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

impl Command {
    /// Whether the command asks for image checks (`--check`).
    fn checks_requested(&self) -> bool {
        match self {
            Self::Build { options, .. } | Self::Flash { options, .. } => !options.checks.is_empty(),
            Self::Compare { .. } => true,
            Self::Monitor { .. } | Self::Devices { .. } | Self::List => false,
        }
    }
}

/// Compare `old` with `new` function by function under the reviewed
/// aliases and allowed differences; fails when they differ beyond
/// placement.
#[cfg(feature = "checks")]
fn compare(
    old: &Path,
    new: &Path,
    aliases: Vec<String>,
    allowed: Vec<String>,
    show: Vec<String>,
) -> Result<()> {
    let aliases = aliases
        .into_iter()
        .map(|alias| {
            alias
                .split_once('=')
                .map(|(a, b)| (a.to_owned(), b.to_owned()))
                .ok_or_else(|| format!("alias `{alias}` is not FROM=TO").into())
        })
        .collect::<Result<Vec<_>>>()?;
    let review = oer_image_compare::Review {
        aliases: oer_image_compare::Aliases(aliases),
        allowed: allowed.into_iter().collect(),
        show,
    };
    let comparison = oer_image_compare::compare_elf(old, new, &review)?;
    if comparison.equivalent(&review.allowed) {
        Ok(())
    } else {
        Err("images differ beyond placement".into())
    }
}

/// Without the `checks` feature, `run` re-executes before a comparison.
#[cfg(not(feature = "checks"))]
fn compare(_: &Path, _: &Path, _: Vec<String>, _: Vec<String>, _: Vec<String>) -> Result<()> {
    unreachable!("an image comparison runs with the `checks` feature")
}

/// Run this command again as `oer-fw` built with the `checks` feature: the
/// image checks (and their analyzers) are only compiled into that build.
fn with_checks(root: &Path) -> Result<ExitCode> {
    let status = oer_process::command(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args([
            "run",
            "--quiet",
            "-p",
            "oer-fw",
            "--features",
            "checks",
            "--",
        ])
        .args(std::env::args_os().skip(1))
        .status()?;
    Ok(match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::FAILURE,
    })
}

/// The image checks `checks` ask for, as the image build's gate.
#[cfg(feature = "checks")]
fn gates(checks: &[CheckArg]) -> Option<Box<dyn oer_image::Checks>> {
    use oer_image_checks::{Check, Gates};
    let mut selected = Vec::new();
    for check in checks {
        match check {
            CheckArg::Stack => selected.push(Check::Stack),
            CheckArg::Placement => selected.push(Check::Placement),
            CheckArg::Interrupts => selected.push(Check::Interrupts),
            CheckArg::All => selected.extend([Check::Stack, Check::Placement, Check::Interrupts]),
        }
    }
    (!selected.is_empty()).then(|| Box::new(Gates::of(&selected)) as Box<dyn oer_image::Checks>)
}

/// Without the `checks` feature, `run` re-executes before any build that
/// asks for a check.
#[cfg(not(feature = "checks"))]
fn gates(checks: &[CheckArg]) -> Option<Box<dyn oer_image::Checks>> {
    assert!(checks.is_empty(), "image checks need the `checks` feature");
    None
}

/// This process's command line, for the device lock's holder record.
fn command_line() -> String {
    let mut words = vec![String::from("cargo fw")];
    words.extend(std::env::args().skip(1));
    words.join(" ")
}

/// The image spec of `example` with `options`, building into `output`.
fn spec(
    root: &Path,
    example: &Example,
    options: &BuildOptions,
    output: PathBuf,
) -> oer_image::ImageSpec {
    oer_image::ImageSpec {
        root: root.to_owned(),
        chip: example.profile.id.clone(),
        application: oer_image::Application {
            workspace: example.workspace.clone(),
            package: example.package.clone(),
            binary: example.package.clone(),
            features: options.features.clone(),
            default_features: !options.no_default_features,
        },
        stack_policy: example.profile.stack_policy(),
        layout_seed: None,
        overrides: oer_image::Overrides::default(),
        builder_inputs: Default::default(),
        reads: Vec::new(),
        output,
        checks: gates(&options.checks),
    }
}

/// Build `example` into a new bundle below its directory and keep it: a
/// later build never replaces the files of an earlier one.
fn build_example(root: &Path, example: &Example, options: &BuildOptions) -> Result<ImageBundle> {
    let directory = example.directory(root);
    std::fs::create_dir_all(&directory)?;
    let output = tempfile::Builder::new()
        .prefix("build-")
        .tempdir_in(&directory)?
        .keep();
    let bundle = oer_image::build(&spec(root, example, options, output))?;
    for record in &bundle.checks {
        println!(
            "check {} ({:?}): {}",
            record.check, record.elf, record.result
        );
    }
    for warning in &bundle.warnings {
        println!("warning: {warning}");
    }
    println!("image bundle: {}", bundle.directory.display());
    println!("application image: {}", bundle.application().display());
    Ok(bundle)
}

/// Refuse build options for an image this command does not build (`what`):
/// a check or feature it would not apply must not pass silently.
fn built_elsewhere(options: &BuildOptions, what: &str) -> Result<()> {
    if !options.checks.is_empty() {
        return Err(format!(
            "--check applies to an example this command builds; {what} is flashed as it is"
        )
        .into());
    }
    if !options.features.is_empty() || options.no_default_features {
        return Err(format!(
            "--features and --no-default-features apply to an example; {what} is flashed as it is"
        )
        .into());
    }
    Ok(())
}

/// Type-check an example's runtime exactly as its image build compiles it.
fn type_check_example(root: &Path, example: &Example, options: &BuildOptions) -> Result<()> {
    let spec = spec(
        root,
        example,
        options,
        example.directory(root).join("check"),
    );
    oer_image::type_check(&spec)?;
    println!(
        "{}: the runtime type-checks with the image flags",
        example.qualified()
    );
    Ok(())
}

/// `cargo fw flash`: resolve the image to a bundle, take the board, write
/// and start it, then monitor when asked.
fn flash(
    root: &Path,
    image: &str,
    options: &BuildOptions,
    device: &DeviceOptions,
    monitor_after: bool,
) -> Result<ExitCode> {
    let found = oer_devices::device::find(device.device.as_deref())?;
    let path = Path::new(image);
    let bundle = if path.is_dir() {
        built_elsewhere(options, "an image bundle")?;
        ImageBundle::load(&std::path::absolute(path)?)?
    } else if let Ok(example) = examples::find(root, image) {
        build_example(root, &example, options)?
    } else {
        let entries = oer_image::esp_idf::catalog::entries(root)?;
        let entry = oer_image::esp_idf::catalog::entry(&entries, image)
            .map_err(|_| format!("`{image}` is no example, bundle directory or catalog image"))?;
        built_elsewhere(options, "an ESP-IDF catalog image")?;
        if let Some(reason) = &entry.hold {
            return Err(format!("`{image}` is held and not flashed: {reason}").into());
        }
        let build = oer_image::esp_idf::catalog::build(root, image)?;
        oer_image::esp_idf::catalog::bundle(
            root,
            entry,
            &build,
            &root.join("target/firmware/catalog").join(&entry.image),
        )?
    };
    let profile = Profile::load(root, &bundle.chip)?;
    let mut opened = found.open(&command_line(), device.wait)?;
    eprintln!(
        "fw: writing {} to {} ({})",
        bundle.directory.display(),
        opened.device().mac,
        opened.device().port.display()
    );
    opened.write(&bundle, image, &profile)?;
    eprintln!("fw: {} runs the image", opened.device().mac);
    if !monitor_after {
        return Ok(ExitCode::SUCCESS);
    }
    monitor(root, opened.console()?, &CaptureOptions::default())
}

/// Copy `console` to the terminal and a log until the capture ends, under
/// its device lock.
fn monitor(
    root: &Path,
    console: oer_devices::device::Console,
    capture: &CaptureOptions,
) -> Result<ExitCode> {
    let mac = console.device().mac.clone();
    let directory = root.join("target/firmware/console");
    std::fs::create_dir_all(&directory)?;
    let log = directory.join(format!(
        "{}-{}.log",
        mac.compact(),
        oer_durable::unix_millis()
    ));
    eprintln!("fw: console of {mac} (log {})", log.display());
    // Without a duration the capture lasts until the process is interrupted.
    let duration = capture
        .duration
        .unwrap_or(Duration::from_secs(u64::from(u32::MAX)));
    let seen = console.capture(duration, capture.until.as_deref(), &log)?;
    Ok(match (&capture.until, seen) {
        (Some(text), false) => {
            eprintln!("fw: `{text}` did not appear");
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    })
}

/// A duration such as `90s`, `2m` or `1h30m`; a bare number is seconds.
fn parse_duration(text: &str) -> std::result::Result<Duration, String> {
    let text = text.trim();
    if let Ok(seconds) = text.parse::<u64>() {
        return Ok(Duration::from_secs(seconds));
    }
    let mut total = 0_u64;
    let mut number = String::new();
    for character in text.chars() {
        if character.is_ascii_digit() {
            number.push(character);
            continue;
        }
        let unit = match character {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return Err(format!("invalid duration `{text}`; use e.g. 30s, 2m or 1h")),
        };
        let value: u64 = number
            .parse()
            .map_err(|_| format!("invalid duration `{text}`"))?;
        total += value * unit;
        number.clear();
    }
    if !number.is_empty() || total == 0 {
        return Err(format!("invalid duration `{text}`; use e.g. 30s, 2m or 1h"));
    }
    Ok(Duration::from_secs(total))
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    use super::*;

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_type_check_refuses_image_checks() {
        let parse = |arguments: &[&str]| {
            Cli::try_parse_from(["cargo fw", "build", "station"].iter().chain(arguments))
        };
        assert!(parse(&["--type-check"]).is_ok());
        assert!(parse(&["--check", "stack"]).is_ok());
        let error = parse(&["--type-check", "--check", "stack"]).err().unwrap();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }
}
