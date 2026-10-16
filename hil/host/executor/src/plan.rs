//! A tier's plan: what the stand cannot run, the image segments, their
//! lease limits and the check against the tier's budget, all before
//! anything is built or leased.
//!
//! A segment is one image on one board lane. Every segment takes its own
//! lease, whose hard limit is the segment's estimate times a margin (#761
//! Q4). A segment closes before that limit would pass the stand's longest
//! lease, and a scenario that cannot fit in any lease is refused rather
//! than given a shorter margin. The image is built before its first
//! segment's lease, so builds count toward the tier's wall time only.
//! Segments are summed as if serial; parallel lanes come with the
//! lane-isolation result.
//!
//! An instance's bounds come from the stand profile when an image of the
//! same recipe reported them; for a new recipe they are its contract's
//! declared constants, and the plan labels every wait on them
//! provisional, as it labels provisional stand spans.

use core::fmt;

use oer_hil_model::{
    Bounds, Estimate, Factor, Instance, Overflow, Provisional, ReportedBounds, Resources,
    ScenarioInstance, Sha256, SpanName, Spec, StandOffer, StandProfile, Subject,
    TargetCapabilities, Tier, Unmet, WaitError, sum,
};

type EstimateFn = dyn Fn(&Bounds, &StandProfile) -> Result<Estimate, WaitError>;

/// One scenario instance in the plan.
pub struct Planned {
    pub id: ScenarioInstance,
    pub image: TargetCapabilities,
    pub resources: Resources,
    contract: &'static str,
    declared: ReportedBounds,
    estimate: Box<EstimateFn>,
}

impl Planned {
    pub fn of<S: Spec>(instance: &Instance<S>) -> Self
    where
        S::Params: 'static,
    {
        let params = instance.params.clone();
        Self {
            id: instance.id(),
            image: S::target(&instance.params),
            resources: S::resources(&instance.params),
            contract: <S::Subject as Subject>::CONTRACT,
            declared: <S::Subject as Subject>::declared_bounds(),
            estimate: Box::new(move |bounds, profile| S::estimate(&params, bounds, profile)),
        }
    }
}

/// What an image costs beyond its scenarios: the stand spans that measure
/// it.
#[derive(Clone, Debug)]
pub struct ImageCosts {
    /// Building the image when no build of its recipe is cached.
    pub build: SpanName,
    /// Writing the image to a board, once per segment.
    pub flash: SpanName,
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
    pub recipe: Sha256,
    pub scenarios: Vec<ScenarioInstance>,
    /// The build before this segment's lease: the image's first segment
    /// builds it, later segments reuse the build.
    pub build_micros: u64,
    /// What the lease holds: the flash and the scenarios.
    pub lease_estimate_micros: u64,
    /// The estimate times the margin, at most the stand's cap.
    pub lease_limit_micros: u64,
}

impl Segment {
    pub fn wall_micros(&self) -> Result<u64, Overflow> {
        sum([self.build_micros, self.lease_estimate_micros])
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub tier: Tier,
    pub segments: Vec<Segment>,
    pub total_micros: u64,
    /// Every provisional value the plan used.
    pub provisional: Vec<Provisional>,
}

/// Why a tier cannot be planned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanError {
    /// The stand lacks boards, board capabilities or fixtures.
    Refused(Vec<(ScenarioInstance, Vec<Unmet>)>),
    /// A wait has no length; `None` for the image costs.
    Wait {
        scenario: Option<ScenarioInstance>,
        error: WaitError,
    },
    Overflow,
    /// The plan exceeds the tier's budget; the largest scenarios first.
    OverBudget {
        budget_micros: u64,
        total_micros: u64,
        largest: Vec<(ScenarioInstance, u64)>,
    },
    /// One scenario alone does not fit in the longest lease.
    Unsplittable {
        scenario: ScenarioInstance,
        lease_micros: u64,
        cap_micros: u64,
    },
}

impl From<Overflow> for PlanError {
    fn from(_: Overflow) -> Self {
        Self::Overflow
    }
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusals) => {
                f.write_str("the stand cannot run:")?;
                for (scenario, unmet) in refusals {
                    for reason in unmet {
                        write!(f, " {scenario} ({reason});")?;
                    }
                }
                Ok(())
            }
            Self::Wait {
                scenario: Some(scenario),
                error,
            } => write!(f, "{scenario}: {error}"),
            Self::Wait {
                scenario: None,
                error,
            } => write!(f, "image costs: {error}"),
            Self::Overflow => Overflow.fmt(f),
            Self::OverBudget {
                budget_micros,
                total_micros,
                largest,
            } => {
                write!(
                    f,
                    "the plan takes {total_micros} µs of a {budget_micros} µs budget; largest:"
                )?;
                for (scenario, micros) in largest {
                    write!(f, " {scenario} ({micros} µs)")?;
                }
                Ok(())
            }
            Self::Unsplittable {
                scenario,
                lease_micros,
                cap_micros,
            } => {
                write!(
                    f,
                    "{scenario} needs a {lease_micros} µs lease; the stand's longest is {cap_micros} µs"
                )
            }
        }
    }
}

impl std::error::Error for PlanError {}

/// How many of the largest scenarios an over-budget refusal names.
const NAMED: usize = 3;

/// The plan's inputs besides the scenarios.
pub struct Stand<'a> {
    /// The digest of the source snapshot the images are built from.
    pub source: &'a Sha256,
    pub profile: &'a StandProfile,
    pub offer: &'a StandOffer,
    pub costs: &'a ImageCosts,
    pub lease: LeasePolicy,
}

/// Plans `scenarios` for `tier`.
pub fn plan(tier: Tier, scenarios: &[Planned], stand: &Stand<'_>) -> Result<Plan, PlanError> {
    let refusals: Vec<_> = scenarios
        .iter()
        .filter_map(|planned| {
            let unmet = planned.resources.unmet(stand.offer);
            (!unmet.is_empty()).then(|| (planned.id.clone(), unmet))
        })
        .collect();
    if !refusals.is_empty() {
        return Err(PlanError::Refused(refusals));
    }

    let mut provisional = Vec::new();
    let mut cost = |span: &SpanName| -> Result<u64, PlanError> {
        let resolved = stand
            .profile
            .budget(span)
            .map_err(|error| PlanError::Wait {
                scenario: None,
                error,
            })?;
        provisional.extend(resolved.provisional);
        Ok(resolved.micros)
    };
    let build_micros = cost(&stand.costs.build)?;
    let flash_micros = cost(&stand.costs.flash)?;

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
    let mut sizes = Vec::new();
    for (image, members) in images {
        let recipe = image.recipe(stand.source);
        let mut builds = true;
        let mut current = Vec::new();
        let mut leased = flash_micros;
        for planned in members {
            let bounds =
                stand
                    .profile
                    .bounds_for(&recipe, planned.contract, planned.declared.clone());
            let estimate =
                (planned.estimate)(&bounds, stand.profile).map_err(|error| PlanError::Wait {
                    scenario: Some(planned.id.clone()),
                    error,
                })?;
            let own = estimate.total_micros()?;
            provisional.extend(estimate.provisional);
            sizes.push((planned.id.clone(), own));
            if !current.is_empty()
                && stand.lease.margin.apply(sum([leased, own])?)? > stand.lease.cap_micros
            {
                segments.push(segment(
                    &image,
                    recipe,
                    core::mem::take(&mut current),
                    builds,
                    build_micros,
                    leased,
                    stand.lease,
                )?);
                builds = false;
                leased = flash_micros;
            }
            let lease_micros = stand.lease.margin.apply(sum([leased, own])?)?;
            if lease_micros > stand.lease.cap_micros {
                return Err(PlanError::Unsplittable {
                    scenario: planned.id.clone(),
                    lease_micros,
                    cap_micros: stand.lease.cap_micros,
                });
            }
            current.push(planned.id.clone());
            leased = sum([leased, own])?;
        }
        if !current.is_empty() {
            segments.push(segment(
                &image,
                recipe,
                current,
                builds,
                build_micros,
                leased,
                stand.lease,
            )?);
        }
    }

    let total_micros = segments
        .iter()
        .try_fold(0_u64, |total, segment| sum([total, segment.wall_micros()?]))?;
    if let Some(budget_micros) = tier.budget_micros()
        && total_micros > budget_micros
    {
        sizes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        sizes.truncate(NAMED);
        return Err(PlanError::OverBudget {
            budget_micros,
            total_micros,
            largest: sizes,
        });
    }
    provisional.sort();
    provisional.dedup();
    Ok(Plan {
        tier,
        segments,
        total_micros,
        provisional,
    })
}

fn segment(
    image: &TargetCapabilities,
    recipe: Sha256,
    scenarios: Vec<ScenarioInstance>,
    builds: bool,
    build_micros: u64,
    leased: u64,
    lease: LeasePolicy,
) -> Result<Segment, Overflow> {
    Ok(Segment {
        image: image.clone(),
        recipe,
        scenarios,
        build_micros: if builds { build_micros } else { 0 },
        lease_estimate_micros: leased,
        lease_limit_micros: lease.margin.apply(leased)?,
    })
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU32;
    use std::collections::BTreeSet;

    use oer_hil_model::InstanceName;
    use oer_hil_model::{
        BoardCapability, BoardNeed, BoardOffer, BoardRole, Composition, CompositionName,
        Conformance, LpClockSource, MeasuredSpan, RawStandProfile, RunId, RunRef, Support, Verdict,
        Wait, name,
    };
    use serde::{Deserialize, Serialize};

    use super::*;

    const SECOND: u64 = 1_000_000;

    struct Port;

    impl Subject for Port {
        const CONTRACT: &'static str = "toy::Port";

        fn declared_bounds() -> ReportedBounds {
            ReportedBounds::default().with(name!(BoundName, "toy.terminal"), 2 * SECOND)
        }
    }

    /// A scenario whose one repetition waits for the measured span
    /// `span` and once for the port's terminal bound.
    struct Fixed;

    #[derive(Clone, Debug, Deserialize, Serialize)]
    struct Params {
        span: SpanName,
        composition: Option<CompositionName>,
        crystal: bool,
    }

    impl Spec for Fixed {
        const ID: oer_hil_model::ScenarioId = name!(ScenarioId, "fixed");
        type Purpose = Conformance;
        type Subject = Port;
        type Params = Params;
        type Criteria = ();
        type Trace = ();

        fn resources(params: &Params) -> Resources {
            let capabilities = if params.crystal {
                vec![BoardCapability::LpClock(LpClockSource::Xtal32k)]
            } else {
                Vec::new()
            };
            Resources {
                boards: vec![BoardNeed {
                    role: BoardRole::Dut,
                    chip: name!(ChipId, "esp32s31"),
                    capabilities,
                }],
                fixtures: Vec::new(),
            }
        }

        fn target(params: &Params) -> TargetCapabilities {
            TargetCapabilities {
                composition: params
                    .composition
                    .clone()
                    .map_or(Composition::Any, Composition::Product),
                capabilities: BTreeSet::from([name!(CapabilityKey, "conformance.toy")]),
            }
        }

        fn estimate(
            params: &Params,
            bounds: &Bounds,
            profile: &StandProfile,
        ) -> Result<Estimate, WaitError> {
            let terminal = Wait::contract(name!(BoundName, "toy.terminal"), Factor::ONE);
            Estimate::from_waits(
                &[],
                &[Wait::measured(params.span.clone()), terminal],
                1,
                bounds,
                profile,
            )
        }

        fn verdict((): &(), (): &()) -> Verdict {
            Verdict::Pass
        }
    }

    fn run_ref() -> RunRef {
        RunRef {
            run: RunId::parse("1788729094522-000d170b").unwrap(),
            sha256: Sha256([0; 32]),
        }
    }

    fn measured(seconds: u64) -> MeasuredSpan {
        MeasuredSpan {
            maximum_micros: seconds * SECOND,
            samples: 3_000,
            margin: Factor::ONE,
            margin_reason: "test".into(),
            support: Support::Measured,
            evidence: vec![run_ref()],
        }
    }

    fn profile(spans: &[(&'static str, u64)]) -> StandProfile {
        let mut raw = RawStandProfile::default();
        raw.spans
            .insert(name!(SpanName, "image.build"), measured(50));
        raw.spans
            .insert(name!(SpanName, "flash.application"), measured(25));
        for (span, seconds) in spans {
            raw.spans
                .insert(SpanName::parse(span).unwrap(), measured(*seconds));
        }
        StandProfile::try_from(raw).unwrap()
    }

    fn costs() -> ImageCosts {
        ImageCosts {
            build: name!(SpanName, "image.build"),
            flash: name!(SpanName, "flash.application"),
        }
    }

    fn offer() -> StandOffer {
        StandOffer {
            boards: vec![BoardOffer {
                chip: name!(ChipId, "esp32s31"),
                roles: vec![BoardRole::Dut],
                capabilities: vec![BoardCapability::LpClock(LpClockSource::RcSlow)],
            }],
            fixtures: Vec::new(),
        }
    }

    fn lease(cap_seconds: u64) -> LeasePolicy {
        LeasePolicy {
            margin: Factor::new(3, NonZeroU32::new(2).unwrap()).unwrap(),
            cap_micros: cap_seconds * SECOND,
        }
    }

    fn fixed(
        instance: &'static str,
        span: &'static str,
        composition: Option<&'static str>,
    ) -> Planned {
        Planned::of(&Instance::<Fixed>::new(
            InstanceName::parse(instance).unwrap(),
            Params {
                span: SpanName::parse(span).unwrap(),
                composition: composition.map(|name| CompositionName::parse(name).unwrap()),
                crystal: false,
            },
        ))
    }

    const STATION: &str = "oer-esp32s31-ieee80211-system";

    fn stand<'a>(
        source: &'a Sha256,
        profile: &'a StandProfile,
        offer: &'a StandOffer,
        costs: &'a ImageCosts,
        cap: u64,
    ) -> Stand<'a> {
        Stand {
            source,
            profile,
            offer,
            costs,
            lease: lease(cap),
        }
    }

    #[test]
    fn a_new_recipe_plans_on_declared_bounds_and_labels_them() {
        let (source, profile, offer, costs) = (
            Sha256([1; 32]),
            profile(&[("toy.case", 10)]),
            offer(),
            costs(),
        );
        let scenarios = [
            fixed("a", "toy.case", None),
            fixed("b", "toy.case", Some(STATION)),
        ];
        let plan = plan(
            Tier::Smoke,
            &scenarios,
            &stand(&source, &profile, &offer, &costs, 3_600),
        )
        .unwrap();
        assert_eq!(plan.segments.len(), 1);
        let segment = &plan.segments[0];
        assert_eq!(segment.build_micros, 50 * SECOND);
        assert_eq!(segment.lease_estimate_micros, (25 + 12 + 12) * SECOND);
        assert_eq!(
            segment.lease_limit_micros,
            segment.lease_estimate_micros * 3 / 2
        );
        assert_eq!(plan.total_micros, (50 + 25 + 24) * SECOND);
        assert_eq!(
            plan.provisional,
            vec![Provisional::DeclaredBound {
                contract: "toy::Port",
                bound: name!(BoundName, "toy.terminal")
            }]
        );
    }

    #[test]
    fn a_provisional_stand_span_is_labelled_in_the_plan() {
        let (source, offer, costs) = (Sha256([1; 32]), offer(), costs());
        let mut raw = RawStandProfile::from(profile(&[("toy.case", 10)]));
        let flash = name!(SpanName, "flash.application");
        let mut early = measured(25);
        early.samples = 201;
        early.support = Support::Provisional {
            reason: "first stand measurement".into(),
        };
        raw.spans.insert(flash.clone(), early);
        let profile = StandProfile::try_from(raw).unwrap();
        let plan = plan(
            Tier::Smoke,
            &[fixed("a", "toy.case", None)],
            &stand(&source, &profile, &offer, &costs, 3_600),
        )
        .unwrap();
        assert!(plan.provisional.contains(&Provisional::Span {
            span: flash,
            reason: "first stand measurement".into()
        }));
    }

    #[test]
    fn a_recorded_recipe_plans_on_its_reported_bounds() {
        let (source, offer, costs) = (Sha256([1; 32]), offer(), costs());
        let scenario = fixed("a", "toy.case", Some(STATION));
        let recipe = scenario.image.recipe(&source);
        let mut raw = RawStandProfile::from(profile(&[("toy.case", 10)]));
        raw.bounds.insert(
            recipe,
            oer_hil_model::RecordedBounds {
                bounds: ReportedBounds::default()
                    .with(name!(BoundName, "toy.terminal"), 5 * SECOND),
                evidence: run_ref(),
            },
        );
        let profile = StandProfile::try_from(raw).unwrap();
        let plan = plan(
            Tier::Smoke,
            &[scenario],
            &stand(&source, &profile, &offer, &costs, 3_600),
        )
        .unwrap();
        assert!(plan.provisional.is_empty());
        assert_eq!(plan.segments[0].lease_estimate_micros, (25 + 15) * SECOND);
    }

    #[test]
    fn a_board_without_a_crystal_refuses_the_plan() {
        let (source, profile, offer, costs) = (
            Sha256([1; 32]),
            profile(&[("toy.case", 10)]),
            offer(),
            costs(),
        );
        let crystal = Planned::of(&Instance::<Fixed>::new(
            name!(InstanceName, "sleep"),
            Params {
                span: name!(SpanName, "toy.case"),
                composition: None,
                crystal: true,
            },
        ));
        let Err(PlanError::Refused(refusals)) = plan(
            Tier::Full,
            &[crystal],
            &stand(&source, &profile, &offer, &costs, 3_600),
        ) else {
            panic!("refused");
        };
        assert_eq!(refusals.len(), 1);
    }

    #[test]
    fn a_long_tier_splits_into_leases_that_fit_the_cap_and_builds_once() {
        let (source, profile, offer, costs) = (
            Sha256([1; 32]),
            profile(&[("toy.long", 598)]),
            offer(),
            costs(),
        );
        let names = ["a", "b", "c", "d", "e", "f"];
        let scenarios: Vec<_> = names
            .iter()
            .map(|name| fixed(name, "toy.long", Some(STATION)))
            .collect();
        let plan = plan(
            Tier::Full,
            &scenarios,
            &stand(&source, &profile, &offer, &costs, 1_200),
        )
        .unwrap();
        assert!(plan.segments.len() > 1);
        assert!(
            plan.segments
                .iter()
                .all(|segment| segment.lease_limit_micros <= 1_200 * SECOND)
        );
        assert_eq!(
            plan.segments
                .iter()
                .filter(|segment| segment.build_micros > 0)
                .count(),
            1
        );
        assert_eq!(
            plan.segments
                .iter()
                .map(|segment| segment.scenarios.len())
                .sum::<usize>(),
            6
        );
    }

    #[test]
    fn the_build_never_shortens_a_lease_margin() {
        // The scenario takes 2_350 s + the 2 s terminal bound. With the
        // 25 s flash its lease is 1.5 × 2_377 s = 3_565.5 s, inside the
        // 3_600 s cap; were the 50 s build inside the lease it would need
        // 1.5 × 2_427 s = 3_640.5 s and the plan would refuse it.
        let (source, profile, offer, costs) = (
            Sha256([1; 32]),
            profile(&[("toy.long", 2_350)]),
            offer(),
            costs(),
        );
        let plan = plan(
            Tier::Full,
            &[fixed("a", "toy.long", None)],
            &stand(&source, &profile, &offer, &costs, 3_600),
        )
        .unwrap();
        let segment = &plan.segments[0];
        assert_eq!(segment.lease_estimate_micros, (25 + 2_352) * SECOND);
        assert_eq!(segment.build_micros, 50 * SECOND);
        assert_eq!(
            segment.lease_limit_micros,
            segment.lease_estimate_micros * 3 / 2
        );
    }

    #[test]
    fn an_unsplittable_scenario_reports_the_lease_it_needs() {
        let (source, profile, offer, costs) = (
            Sha256([1; 32]),
            profile(&[("toy.soak", 2_398)]),
            offer(),
            costs(),
        );
        assert_eq!(
            plan(
                Tier::Full,
                &[fixed("a", "toy.soak", None)],
                &stand(&source, &profile, &offer, &costs, 3_600)
            ),
            Err(PlanError::Unsplittable {
                scenario: ScenarioInstance {
                    scenario: name!(ScenarioId, "fixed"),
                    instance: name!(InstanceName, "a")
                },
                lease_micros: (25 + 2_400) * SECOND * 3 / 2,
                cap_micros: 3_600 * SECOND,
            })
        );
    }

    #[test]
    fn an_over_budget_plan_names_its_largest_scenarios() {
        let spans = [("toy.a", 100), ("toy.b", 200), ("toy.c", 50), ("toy.d", 10)];
        let (source, profile, offer, costs) = (Sha256([1; 32]), profile(&spans), offer(), costs());
        let scenarios = [
            fixed("a", "toy.a", None),
            fixed("b", "toy.b", None),
            fixed("c", "toy.c", None),
            fixed("d", "toy.d", None),
        ];
        let Err(PlanError::OverBudget { largest, .. }) = plan(
            Tier::Smoke,
            &scenarios,
            &stand(&source, &profile, &offer, &costs, 3_600),
        ) else {
            panic!("over budget");
        };
        let names: Vec<_> = largest
            .iter()
            .map(|(id, _)| id.instance.to_string())
            .collect();
        assert_eq!(names, ["b", "a", "c"]);
    }

    #[test]
    fn compositions_get_separate_images() {
        let (source, profile, offer, costs) = (
            Sha256([1; 32]),
            profile(&[("toy.case", 10)]),
            offer(),
            costs(),
        );
        let scenarios = [
            fixed("a", "toy.case", Some(STATION)),
            fixed("b", "toy.case", Some("oer-esp32s31-bluetooth-system")),
        ];
        let plan = plan(
            Tier::Full,
            &scenarios,
            &stand(&source, &profile, &offer, &costs, 3_600),
        )
        .unwrap();
        assert_eq!(plan.segments.len(), 2);
        assert!(plan.segments.iter().all(|segment| segment.build_micros > 0));
    }
}
