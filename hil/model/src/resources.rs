//! What a scenario needs from the stand: boards by role and capability,
//! fixtures and its footprint on the air.

use core::fmt;

use serde::{Deserialize, Serialize};

/// What a board does in a scenario.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoardRole {
    /// The device under test.
    Dut,
    /// A peer: a reference board, an access-point client, a BLE central.
    Peer,
    /// An air observer.
    Observer,
}

/// The low-power clock source a board has.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LpClockSource {
    /// The internal RC oscillator, on every board.
    RcSlow,
    /// A 32 kHz crystal. ESP32-S31-Function-CoreBoard-1 has none (#563,
    /// decision 7).
    Xtal32k,
}

/// A property of a board a scenario may require.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case", tag = "capability", content = "value")]
pub enum BoardCapability {
    LpClock(LpClockSource),
    /// An antenna connector for conducted RF.
    Ufl,
}

/// One board a scenario needs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct BoardNeed {
    pub role: BoardRole,
    /// The chip identifier of `platform/<chip>/chip.toml`.
    pub chip: String,
    pub capabilities: Vec<BoardCapability>,
}

/// One board the stand offers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct BoardOffer {
    pub name: String,
    pub chip: String,
    pub roles: Vec<BoardRole>,
    pub capabilities: Vec<BoardCapability>,
}

/// The band a scenario's radio work occupies.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Band {
    Ghz2_4,
    Ghz5,
}

/// How much air a scenario uses.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Airtime {
    /// Occasional frames.
    Quiet,
    /// Ordinary protocol traffic.
    Normal,
    /// Continuous transmission: a carrier, DTM, a throughput flood.
    Flood,
}

/// Whether a scenario tolerates other transmitters in its range.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tolerance {
    /// No other transmitter may use the range: RF figures, performance.
    Strict,
    Tolerant,
}

/// A scenario's footprint on the stand's air, which the arbiter grants.
/// It is the stand's resource, distinct from the on-chip `AirClaim` of the
/// radio-system contract (#771) that a coexistence scenario takes as a
/// parameter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AirFootprint {
    pub band: Band,
    /// Centre frequencies in MHz.
    pub centres_mhz: Vec<u32>,
    pub bandwidth_mhz: u32,
    pub airtime: Airtime,
    pub tolerance: Tolerance,
}

/// Everything a scenario needs from the stand.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Resources {
    pub boards: Vec<BoardNeed>,
    /// Fixture names of the stand file (#335).
    pub fixtures: Vec<String>,
    /// `None` for work that does not use the air.
    pub air: Option<AirFootprint>,
}

/// Why the stand cannot run a scenario, found at plan time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Unmet {
    Board {
        role: BoardRole,
        chip: String,
        missing: Vec<BoardCapability>,
    },
    Fixture(String),
}

impl fmt::Display for Unmet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Board {
                role,
                chip,
                missing,
            } if missing.is_empty() => {
                write!(f, "no free {chip} board can take the {role:?} role")
            }
            Self::Board {
                role,
                chip,
                missing,
            } => {
                write!(f, "no {chip} board for the {role:?} role has {missing:?}")
            }
            Self::Fixture(name) => write!(f, "the stand has no fixture `{name}`"),
        }
    }
}

impl Resources {
    /// What the stand cannot provide. Needed boards are matched to distinct
    /// offered boards by a maximum bipartite matching, so a board with a
    /// rare capability is not spent on a need that does not require it.
    /// Empty when the stand can run the scenario.
    pub fn unmet(&self, boards: &[BoardOffer], fixtures: &[String]) -> Vec<Unmet> {
        let takes = |need: &BoardNeed, board: &BoardOffer| {
            board.chip == need.chip && board.roles.contains(&need.role)
        };
        let fits = |need: &BoardNeed, board: &BoardOffer| {
            takes(need, board)
                && need
                    .capabilities
                    .iter()
                    .all(|c| board.capabilities.contains(c))
        };
        // Kuhn's augmenting paths: `owner[board]` is the need holding it.
        let mut owner: Vec<Option<usize>> = vec![None; boards.len()];
        fn augment(
            need: usize,
            fits: &dyn Fn(usize, usize) -> bool,
            boards: usize,
            seen: &mut [bool],
            owner: &mut [Option<usize>],
        ) -> bool {
            for board in 0..boards {
                if seen[board] || !fits(need, board) {
                    continue;
                }
                seen[board] = true;
                if owner[board].is_none_or(|other| augment(other, fits, boards, seen, owner)) {
                    owner[board] = Some(need);
                    return true;
                }
            }
            false
        }
        let fit = |need: usize, board: usize| fits(&self.boards[need], &boards[board]);
        let mut unmet = Vec::new();
        for (index, need) in self.boards.iter().enumerate() {
            let mut seen = vec![false; boards.len()];
            if augment(index, &fit, boards.len(), &mut seen, &mut owner) {
                continue;
            }
            // Name what the board of this chip and role that lacks the
            // fewest capabilities lacks, or nothing when no board takes the
            // role at all.
            let missing = boards
                .iter()
                .filter(|board| takes(need, board))
                .map(|board| {
                    need.capabilities
                        .iter()
                        .filter(|c| !board.capabilities.contains(c))
                        .copied()
                        .collect::<Vec<_>>()
                })
                .min_by_key(Vec::len)
                .unwrap_or_default();
            unmet.push(Unmet::Board {
                role: need.role,
                chip: need.chip.clone(),
                missing,
            });
        }
        for fixture in &self.fixtures {
            if !fixtures.contains(fixture) {
                unmet.push(Unmet::Fixture(fixture.clone()));
            }
        }
        unmet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s31(name: &str, capabilities: Vec<BoardCapability>) -> BoardOffer {
        BoardOffer {
            name: name.into(),
            chip: "esp32s31".into(),
            roles: vec![BoardRole::Dut, BoardRole::Peer],
            capabilities,
        }
    }

    #[test]
    fn a_crystal_scenario_is_rejected_on_a_board_without_one() {
        let needs = Resources {
            boards: vec![BoardNeed {
                role: BoardRole::Dut,
                chip: "esp32s31".into(),
                capabilities: vec![BoardCapability::LpClock(LpClockSource::Xtal32k)],
            }],
            ..Resources::default()
        };
        let stand = [s31(
            "a",
            vec![BoardCapability::LpClock(LpClockSource::RcSlow)],
        )];
        assert_eq!(
            needs.unmet(&stand, &[]),
            vec![Unmet::Board {
                role: BoardRole::Dut,
                chip: "esp32s31".into(),
                missing: vec![BoardCapability::LpClock(LpClockSource::Xtal32k)],
            }]
        );
    }

    #[test]
    fn a_rare_capability_is_kept_for_the_need_that_requires_it() {
        let needs = Resources {
            boards: vec![
                BoardNeed {
                    role: BoardRole::Dut,
                    chip: "esp32s31".into(),
                    capabilities: Vec::new(),
                },
                BoardNeed {
                    role: BoardRole::Peer,
                    chip: "esp32s31".into(),
                    capabilities: vec![BoardCapability::Ufl],
                },
            ],
            ..Resources::default()
        };
        let stand = [s31("a", vec![BoardCapability::Ufl]), s31("b", Vec::new())];
        assert!(needs.unmet(&stand, &[]).is_empty());
    }

    #[test]
    fn an_unmet_need_names_what_the_closest_board_lacks() {
        let crystal = BoardCapability::LpClock(LpClockSource::Xtal32k);
        let needs = Resources {
            boards: vec![BoardNeed {
                role: BoardRole::Dut,
                chip: "esp32s31".into(),
                capabilities: vec![BoardCapability::Ufl, crystal],
            }],
            ..Resources::default()
        };
        let stand = [s31("a", Vec::new()), s31("b", vec![BoardCapability::Ufl])];
        assert_eq!(
            needs.unmet(&stand, &[]),
            vec![Unmet::Board {
                role: BoardRole::Dut,
                chip: "esp32s31".into(),
                missing: vec![crystal]
            }]
        );
    }

    #[test]
    fn each_needed_board_takes_a_distinct_offer() {
        let need = |role| BoardNeed {
            role,
            chip: "esp32s31".into(),
            capabilities: Vec::new(),
        };
        let needs = Resources {
            boards: vec![need(BoardRole::Dut), need(BoardRole::Peer)],
            fixtures: vec!["hostapd".into()],
            air: None,
        };
        let one = [s31("a", Vec::new())];
        assert_eq!(needs.unmet(&one, &["hostapd".into()]).len(), 1);
        let two = [s31("a", Vec::new()), s31("b", Vec::new())];
        assert!(needs.unmet(&two, &["hostapd".into()]).is_empty());
        assert_eq!(
            needs.unmet(&two, &[]),
            vec![Unmet::Fixture("hostapd".into())]
        );
    }
}
