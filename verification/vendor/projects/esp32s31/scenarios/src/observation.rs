//! Production PHY source lines the compared observations depend on.
//!
//! Blobray reports which executed production instructions a compared
//! observation depends on. Debug line information of the probe ELF maps them
//! to production PHY source lines, including inlined frames: a line is
//! observed when any of its executed instructions is. Every executed but
//! unobserved line is either reviewed by a decision below, with its reason, or
//! reported as untriaged in the evidence index. A decision that matches no
//! unobserved line fails the run, so the table cannot outlive the code it
//! describes.
use crate::harness::{Result, invalid};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Production PHY sources, relative to the repository root.
pub const SCOPE: &str = "crates/hardware/esp32s31/phy/src";

/// Repository root: this package lives five directories below it.
pub fn root() -> Result<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../..")
        .canonicalize()?)
}

/// A source line of one file, relative to the repository root.
pub type SourceLine = (PathBuf, u32);

/// A reviewed decision on executed lines no compared observation depends on.
/// Each place is a file below `SCOPE` and a line's trimmed text, so the
/// decision survives unrelated line shifts.
#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub reason: &'static str,
    pub places: &'static [(&'static str, &'static str)],
}

/// Reviewed unobserved lines.
pub const DECISIONS: &[Decision] = &[
    Decision {
        reason: "execution statistics of the RX-gain and TX-DC executors; they \
            report effort to production diagnostics and have no vendor counterpart",
        places: &[
            ("rx/gain_calibration.rs", "stats.minimum_searches += 1;"),
            (
                "rx/gain_calibration.rs",
                "stats.minimum_operations += completion.operations();",
            ),
            (
                "rx/gain_calibration.rs",
                "stats.settle_1us += 2 * u32::from(completion.estimators());",
            ),
            (
                "target_port/calibration.rs",
                "execution.minimum_searches += 1;",
            ),
            (
                "target_port/calibration.rs",
                "execution.minimum_operations += completion.operations();",
            ),
            (
                "target_port/calibration.rs",
                "execution.outer_operations += 1;",
            ),
            (
                "target_port/calibration.rs",
                "execution.minimum_searches += stats.minimum_searches;",
            ),
            (
                "target_port/calibration.rs",
                "execution.minimum_operations += stats.minimum_operations;",
            ),
            (
                "target_port/calibration.rs",
                "execution.settle_1us += stats.settle_1us;",
            ),
            ("target_port/calibration.rs", "execution.settle_10us += 1;"),
        ],
    },
    Decision {
        reason: "failure payload: the measurement, observation count or force-test \
            transaction a timed-out step reports; every compared case completes",
        places: &[
            (
                "target_port/calibration.rs",
                "measurement: measurement_base + 1,",
            ),
            (
                "target_port/calibration.rs",
                "PhyPbusForceTest::new(1, 1, shared_control),",
            ),
            ("tx/dc_power_detector.rs", "let measurement = self"),
            (
                "tx/dc_power_detector.rs",
                ".wrapping_add(self.total_measurements);",
            ),
            (
                "tx/dc_power_detector.rs",
                "observations = observations.wrapping_add(1);",
            ),
            ("tx/dc_power_detector.rs", "for transaction in ["),
            (
                "tx/dc_power_detector.rs",
                "PhyPbusForceTest::new(4, 2, u16::from(tx_path_value) << 3),",
            ),
        ],
    },
    Decision {
        reason: "spills and reloads of capability references (register access, radio \
            channel, platform, observer) that the probes bind to zero-sized or no-op \
            implementations; the register effects themselves are compared",
        places: &[
            ("target_port.rs", "registers: &mut impl SharedPhyAccess,"),
            (
                "target_port.rs",
                "channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,",
            ),
            ("target_port.rs", "observer: &mut O,"),
            ("target_port.rs", "platform: &mut P,"),
            ("target_port.rs", "&'port mut self,"),
            ("target_port.rs", "&mut self.observer,"),
            ("target_port.rs", "self.platform,"),
            ("target_port.rs", "self.registers,"),
            (
                "target_port/rfpll.rs",
                "registers: &mut impl SharedPhyAccess,",
            ),
            (
                "target_port/temperature.rs",
                "registers: &mut impl SharedPhyAccess,",
            ),
            (
                "validation.rs",
                "channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,",
            ),
            ("validation.rs", "observer: &mut O,"),
            (
                "validation.rs",
                "crate::target_port::select_phy_channel_with_hal::<D, _, _>(",
            ),
            (
                "target_port.rs",
                "TargetCompleter::<D>::select_channel_hal(state, channel_or_frequency, cbw, channel, observer)",
            ),
        ],
    },
    Decision {
        reason: "action payloads the calibration-tracking dispatch builds and discards: the \
            executor and target port call the out-of-line `action()` only for its variant, \
            `begin_*` lowers the action again and `commit` builds its own inlined outcome, \
            whose instructions are observed",
        places: &[
            (
                "tracking/calibration.rs",
                "channel: self.parameters.current_channel,",
            ),
            (
                "tracking/calibration.rs",
                "cbw: self.parameters.channel_bandwidth,",
            ),
            ("tracking/calibration.rs", "class: self.active_class,"),
            (
                "tracking/calibration.rs",
                "let current = self.parameters.current_temperature;",
            ),
            ("tracking/calibration.rs", "clients: self.request.clients,"),
            ("tracking/calibration.rs", "threshold: self.threshold,"),
            ("tracking/calibration.rs", "self.dcode"),
            ("tracking/calibration.rs", "self.channel"),
            ("tracking/calibration.rs", "self.tx_dc_pwdet[0]"),
        ],
    },
    Decision {
        reason: "async future bookkeeping attributed to signatures and closing braces: \
            state-discriminant and local stores into the future frame and register restores; \
            the probes poll each future to completion without suspension, so no resume reads \
            them",
        places: &[
            ("target_port.rs", "}"),
            ("target_port/rfpll.rs", "}"),
            (
                "target_port/temperature.rs",
                ") -> Result<Result<PhyTemperatureOutcome, PhyTemperatureFailure>, PhyTargetPortError> {",
            ),
        ],
    },
    Decision {
        reason: "probe snapshot artifact: the tracking probes read the RX-gain memory \
            parameters to export their DC banks and drop `parameter_002`, which the RX-gain \
            scenario compares where calibration uses it",
        places: &[("state.rs", "parameter_002: self.config.pbus_rx_path,")],
    },
    Decision {
        reason: "diagnostic-print selectors retained with the TX-power child's parent action: \
            commit reads only its variant and `enabled`, and the vendor diagnostics select \
            only console output, which production does not emit",
        places: &[("tracking/parameters.rs", "Ok(Self {")],
    },
    Decision {
        reason: "temperature acquisition provenance, a production scheduling record with no \
            vendor counterpart; the temperature and sensor index are compared",
        places: &[("tracking/temperature.rs", "Acquisition::Undated => Self {")],
    },
    Decision {
        reason: "RFPLL correction report: the search and frequency-memory outcome reach only \
            the observer and the maintenance result, the counterpart of the vendor's optional \
            diagnostic `phy_printf`; state keeps only whether a correction happened, and the \
            capacitor and frequency-memory writes are compared",
        places: &[
            (
                "analog/frequency.rs",
                "PhyFrequencyCapMemoryAction::Complete(PhyFrequencyCapMemoryOutcome {",
            ),
            (
                "analog/frequency.rs",
                "correction: self.request.correction,",
            ),
            ("tracking/rfpll.rs", "search: *search,"),
            ("tracking/rfpll/thermal.rs", "self.request"),
            ("tracking/parameters.rs", "self.child.request()"),
            ("target_port.rs", "let request = child.request();"),
            (
                "target_port/rfpll.rs",
                ") -> Result<rfpll::Outcome, PhyTargetPortError> {",
            ),
            (
                "target_port/rfpll.rs",
                ".ok_or(PhyTargetPortError::UnexpectedBinding)",
            ),
        ],
    },
    Decision {
        reason: "power sentinel of an incomplete RX-DC minimum search: the search is \
            incomplete only when its best power is at least 48, and every consumer \
            thresholds power at 45 (baseband corrections) or 46 (convergence), so the \
            sentinel 56 decides exactly as the best power it replaces",
        places: &[("rx/dc_offset.rs", "if !complete {")],
    },
    Decision {
        reason: "initial correction of the event-driven RX-DC host model; the target \
            runs the calibration as one direct transaction starting from the request",
        places: &[("rx/gain_calibration.rs", "current: request.initial,")],
    },
];

/// Executed and observed production PHY lines.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lines {
    pub executed: BTreeSet<SourceLine>,
    pub observed: BTreeSet<SourceLine>,
    /// Lines an emitted event depends on, compared or not.
    pub effect: BTreeSet<SourceLine>,
    /// Lines the memory state at a case end depends on.
    pub state: BTreeSet<SourceLine>,
}

impl Lines {
    pub fn extend(&mut self, other: &Lines) {
        self.executed.extend(other.executed.iter().cloned());
        self.observed.extend(other.observed.iter().cloned());
        self.effect.extend(other.effect.iter().cloned());
        self.state.extend(other.state.iter().cloned());
    }

    pub fn unobserved(&self) -> BTreeSet<SourceLine> {
        self.executed.difference(&self.observed).cloned().collect()
    }
}

/// Maps probe instructions to production PHY source lines.
pub struct LineMap {
    lines: BTreeMap<u32, Vec<SourceLine>>,
}

impl LineMap {
    /// The lines within `SCOPE` of every instruction in `pcs` of `elf`.
    pub fn new(elf: &[u8], pcs: &BTreeSet<u32>, root: &Path) -> Result<Self> {
        use object::{Object, ObjectSection};
        let file = object::File::parse(elf)?;
        let endian = gimli::RunTimeEndian::Little;
        let dwarf = gimli::Dwarf::load(|id| {
            let data = file
                .section_by_name(id.name())
                .and_then(|section| section.data().ok())
                .unwrap_or(&[]);
            Ok::<_, gimli::Error>(gimli::EndianSlice::new(data, endian))
        })?;
        let context = addr2line::Context::from_dwarf(dwarf)?;
        let root = root.canonicalize()?;
        let scope = root.join(SCOPE);
        let mut lines = BTreeMap::new();
        for pc in pcs {
            let mut located = vec![];
            let mut frames = context
                .find_frames(u64::from(*pc))
                .skip_all_loads()
                .map_err(|e| invalid(format!("debug information at {pc:#x}: {e}")))?;
            while let Some(frame) = frames
                .next()
                .map_err(|e| invalid(format!("debug information at {pc:#x}: {e}")))?
            {
                let Some(location) = frame.location else {
                    continue;
                };
                let (Some(path), Some(line)) = (location.file, location.line) else {
                    continue;
                };
                if let Ok(relative) = Path::new(path).strip_prefix(&scope) {
                    located.push((Path::new(SCOPE).join(relative), line));
                }
            }
            lines.insert(*pc, located);
        }
        Ok(Self { lines })
    }

    /// Lines of `executed` instructions, observed when any of `observed` is.
    pub fn lines(
        &self,
        instructions: &blobray_application::in_process::ObservedInstructions,
    ) -> Lines {
        let mut result = Lines::default();
        for pc in &instructions.executed {
            for line in self.lines.get(pc).into_iter().flatten() {
                result.executed.insert(line.clone());
                for (set, lines) in [
                    (&instructions.observed, &mut result.observed),
                    (&instructions.effect, &mut result.effect),
                    (&instructions.state, &mut result.state),
                ] {
                    if set.contains(pc) {
                        lines.insert(line.clone());
                    }
                }
            }
        }
        result
    }
}

/// Trimmed text of source lines, read once per file.
#[derive(Default)]
pub struct Sources {
    files: BTreeMap<PathBuf, Vec<String>>,
}

impl Sources {
    fn text(&mut self, root: &Path, (file, line): &SourceLine) -> Result<&str> {
        if !self.files.contains_key(file) {
            let text = std::fs::read_to_string(root.join(file))?;
            self.files.insert(
                file.clone(),
                text.lines().map(|l| l.trim().to_owned()).collect(),
            );
        }
        self.files[file]
            .get(*line as usize - 1)
            .map(String::as_str)
            .ok_or_else(|| invalid(format!("{}:{line} is outside its file", file.display())))
    }

    /// Split `unobserved` into (reviewed, untriaged) under `decisions`.
    pub fn classify(
        &mut self,
        root: &Path,
        decisions: &[Decision],
        unobserved: &BTreeSet<SourceLine>,
    ) -> Result<(BTreeSet<SourceLine>, BTreeSet<SourceLine>)> {
        let mut reviewed = BTreeSet::new();
        let mut untriaged = BTreeSet::new();
        for line in unobserved {
            let text = self.text(root, line)?;
            if decisions.iter().any(|d| {
                d.places
                    .iter()
                    .any(|(file, source)| line.0 == Path::new(SCOPE).join(file) && text == *source)
            }) {
                reviewed.insert(line.clone());
            } else {
                untriaged.insert(line.clone());
            }
        }
        Ok((reviewed, untriaged))
    }

    /// Every place of every decision must still match an unobserved line.
    pub fn check(
        &mut self,
        root: &Path,
        decisions: &[Decision],
        unobserved: &BTreeSet<SourceLine>,
    ) -> Result<()> {
        let mut matched = BTreeSet::new();
        for line in unobserved {
            let text = self.text(root, line)?.to_owned();
            for decision in decisions {
                for place in decision.places {
                    if line.0 == Path::new(SCOPE).join(place.0) && text == place.1 {
                        matched.insert(*place);
                    }
                }
            }
        }
        match decisions
            .iter()
            .flat_map(|d| d.places)
            .find(|place| !matched.contains(*place))
        {
            Some((file, source)) => Err(invalid(format!(
                "observation decision for `{source}` in {file} matches no unobserved line; \
                 the line is observed or no longer executed"
            ))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "crates/hardware/esp32s31/phy/src/example.rs";
    const PLACE: &str = "example.rs";

    fn root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join(FILE);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "fn f() {\n    let unused = 1;\n    write();\n}\n").unwrap();
        root
    }

    fn line(number: u32) -> SourceLine {
        (PathBuf::from(FILE), number)
    }

    const DECIDED: &[Decision] = &[Decision {
        reason: "test",
        places: &[(PLACE, "let unused = 1;")],
    }];

    #[test]
    fn decisions_review_their_lines_and_leave_the_rest_untriaged() {
        let root = root();
        let unobserved = BTreeSet::from([line(2), line(3)]);
        let (reviewed, untriaged) = Sources::default()
            .classify(root.path(), DECIDED, &unobserved)
            .unwrap();
        assert_eq!(reviewed, BTreeSet::from([line(2)]));
        assert_eq!(untriaged, BTreeSet::from([line(3)]));
    }

    #[test]
    fn a_decision_whose_line_is_observed_or_not_executed_is_stale() {
        let root = root();
        let mut sources = Sources::default();
        sources
            .check(root.path(), DECIDED, &BTreeSet::from([line(2)]))
            .unwrap();
        assert!(
            sources
                .check(root.path(), DECIDED, &BTreeSet::from([line(3)]))
                .is_err()
        );
        assert!(
            sources
                .check(root.path(), DECIDED, &BTreeSet::new())
                .is_err()
        );
    }

    #[test]
    fn a_line_is_observed_when_any_of_its_instructions_is() {
        let map = LineMap {
            lines: BTreeMap::from([(0x10, vec![line(2)]), (0x14, vec![line(2), line(3)])]),
        };
        let lines = map.lines(&blobray_application::in_process::ObservedInstructions {
            executed: BTreeSet::from([0x10, 0x14]),
            observed: BTreeSet::from([0x10]),
            ..Default::default()
        });
        assert_eq!(lines.executed, BTreeSet::from([line(2), line(3)]));
        assert_eq!(lines.observed, BTreeSet::from([line(2)]));
        assert_eq!(lines.unobserved(), BTreeSet::from([line(3)]));
    }
}
