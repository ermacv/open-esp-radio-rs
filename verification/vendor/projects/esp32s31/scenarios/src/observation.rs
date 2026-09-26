//! Production PHY source lines the compared observations depend on.
//!
//! Blobray reports which executed production instructions a compared
//! observation depends on. Debug line information of the probe ELF maps them
//! to production hardware source lines (PHY, HAL, PAC and MAC driver crates),
//! including inlined frames: a line is
//! observed when any of its executed instructions is. Every executed but
//! unobserved line is either reviewed by a decision below, with its reason, or
//! reported as untriaged in the evidence index. A decision that matches no
//! unobserved line fails the run, so the table cannot outlive the code it
//! describes.
use crate::harness::{Result, invalid};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Production ESP32-S31 hardware sources, relative to the repository root.
pub const SCOPE: &str = "crates/hardware/esp32s31";

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
        reason: "status-four transmit-error classification, compared through the stop goal: \
            each dispatch case completes only at its reviewed retry leaf, which dependence on \
            compared effects does not record",
        places: &[
            (
                "driver/ieee80211/mac/src/tx.rs",
                "4 => match self.detail() {",
            ),
            (
                "driver/ieee80211/mac/src/tx.rs",
                "1 | 3..=5 => TxCompletionDisposition::Collision,",
            ),
        ],
    },
    Decision {
        reason: "claim of the validation-only radio owner capability in the isolated probe \
            image, which carries no data the leaf reads; the leaf's register effects and return \
            compare",
        places: &[("hal/src/validation.rs", "owner()")],
    },
    Decision {
        reason: "device-ordering fences the MAC and power event clears and the ordinary \
            transmit publication add around their register edge: each reviewed contract \
            requires exactly that many, counted but not paired with a vendor effect",
        places: &[
            (
                "pac/raw/src/lib.rs",
                "core::arch::asm!(\"fence iorw, iorw\")",
            ),
            ("pac/src/ownership.rs", "svd::device_access::fence();"),
            ("pac/src/wifi/mac/interrupt.rs", "device_fence();"),
            ("pac/src/wifi/mac/tx.rs", "device_fence();"),
        ],
    },
    Decision {
        reason: "restore-record copy of the captured TX-DC power-detector calibration field: \
            the optimized probe restores the field from the captured register word it keeps in \
            a register, whose restore write is compared, and never reads the record copy back",
        places: &[
            (
                "pac/raw/src/lib.rs",
                "CalibrationFieldUnknownR::new(((self.bits >> 4) & 0xff) as u8)",
            ),
            ("pac/raw/src/lib.rs", ".calibration_field_unknown()"),
        ],
    },
    Decision {
        reason: "construction of the validation-only shared-PHY wrapper: its fresh route \
            state is a constant the optimized probes fold into each restore-slot check, so no \
            store of it is read",
        places: &[("hal/src/owner.rs", "Self {")],
    },
    Decision {
        reason: "execution statistics of the RX-gain and TX-DC executors; they \
            report effort to production diagnostics and have no vendor counterpart",
        places: &[
            (
                "phy/src/rx/gain_calibration.rs",
                "stats.minimum_operations += completion.operations();",
            ),
            (
                "phy/src/rx/gain_calibration.rs",
                "stats.settle_1us += 2 * u32::from(completion.estimators());",
            ),
            (
                "phy/src/target_port/calibration.rs",
                "execution.minimum_searches += 1;",
            ),
            (
                "phy/src/target_port/calibration.rs",
                "execution.outer_operations += 1;",
            ),
            (
                "phy/src/target_port/calibration.rs",
                "execution.minimum_searches += stats.minimum_searches;",
            ),
            (
                "phy/src/target_port/calibration.rs",
                "execution.minimum_operations += stats.minimum_operations;",
            ),
            (
                "phy/src/target_port/calibration.rs",
                "execution.settle_1us += stats.settle_1us;",
            ),
            (
                "phy/src/target_port/calibration.rs",
                "execution.settle_10us += 1;",
            ),
        ],
    },
    Decision {
        reason: "failure payload: the measurement, observation count or force-test \
            transaction a timed-out step reports; every compared case completes",
        places: &[
            (
                "phy/src/target_port/calibration.rs",
                "measurement: measurement_base + 1,",
            ),
            (
                "phy/src/target_port/calibration.rs",
                "PhyPbusForceTest::new(1, 1, shared_control),",
            ),
            ("phy/src/tx/dc_power_detector.rs", "let measurement = self"),
            (
                "phy/src/tx/dc_power_detector.rs",
                ".wrapping_add(self.total_measurements);",
            ),
            (
                "phy/src/tx/dc_power_detector.rs",
                "observations = observations.wrapping_add(1);",
            ),
            ("phy/src/tx/dc_power_detector.rs", "for transaction in ["),
            (
                "phy/src/tx/dc_power_detector.rs",
                "PhyPbusForceTest::new(4, 2, u16::from(tx_path_value) << 3),",
            ),
        ],
    },
    Decision {
        reason: "spills and reloads of capability references (register access, radio \
            channel, platform, observer) that the probes bind to zero-sized or no-op \
            implementations; the register effects themselves are compared",
        places: &[
            (
                "phy/src/target_port.rs",
                "registers: &mut impl SharedPhyAccess,",
            ),
            (
                "phy/src/target_port.rs",
                "channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,",
            ),
            ("phy/src/target_port.rs", "observer: &mut O,"),
            ("phy/src/target_port.rs", "platform: &mut P,"),
            ("phy/src/target_port.rs", "&'port mut self,"),
            ("phy/src/target_port.rs", "&mut self.observer,"),
            ("phy/src/target_port.rs", "self.platform,"),
            ("phy/src/target_port.rs", "self.registers,"),
            (
                "phy/src/target_port/rfpll.rs",
                "registers: &mut impl SharedPhyAccess,",
            ),
            (
                "phy/src/target_port/temperature.rs",
                "registers: &mut impl SharedPhyAccess,",
            ),
            (
                "phy/src/validation.rs",
                "channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,",
            ),
            ("phy/src/validation.rs", "observer: &mut O,"),
            (
                "phy/src/validation.rs",
                "crate::target_port::select_phy_channel_with_hal::<D, _, _>(",
            ),
            (
                "phy/src/target_port.rs",
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
                "phy/src/tracking/calibration.rs",
                "channel: self.parameters.current_channel,",
            ),
            (
                "phy/src/tracking/calibration.rs",
                "cbw: self.parameters.channel_bandwidth,",
            ),
            (
                "phy/src/tracking/calibration.rs",
                "class: self.active_class,",
            ),
            (
                "phy/src/tracking/calibration.rs",
                "let current = self.parameters.current_temperature;",
            ),
            (
                "phy/src/tracking/calibration.rs",
                "clients: self.request.clients,",
            ),
            (
                "phy/src/tracking/calibration.rs",
                "threshold: self.threshold,",
            ),
            ("phy/src/tracking/calibration.rs", "self.dcode"),
            ("phy/src/tracking/calibration.rs", "self.channel"),
            ("phy/src/tracking/calibration.rs", "self.tx_dc_pwdet[0]"),
        ],
    },
    Decision {
        reason: "async future bookkeeping attributed to signatures and closing braces: \
            state-discriminant and local stores into the future frame, register restores, and \
            the ready-flag take of a completed delay future at its `.await`; the probes poll \
            each future to completion without suspension, so no resume reads them",
        places: &[
            ("phy/src/target_port.rs", "}"),
            ("phy/src/target_port.rs", ".await;"),
            ("phy/src/target_port/rfpll.rs", "}"),
            (
                "phy/src/target_port/temperature.rs",
                ") -> Result<Result<PhyTemperatureOutcome, PhyTemperatureFailure>, PhyTargetPortError> {",
            ),
        ],
    },
    Decision {
        reason: "probe snapshot artifact: the tracking probes read the RX-gain memory \
            parameters to export their DC banks and drop `parameter_002`, which the RX-gain \
            scenario compares where calibration uses it",
        places: &[(
            "phy/src/state.rs",
            "parameter_002: self.config.pbus_rx_path,",
        )],
    },
    Decision {
        reason: "diagnostic-print selectors retained with the TX-power child's parent action: \
            commit reads only its variant and `enabled`, and the vendor diagnostics select \
            only console output, which production does not emit",
        places: &[("phy/src/tracking/parameters.rs", "Ok(Self {")],
    },
    Decision {
        reason: "measurement identity of the event-driven RX-DC host model: the ROM search \
            has no corresponding field and the direct target transaction never reads it",
        places: &[
            (
                "phy/src/rx/gain_calibration.rs",
                "measurement: iteration.wrapping_mul(2).wrapping_add(high as u8),",
            ),
            ("phy/src/rx/gain_calibration.rs", "policy.iteration,"),
        ],
    },
    Decision {
        reason: "state stores the parent probe seeds and the optimized probe forwards in a \
            register to the tracking policy it builds next; each flag's decision (the \
            Bluetooth/802.15.4 TX-power update, the relaxed Wi-Fi TX-power threshold) is \
            observed under both flag values",
        places: &[
            ("phy/src/state.rs", "self.bluetooth.power_tracking = value;"),
            (
                "phy/src/state.rs",
                "state.wifi.tx_power_tracking_slow = relaxed_threshold.into();",
            ),
        ],
    },
    Decision {
        reason: "client ownership bits of the validation-only pending parent fixture; the \
            compared parent transition never reads them",
        places: &[
            (
                "phy/src/state/client.rs",
                "owner.bits = if request.wifi() { WIFI_BIT } else { 0 }",
            ),
            (
                "phy/src/state/client.rs",
                "| if request.bluetooth_ieee802154() {",
            ),
            (
                "phy/src/state/client.rs",
                "owner.tracker_model_armed = owner.bits != 0;",
            ),
        ],
    },
    Decision {
        reason: "initial fields of freshly built tracking children that are written before \
            any read: the calibration child reads each `None` outcome field only after the \
            branch that sets it, and the recalibrated RX-gain child overwrites its initial \
            table results before reading them",
        places: &[
            ("phy/src/target_port.rs", "transition"),
            ("phy/src/target_port.rs", "let mut child = pending"),
        ],
    },
    Decision {
        reason: "temperature acquisition provenance, a production scheduling record with no \
            vendor counterpart; the temperature and sensor index are compared",
        places: &[(
            "phy/src/tracking/temperature.rs",
            "Acquisition::Undated => Self {",
        )],
    },
    Decision {
        reason: "RFPLL correction report: the search and frequency-memory outcome reach only \
            the observer and the maintenance result, the counterpart of the vendor's optional \
            diagnostic `phy_printf`; state keeps only whether a correction happened, and the \
            capacitor and frequency-memory writes are compared",
        places: &[
            (
                "phy/src/analog/frequency.rs",
                "PhyFrequencyCapMemoryAction::Complete(PhyFrequencyCapMemoryOutcome {",
            ),
            (
                "phy/src/analog/frequency.rs",
                "correction: self.request.correction,",
            ),
            ("phy/src/tracking/rfpll.rs", "search: *search,"),
            ("phy/src/tracking/rfpll/thermal.rs", "self.request"),
            ("phy/src/tracking/parameters.rs", "self.child.request()"),
            ("phy/src/target_port.rs", "let request = child.request();"),
            (
                "phy/src/target_port/rfpll.rs",
                ") -> Result<rfpll::Outcome, PhyTargetPortError> {",
            ),
            (
                "phy/src/target_port/rfpll.rs",
                ".ok_or(PhyTargetPortError::UnexpectedBinding)",
            ),
        ],
    },
    Decision {
        reason: "initial correction of the event-driven RX-DC host model; the target \
            runs the calibration as one direct transaction starting from the request",
        places: &[(
            "phy/src/rx/gain_calibration.rs",
            "current: request.initial,",
        )],
    },
];

/// Executed and observed production hardware lines.
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

/// Maps probe instructions to production hardware source lines.
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
    const PLACE: &str = "phy/src/example.rs";

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
