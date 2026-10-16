//! A tier's plan: its image segments, their lease limits and the check
//! against the tier's budget, all before anything is built or leased.
//!
//! A segment is one image on one board lane. Every segment takes its own
//! lease, whose hard limit is the segment's estimate times a margin, capped
//! by the stand's maximum (#761 Q4). A stage gate therefore never needs one
//! lease of the whole hour, and the arbiter interleaves other work between
//! segments. Segments are summed as if serial; parallel lanes come with the
//! lane-isolation result.

use core::fmt;

use oer_hil_model::{Estimate, Factor, TargetCapabilities, Tier};

/// One scenario instance in the plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Planned {
    /// `scenario/instance`.
    pub name: String,
    pub image: TargetCapabilities,
    pub estimate: Estimate,
}

/// What an image and a scenario cost beyond the scenario's own estimate,
/// from the stand profile's measured spans.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageCosts {
    /// Building the image when no build of its recipe is cached.
    pub build_micros: u64,
    /// Writing the image to a board, once per segment.
    pub flash_micros: u64,
    /// Reset to Hello, once per scenario instance.
    pub boot_micros: u64,
}

/// How a segment's lease limit derives from its estimate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeasePolicy {
    pub margin: Factor,
    /// The stand's longest lease.
    pub cap_micros: u64,
}

/// One image on one board lane, under one lease.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    pub image: TargetCapabilities,
    pub scenarios: Vec<String>,
    /// Whether this segment builds the image; later segments of the same
    /// image reuse the build.
    pub builds: bool,
    pub estimate_micros: u64,
    pub lease_limit_micros: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub tier: Tier,
    pub segments: Vec<Segment>,
    pub total_micros: u64,
}

/// Why a tier cannot be planned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanError {
    /// The plan exceeds the tier's budget; the largest scenarios first.
    OverBudget {
        budget_micros: u64,
        total_micros: u64,
        largest: Vec<(String, u64)>,
    },
    /// One scenario alone does not fit in the longest lease.
    Unsplittable {
        scenario: String,
        lease_micros: u64,
        cap_micros: u64,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OverBudget {
                budget_micros,
                total_micros,
                largest,
            } => {
                write!(
                    f,
                    "the plan takes {} s of a {} s budget; largest:",
                    total_micros / 1_000_000,
                    budget_micros / 1_000_000
                )?;
                for (name, micros) in largest {
                    write!(f, " {name} ({} s)", micros / 1_000_000)?;
                }
                Ok(())
            }
            Self::Unsplittable {
                scenario,
                lease_micros,
                cap_micros,
            } => write!(
                f,
                "{scenario} needs a {} s lease; the stand's longest is {} s",
                lease_micros / 1_000_000,
                cap_micros / 1_000_000
            ),
        }
    }
}

impl std::error::Error for PlanError {}

/// How many of the largest scenarios an over-budget refusal names.
const NAMED: usize = 3;

/// Plans `scenarios` for `tier`: one image per composition (the union of
/// its scenarios' capabilities; a scenario that needs no composition joins
/// the first image), each split into segments whose lease fits the cap.
pub fn plan(
    tier: Tier,
    scenarios: &[Planned],
    costs: ImageCosts,
    lease: LeasePolicy,
) -> Result<Plan, PlanError> {
    let scenario_micros = |planned: &Planned| {
        costs
            .boot_micros
            .saturating_add(planned.estimate.total_micros())
    };

    // Images in first-use order: a scenario joins the first image whose
    // composition it shares, or either side needs none.
    let mut images: Vec<(TargetCapabilities, Vec<&Planned>)> = Vec::new();
    for planned in scenarios {
        let joined = images.iter_mut().find_map(|(image, members)| {
            let union = image.union(&planned.image).ok()?;
            *image = union;
            members.push(planned);
            Some(())
        });
        if joined.is_none() {
            images.push((planned.image.clone(), vec![planned]));
        }
    }

    let mut segments = Vec::new();
    for (image, members) in images {
        let mut builds = true;
        let mut current: Vec<String> = Vec::new();
        let mut current_micros = 0_u64;
        let base = |builds: bool| {
            costs
                .flash_micros
                .saturating_add(if builds { costs.build_micros } else { 0 })
        };
        for planned in members {
            let own = scenario_micros(planned);
            let alone = lease.margin.apply(base(builds).saturating_add(own));
            if lease.margin.apply(base(false).saturating_add(own)) > lease.cap_micros {
                return Err(PlanError::Unsplittable {
                    scenario: planned.name.clone(),
                    lease_micros: alone,
                    cap_micros: lease.cap_micros,
                });
            }
            let with = base(builds)
                .saturating_add(current_micros)
                .saturating_add(own);
            if !current.is_empty() && lease.margin.apply(with) > lease.cap_micros {
                let estimate = base(builds).saturating_add(current_micros);
                segments.push(Segment {
                    image: image.clone(),
                    scenarios: core::mem::take(&mut current),
                    builds,
                    estimate_micros: estimate,
                    lease_limit_micros: lease.margin.apply(estimate).min(lease.cap_micros),
                });
                builds = false;
                current_micros = 0;
            }
            current.push(planned.name.clone());
            current_micros = current_micros.saturating_add(own);
        }
        if !current.is_empty() {
            let estimate = base(builds).saturating_add(current_micros);
            segments.push(Segment {
                image: image.clone(),
                scenarios: current,
                builds,
                estimate_micros: estimate,
                lease_limit_micros: lease.margin.apply(estimate).min(lease.cap_micros),
            });
        }
    }

    let total_micros = segments.iter().fold(0_u64, |total, segment| {
        total.saturating_add(segment.estimate_micros)
    });
    if let Some(budget_micros) = tier.budget_micros()
        && total_micros > budget_micros
    {
        let mut largest: Vec<(String, u64)> = scenarios
            .iter()
            .map(|planned| (planned.name.clone(), scenario_micros(planned)))
            .collect();
        largest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        largest.truncate(NAMED);
        return Err(PlanError::OverBudget {
            budget_micros,
            total_micros,
            largest,
        });
    }
    Ok(Plan {
        tier,
        segments,
        total_micros,
    })
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU32;

    use super::*;

    const SECOND: u64 = 1_000_000;
    const STATION: &str = "oer-esp32s31-ieee80211-system";

    fn planned(name: &str, composition: Option<&str>, key: &str, seconds: u64) -> Planned {
        Planned {
            name: name.into(),
            image: TargetCapabilities::of(composition, &[key]),
            estimate: Estimate {
                setup_micros: 0,
                repetition_micros: seconds * SECOND,
                repetitions: 1,
            },
        }
    }

    fn costs() -> ImageCosts {
        ImageCosts {
            build_micros: 50 * SECOND,
            flash_micros: 25 * SECOND,
            boot_micros: 2 * SECOND,
        }
    }

    fn lease(cap_seconds: u64) -> LeasePolicy {
        LeasePolicy {
            margin: Factor::new(3, NonZeroU32::new(2).unwrap()).unwrap(),
            cap_micros: cap_seconds * SECOND,
        }
    }

    #[test]
    fn smoke_is_one_image_built_and_flashed_once() {
        let scenarios = [
            planned("boot-smoke/default", None, "base/boot", 10),
            planned(
                "lower-mac-tx-ownership/one-frame",
                Some(STATION),
                "conformance/lower-mac",
                20,
            ),
        ];
        let plan = plan(Tier::Smoke, &scenarios, costs(), lease(3_600)).unwrap();
        assert_eq!(plan.segments.len(), 1);
        let segment = &plan.segments[0];
        assert_eq!(segment.image.capabilities.len(), 2);
        assert_eq!(segment.estimate_micros, (50 + 25 + 12 + 22) * SECOND);
        assert_eq!(segment.lease_limit_micros, segment.estimate_micros * 3 / 2);
    }

    #[test]
    fn a_long_gate_splits_into_leases_that_fit_the_cap_and_build_once() {
        let scenarios: Vec<_> = (0..6)
            .map(|i| {
                planned(
                    &format!("interop/{i}"),
                    Some(STATION),
                    "interop/station",
                    598,
                )
            })
            .collect();
        let plan = plan(Tier::Full, &scenarios, costs(), lease(1_200)).unwrap();
        assert!(plan.segments.len() > 1);
        assert!(
            plan.segments
                .iter()
                .all(|segment| segment.lease_limit_micros <= 1_200 * SECOND)
        );
        assert_eq!(
            plan.segments
                .iter()
                .filter(|segment| segment.builds)
                .count(),
            1
        );
        assert_eq!(
            plan.segments
                .iter()
                .map(|s| s.scenarios.len())
                .sum::<usize>(),
            6
        );
    }

    #[test]
    fn an_over_budget_plan_names_its_largest_scenarios() {
        let scenarios = [
            planned("a/x", Some(STATION), "k", 100),
            planned("b/x", Some(STATION), "k", 200),
            planned("c/x", Some(STATION), "k", 50),
            planned("d/x", Some(STATION), "k", 10),
        ];
        let Err(PlanError::OverBudget { largest, .. }) =
            plan(Tier::Smoke, &scenarios, costs(), lease(3_600))
        else {
            panic!("over budget");
        };
        let names: Vec<_> = largest.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["b/x", "a/x", "c/x"]);
    }

    #[test]
    fn a_scenario_longer_than_any_lease_is_refused() {
        let scenarios = [planned("soak/x", Some(STATION), "k", 7_200)];
        assert!(matches!(
            plan(Tier::Nightly, &scenarios, costs(), lease(3_600)),
            Err(PlanError::Unsplittable { .. })
        ));
    }

    #[test]
    fn compositions_get_separate_images() {
        let scenarios = [
            planned("wifi/x", Some(STATION), "a", 10),
            planned("ble/x", Some("oer-esp32s31-bluetooth-system"), "b", 10),
        ];
        let plan = plan(Tier::Full, &scenarios, costs(), lease(3_600)).unwrap();
        assert_eq!(plan.segments.len(), 2);
        assert!(plan.segments.iter().all(|segment| segment.builds));
    }
}
