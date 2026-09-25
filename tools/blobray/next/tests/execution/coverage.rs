use super::interfaces::run;
use super::*;

/// `beq a0, zero, +8`, `li a0, 1`, `ret`, then an unreachable `li a0, 2`.
const PROGRAM: [u32; 4] = [0x00050463, 0x00100513, 0x00008067, 0x00200513];

fn coverage(rows: &[ExecutionEvidence], side: bool) -> ExecutionCoverage {
    let mut found = rows.iter().filter_map(|r| match r {
        ExecutionEvidence::Coverage {
            replacement,
            coverage,
        } if *replacement == side => Some(coverage.clone()),
        _ => None,
    });
    let coverage = found.next().expect("side coverage");
    assert!(found.next().is_none(), "one coverage record per side");
    coverage
}

#[test]
fn coverage_accumulates_each_side_across_cold_sessions() {
    let f = Fixture::new(&PROGRAM);
    let mut r = f.request();
    // Vendor: taken, then (after a cold reset) fallthrough. Replacement: taken twice.
    let mut second = r.cases[0].clone();
    second.name = "fallthrough".into();
    second.reset = SessionReset::Cold;
    second.vendor.arguments[0] = Some(5);
    r.cases.push(second);
    let (_, rows) = run(&f, r);
    assert!(matches!(
        rows.last(),
        Some(ExecutionEvidence::Coverage {
            replacement: true,
            ..
        })
    ));
    assert_eq!(
        coverage(&rows, false),
        ExecutionCoverage {
            instructions: vec![0x1000, 0x1004, 0x1008],
            branches: vec![BranchCoverage {
                site: 0x1000,
                taken: true,
                fallthrough: true,
            }],
        }
    );
    assert_eq!(
        coverage(&rows, true),
        ExecutionCoverage {
            instructions: vec![0x1000, 0x1008],
            branches: vec![BranchCoverage {
                site: 0x1000,
                taken: true,
                fallthrough: false,
            }],
        }
    );
}

#[test]
fn coverage_does_not_depend_on_selected_timeline() {
    let f = Fixture::new(&PROGRAM);
    let r = f.request();
    let (_, plain) = run(&f, r.clone());
    let mut traced = r;
    let case = &mut traced.cases[0];
    case.vendor.observe_timeline.branches = true;
    case.replacement.as_mut().unwrap().observe_timeline.branches = true;
    case.relation.as_mut().unwrap().events.timeline.branches = true;
    let (_, traced) = run(&f, traced);
    assert_eq!(coverage(&plain, false), coverage(&traced, false));
    assert!(traced.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::Branch { .. },
            ..
        }
    )));
}
