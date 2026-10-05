//! Build HIL firmware image classes the way `cargo hil image build` does,
//! with the link-time stack, placement and application audits, and report
//! every class's outcome instead of stopping at the first failure. A built
//! final image (`performance`, `correctness`) is also audited by Blobray:
//! its runtime must make no call into the vendor radio ROM.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};

use oer_hil_image_class::ImageClass;
use oer_hil_source_snapshot::FrozenSources;

use crate::Result;
use oer_process::Checkout;

/// How far each class is taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Depth {
    /// The full image build and its audits.
    Build,
    /// A `cargo check` of the runtime with the class's features only.
    TypeCheck,
}

/// One class's result.
#[derive(Debug)]
pub struct Outcome {
    pub class: ImageClass,
    pub elapsed: Duration,
    pub failure: Option<String>,
    /// The ROM summaries the class's image applied.
    pub summaries: BTreeSet<String>,
    /// The built runtime ELF.
    pub runtime_elf: Option<PathBuf>,
}

/// The classes whose built image Blobray audits for calls into the vendor
/// radio ROM: the images a product ships.
const FINAL: [ImageClass; 2] = [ImageClass::Performance, ImageClass::Correctness];

/// The vendor radio ROM ranges no final image may call into.
const FORBIDDEN: [&str; 2] = [
    "esp32s31-eco0-radio-api=0x2f800bf0..0x2f8016bc",
    "esp32s31-eco0-radio-body=0x2f823c12..0x2f83e6d0",
];

/// Every class with the runtime features its image builds with, and how to
/// type-check one.
pub fn list() -> String {
    let mut text = String::new();
    for class in ImageClass::ALL {
        text.push_str(&format!(
            "{:<36} {}\n",
            class.id(),
            class.build_features(oer_hil_image_class::NETWORK_FEATURE)
        ));
    }
    text.push_str(
        "type-check one class: cargo xtask check firmware --class <class> --type-check\n",
    );
    text
}

/// The classes to take: `selected`, or every class when it is empty, in
/// [`ImageClass::ALL`] order without repeats.
pub fn classes(selected: &[ImageClass]) -> Vec<ImageClass> {
    ImageClass::ALL
        .into_iter()
        .filter(|class| selected.is_empty() || selected.contains(class))
        .collect()
}

/// Classes built at once when `--jobs` is not given. Once the seed classes
/// built the shared dependencies, most of a class's build is its final
/// crate's fat LTO, which runs on one core and needs about 1.5 GB; half the
/// cores, at most eight, keeps both the cores and the memory busy.
pub fn default_jobs() -> usize {
    std::thread::available_parallelism().map_or(1, |cores| (cores.get() / 2).clamp(1, 8))
}

/// One class of each dependency family, built first. Between them they
/// compile every dependency the other classes share: Wi-Fi with Bluetooth,
/// and IEEE 802.15.4 with OpenThread.
const SEEDS: [ImageClass; 2] = [
    ImageClass::WifiBleCoex,
    ImageClass::DiagnosticIeee802154Thread,
];

/// The seed whose compiled units `class` starts from: an IEEE 802.15.4
/// class the IEEE 802.15.4 seed, every other class the Wi-Fi with Bluetooth
/// seed.
fn seed_of(class: ImageClass) -> ImageClass {
    if class.id().contains("ieee802154") {
        ImageClass::DiagnosticIeee802154Thread
    } else {
        ImageClass::WifiBleCoex
    }
}

/// The classes whose image a change of `changed` (repository-relative
/// paths) can alter or whose audits it can change: those whose last build in
/// the host build root read a changed file, as its `source-inputs.json`
/// records (sources, workspace and Cargo configuration, policies and the
/// image builder itself), and every class that has no complete record yet.
pub fn affected(changed: &[PathBuf]) -> Result<Vec<ImageClass>> {
    Ok(affected_in(
        &oer_hil_image::chip_build_root("esp32s31")?,
        changed,
    ))
}

fn affected_in(builds: &std::path::Path, changed: &[PathBuf]) -> Vec<ImageClass> {
    ImageClass::ALL
        .into_iter()
        .filter(|class| match last_inputs(builds, *class) {
            Some(inputs) => changed.iter().any(|path| inputs.contains(path)),
            None => true,
        })
        .collect()
}

/// Every input the newest build of `class` in the host build root recorded,
/// or `None` when it has none or an older record that listed only the
/// compiled sources.
fn last_inputs(
    builds: &std::path::Path,
    class: ImageClass,
) -> Option<std::collections::BTreeSet<PathBuf>> {
    #[derive(serde::Deserialize)]
    struct SourceInputs {
        schema: u32,
        files: Vec<PathBuf>,
    }
    let name = format!("{}-{}", class.id(), oer_hil_image_class::NETWORK);
    let newest = std::fs::read_dir(builds.join("snapshot-builds"))
        .ok()?
        .flatten()
        .map(|snapshot| snapshot.path().join(&name).join("source-inputs.json"))
        .filter_map(|path| Some((std::fs::metadata(&path).ok()?.modified().ok()?, path)))
        .max()?;
    let inputs: SourceInputs = serde_json::from_slice(&std::fs::read(newest.1).ok()?).ok()?;
    (inputs.schema == oer_image::source_inputs::SCHEMA).then(|| inputs.files.into_iter().collect())
}

/// Classes waiting to build, and the outcomes of those that did.
#[derive(Default)]
struct Queue {
    ready: std::collections::VecDeque<(usize, ImageClass)>,
    /// Classes whose seed is still building.
    waiting: Vec<(usize, ImageClass)>,
    outcomes: Vec<(usize, Outcome)>,
}

/// Builds or type-checks every selected class, up to `jobs` at once, and
/// reports the outcomes in catalog order. Each class compiles in its own
/// target directory. A full build takes the selected `SEEDS` first; as
/// each finishes, the classes of its family start from a copy-on-write copy
/// of its compiled units, so a dependency is compiled once per family
/// instead of once per class.
pub fn run(ctx: &Checkout, selected: &[ImageClass], depth: Depth, jobs: usize) -> Result<()> {
    // A full build compiles a snapshot of this checkout in one of the host's
    // build slots, whose paths and caches every checkout shares.
    let frozen = match depth {
        Depth::Build => {
            let snapshot = oer_hil_source_snapshot::capture(
                &ctx.root,
                &[],
                true,
                &oer_hil_image::source_snapshot_store()?,
            )?;
            Some(FrozenSources::open_in_free_workspace(
                snapshot.directory(),
                &oer_hil_image::frozen::build_slots("esp32s31")?,
            )?)
        }
        Depth::TypeCheck => None,
    };
    let frozen = frozen.as_ref();
    let classes: Vec<(usize, ImageClass)> = classes(selected).into_iter().enumerate().collect();
    let total = classes.len();
    let seeds: Vec<ImageClass> = classes
        .iter()
        .map(|(_, class)| *class)
        .filter(|class| depth == Depth::Build && SEEDS.contains(class))
        .collect();
    let mut queue = Queue::default();
    for &(index, class) in &classes {
        if seeds.contains(&class) {
            queue.ready.push_front((index, class));
        } else if seeds.contains(&seed_of(class)) {
            queue.waiting.push((index, class));
        } else {
            queue.ready.push_back((index, class));
        }
    }
    let queue = std::sync::Mutex::new(queue);
    let changed = std::sync::Condvar::new();
    let lock = || {
        queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, total.max(1)) {
            scope.spawn(|| {
                loop {
                    let next = {
                        let mut state = lock();
                        loop {
                            if let Some(next) = state.ready.pop_front() {
                                break Some(next);
                            }
                            if state.waiting.is_empty() {
                                break None;
                            }
                            state = changed
                                .wait(state)
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                    };
                    let Some((index, class)) = next else {
                        break;
                    };
                    let outcome = build_one(ctx, frozen, class, index, total, depth);
                    let family = if seeds.contains(&class) {
                        let mut state = lock();
                        let (family, others) = std::mem::take(&mut state.waiting)
                            .into_iter()
                            .partition::<Vec<_>, _>(|(_, waiting)| seed_of(*waiting) == class);
                        state.waiting = others;
                        family
                    } else {
                        Vec::new()
                    };
                    // A seed whose audit failed still compiled its units, and
                    // Cargo uses a copied unit only where it is exactly the
                    // unit needed. A failed copy only leaves a class colder.
                    for (_, member) in &family {
                        if let Err(error) = frozen
                            .ok_or_else(|| "a type check shares no units".into())
                            .and_then(|frozen| share_units(frozen, class, *member))
                        {
                            println!("check firmware: {} starts cold: {error}", member.id());
                        }
                    }
                    let mut state = lock();
                    state.ready.extend(family);
                    state.outcomes.push((index, outcome));
                    changed.notify_all();
                }
            });
        }
    });
    let mut outcomes = queue
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .outcomes;
    outcomes.sort_by_key(|(index, _)| *index);
    let mut outcomes: Vec<Outcome> = outcomes.into_iter().map(|(_, outcome)| outcome).collect();
    if depth == Depth::Build {
        audit_final_images(&mut outcomes, |elf| final_image_audit(ctx, elf))?;
    }
    print!("{}", summary(&outcomes));
    verdict(&outcomes)?;
    // Only the whole catalog, built, shows which summaries no image uses.
    if selected.is_empty() && depth == Depth::Build {
        let reviewed = oer_image::interrupt_stack::rom_summaries(&ctx.root)?;
        stale_summaries(&reviewed, &outcomes)?;
    }
    Ok(())
}

/// Audit every built final image with `audit`; a failed audit fails its
/// class.
fn audit_final_images(
    outcomes: &mut [Outcome],
    mut audit: impl FnMut(&std::path::Path) -> Result<()>,
) -> Result<()> {
    for outcome in outcomes
        .iter_mut()
        .filter(|outcome| FINAL.contains(&outcome.class) && outcome.failure.is_none())
    {
        let elf = outcome
            .runtime_elf
            .clone()
            .ok_or("a built final image names its runtime ELF")?;
        if let Err(error) = audit(&elf) {
            outcome.failure = Some(format!("final radio target audit: {error}"));
        }
    }
    Ok(())
}

/// Blobray's target audit of the runtime ELF `runtime`, with the Blobray
/// host built first.
fn final_image_audit(ctx: &Checkout, runtime: &std::path::Path) -> Result<()> {
    let mut command = ctx.command(oer_toolchain::blobray::host(&ctx.root)?);
    command.args(["audit-targets", "--artifact"]).arg(runtime);
    for range in FORBIDDEN {
        command.args(["--forbid", range]);
    }
    oer_process::run_with_shutdown_grace(&mut command, Duration::from_secs(20))
}

/// Fails naming every reviewed ROM summary no class's image applied: a
/// summary of a ROM function no image reaches is stale.
fn stale_summaries(reviewed: &[String], outcomes: &[Outcome]) -> Result<()> {
    let applied: BTreeSet<&String> = outcomes
        .iter()
        .flat_map(|outcome| &outcome.summaries)
        .collect();
    let stale: Vec<&str> = reviewed
        .iter()
        .filter(|name| !applied.contains(name))
        .map(String::as_str)
        .collect();
    if stale.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "no image class reaches the ROM functions of these summaries; remove them: {}",
            stale.join(", ")
        )
        .into())
    }
}

/// Copies the compiled units of `seed`'s build caches into `class`'s, by
/// reflink where the filesystem supports it. Cargo names each unit by its
/// package, features and flags, so a copied unit is used only where it is
/// exactly the unit the class needs; the uplifted binaries are not copied.
fn share_units(frozen: &FrozenSources, seed: ImageClass, class: ImageClass) -> Result<()> {
    let from = oer_hil_image::frozen::compile_cache(frozen, seed);
    let to = oer_hil_image::frozen::compile_cache(frozen, class);
    for build in ["runtime", "bootstrap"] {
        let Ok(entries) = std::fs::read_dir(from.join(build)) else {
            continue;
        };
        // `release` for build scripts, `<target>/release` for the image.
        let mut profiles = vec![PathBuf::from("release")];
        for entry in entries.flatten() {
            if entry.path().join("release").is_dir() {
                profiles.push(PathBuf::from(entry.file_name()).join("release"));
            }
        }
        for profile in profiles {
            for units in ["deps", "build", ".fingerprint"] {
                let source = from.join(build).join(&profile).join(units);
                if !source.is_dir() {
                    continue;
                }
                let destination = to.join(build).join(&profile).join(units);
                std::fs::create_dir_all(&destination)?;
                let status = std::process::Command::new("cp")
                    .args(["-a", "--reflink=auto"])
                    .arg(source.join("."))
                    .arg(&destination)
                    .status()?;
                if !status.success() {
                    return Err(format!(
                        "copying {} into {} failed",
                        source.display(),
                        destination.display()
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}

/// Builds or type-checks `class`, the `index`th of `total`.
fn build_one(
    ctx: &Checkout,
    frozen: Option<&FrozenSources>,
    class: ImageClass,
    index: usize,
    total: usize,
    depth: Depth,
) -> Outcome {
    println!(
        "check firmware: [{}/{total}] {} {}",
        index + 1,
        match depth {
            Depth::Build => "building",
            Depth::TypeCheck => "type-checking",
        },
        class.id()
    );
    let started = Instant::now();
    let result = match depth {
        Depth::Build => match frozen {
            Some(frozen) => oer_hil_image::frozen::build(frozen, class, None, &Default::default())
                .map(|artifacts| {
                    (
                        artifacts.bundle.rom_summaries.clone(),
                        Some(artifacts.bundle.runtime_elf()),
                    )
                }),
            None => Err("a full build needs the frozen sources".into()),
        },

        Depth::TypeCheck => {
            oer_hil_image::check(&ctx.root, class).map(|()| (BTreeSet::new(), None))
        }
    };
    let (summaries, runtime_elf, failure) = match result {
        Ok((summaries, runtime_elf)) => (summaries, runtime_elf, None),
        Err(error) => (BTreeSet::new(), None, Some(error.to_string())),
    };
    let outcome = Outcome {
        class,
        elapsed: started.elapsed(),
        failure,
        summaries,
        runtime_elf,
    };
    println!(
        "check firmware: {} {} ({}s)",
        class.id(),
        if outcome.failure.is_some() {
            "failed"
        } else {
            "passed"
        },
        outcome.elapsed.as_secs()
    );
    outcome
}

pub fn summary(outcomes: &[Outcome]) -> String {
    let mut text = String::new();
    for outcome in outcomes {
        let status = if outcome.failure.is_some() {
            "FAIL"
        } else {
            "PASS"
        };
        text.push_str(&format!(
            "{status} {:<36} {:>5}s",
            outcome.class.id(),
            outcome.elapsed.as_secs()
        ));
        if let Some(failure) = &outcome.failure {
            let first = failure.lines().next().unwrap_or_default();
            text.push_str(&format!("  {first}"));
        }
        text.push('\n');
    }
    let failed = outcomes.iter().filter(|o| o.failure.is_some()).count();
    text.push_str(&format!(
        "check firmware: {} passed, {failed} failed\n",
        outcomes.len() - failed
    ));
    text
}

/// Fails naming every failed class.
pub fn verdict(outcomes: &[Outcome]) -> Result<()> {
    let failed = outcomes
        .iter()
        .filter(|outcome| outcome.failure.is_some())
        .map(|outcome| outcome.class.id())
        .collect::<Vec<_>>();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("HIL firmware failed for {}", failed.join(", ")).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_no_class_applied_is_stale() {
        let reviewed = ["memset".to_owned(), "memcpy".to_owned()];
        let mut first = outcome(ImageClass::SystemWatchdog, None);
        first.summaries.insert("memset".to_owned());
        let second = outcome(ImageClass::SystemPanicReset, None);
        let error = stale_summaries(&reviewed, &[first, second])
            .unwrap_err()
            .to_string();
        assert!(error.ends_with("remove them: memcpy"), "{error}");
        let mut both = outcome(ImageClass::SystemPanicReset, None);
        both.summaries
            .extend(["memset".to_owned(), "memcpy".to_owned()]);
        assert!(stale_summaries(&reviewed, &[both]).is_ok());
    }

    fn outcome(class: ImageClass, failure: Option<&str>) -> Outcome {
        Outcome {
            class,
            elapsed: Duration::from_secs(3),
            failure: failure.map(str::to_owned),
            summaries: BTreeSet::new(),
            runtime_elf: Some(PathBuf::from(format!("/{}/runtime.elf", class.id()))),
        }
    }

    #[test]
    fn only_built_final_images_are_audited_and_a_failed_audit_fails_its_class() {
        let mut outcomes = [
            outcome(ImageClass::Performance, None),
            outcome(ImageClass::Correctness, Some("link failed")),
            outcome(ImageClass::SystemWatchdog, None),
        ];
        let mut audited = Vec::new();
        audit_final_images(&mut outcomes, |elf| {
            audited.push(elf.to_owned());
            Err("calls the radio ROM".into())
        })
        .unwrap();
        assert_eq!(audited, [PathBuf::from("/performance/runtime.elf")]);
        assert!(
            outcomes[0]
                .failure
                .as_deref()
                .unwrap()
                .contains("calls the radio ROM")
        );
        assert_eq!(outcomes[1].failure.as_deref(), Some("link failed"));
        assert!(outcomes[2].failure.is_none());
    }

    #[test]
    fn a_change_affects_the_classes_whose_last_build_compiled_it() {
        let builds = tempfile::tempdir().unwrap();
        let record = |class: ImageClass, files: &[&str]| {
            let directory = builds.path().join("snapshot-builds/abc").join(format!(
                "{}-{}",
                class.id(),
                oer_hil_image_class::NETWORK
            ));
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("source-inputs.json"),
                serde_json::json!({
                    "schema": oer_image::source_inputs::SCHEMA,
                    "files": files,
                })
                .to_string(),
            )
            .unwrap();
        };
        for class in ImageClass::ALL {
            record(class, &["crates/common/src/lib.rs"]);
        }
        let [wifi, bluetooth] = [ImageClass::Performance, ImageClass::BluetoothGatt];
        record(
            wifi,
            &["crates/common/src/lib.rs", "crates/wifi/src/lib.rs"],
        );
        let changed = |paths: &[&str]| -> Vec<ImageClass> {
            affected_in(
                builds.path(),
                &paths.iter().map(PathBuf::from).collect::<Vec<_>>(),
            )
        };
        assert_eq!(changed(&["crates/wifi/src/lib.rs"]), [wifi]);
        assert!(changed(&["docs/guide.md"]).is_empty());
        assert_eq!(
            changed(&["crates/common/src/lib.rs"]).len(),
            ImageClass::ALL.len()
        );
        assert!(!changed(&["crates/wifi/src/lib.rs"]).contains(&bluetooth));
        // Configuration and builder files are inputs like any other: a
        // class is rebuilt when its record names one.
        record(
            wifi,
            &[
                "crates/wifi/src/lib.rs",
                "hil/targets/esp32s31/stack.toml",
                "tools/image/src/lib.rs",
            ],
        );
        assert_eq!(changed(&["tools/image/src/lib.rs"]), [wifi]);
        for unread in ["Cargo.lock", "hil/host/runner/src/main.rs", "docs/guide.md"] {
            assert!(changed(&[unread]).is_empty(), "{unread}");
        }
        // A record that lists only the compiled sources is incomplete.
        let old = builds.path().join("snapshot-builds/abc").join(format!(
            "{}-{}",
            wifi.id(),
            oer_hil_image_class::NETWORK
        ));
        std::fs::write(
            old.join("source-inputs.json"),
            serde_json::json!({"schema": 1, "files": ["crates/wifi/src/lib.rs"]}).to_string(),
        )
        .unwrap();
        assert_eq!(changed(&["docs/guide.md"]), [wifi]);
        std::fs::remove_dir_all(builds.path().join("snapshot-builds/abc").join(format!(
            "{}-{}",
            bluetooth.id(),
            oer_hil_image_class::NETWORK
        )))
        .unwrap();
        assert!(
            changed(&["docs/guide.md"]).contains(&bluetooth),
            "no record: build it"
        );
    }

    #[test]
    fn the_listing_names_every_class_with_its_features() {
        let listing = list();
        for class in ImageClass::ALL {
            let line = listing
                .lines()
                .find(|line| line.split_whitespace().next() == Some(class.id()))
                .unwrap_or_else(|| panic!("{} is not listed", class.id()));
            assert!(line.contains(class.runtime_features()), "{line}");
        }
        assert!(listing.contains("--type-check"));
    }

    #[test]
    fn no_selection_takes_every_class() {
        assert_eq!(classes(&[]), ImageClass::ALL.to_vec());
    }

    #[test]
    fn selection_keeps_catalog_order_without_repeats() {
        let first = ImageClass::ALL[0];
        let last = ImageClass::ALL[ImageClass::ALL.len() - 1];
        assert_eq!(classes(&[last, first, last]), vec![first, last]);
    }

    #[test]
    fn a_failure_does_not_hide_the_other_classes() {
        let [first, second, third] = [ImageClass::ALL[0], ImageClass::ALL[1], ImageClass::ALL[2]];
        let outcomes = [
            outcome(first, None),
            outcome(second, Some("link failed\nmore detail")),
            outcome(third, Some("stack audit")),
        ];
        let text = summary(&outcomes);
        assert!(text.contains(&format!("PASS {}", first.id())));
        assert!(text.contains("link failed"));
        assert!(!text.contains("more detail"));
        assert!(text.contains("1 passed, 2 failed"));
        assert_eq!(
            verdict(&outcomes).unwrap_err().to_string(),
            format!("HIL firmware failed for {}, {}", second.id(), third.id())
        );
    }

    #[test]
    fn all_passing_classes_pass() {
        assert!(verdict(&[outcome(ImageClass::ALL[0], None)]).is_ok());
    }
}
