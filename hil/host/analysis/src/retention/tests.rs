use oer_hil_run_bundle::RunStore;
use oer_hil_run_bundle::run::test_support::write_run;
use oer_hil_run_bundle::store::Sidecar;

use super::*;
use crate::runs::tests::bundle;

#[test]
fn over_its_budget_a_store_loses_its_oldest_runs_only_age_kept() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    for (id, started) in [("1-a", 1), ("2-b", 2), ("3-c", 3), ("4-d", 4)] {
        write_run(
            &store.run(id),
            started,
            RunState::Completed,
            Vec::new(),
            |_| {},
        );
    }
    let runs = Run::all(&store).unwrap();
    let sizes = [("1-a", 10), ("2-b", 20), ("3-c", 30), ("4-d", 40)]
        .map(|(id, size)| (String::from(id), size))
        .into();
    let kept = BTreeMap::from([(String::from("2-b"), String::from("pinned"))]);
    let deleted = over_budget(&runs, &kept, &sizes, 65)
        .into_iter()
        .map(Run::id)
        .collect::<Vec<_>>();
    // 100 bytes: the oldest goes (90), the pinned one stays, the next
    // goes (60) and the store fits.
    assert_eq!(deleted, ["1-a", "3-c"]);
    assert!(over_budget(&runs, &kept, &sizes, 100).is_empty());
}

#[test]
fn pruning_keeps_what_is_cited_replayed_recent_and_latest() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    let runs = store.runs();
    let day = 24 * 3600 * 1000;
    bundle(&runs, "old-pass", day, true, None);
    bundle(&runs, "new-pass", 2 * day, true, None);
    for index in 0..4 {
        bundle(
            &runs,
            &format!("fail-{index}"),
            3 * day + index,
            false,
            None,
        );
    }
    bundle(&runs, "baseline", 4 * day, false, None);
    bundle(&runs, "replayer", 5 * day, true, Some("old-pass"));
    bundle(&runs, "recent", 100 * day, false, None);
    let runs = Run::all(&store).unwrap();
    let keep = retained(
        &runs,
        &Retention {
            keep_days: 30,
            keep_failed: 2,
        },
        100 * day,
        &BTreeSet::from([String::from("baseline")]),
        &BTreeSet::from([String::from("fail-0")]),
    );
    assert_eq!(keep["baseline"], "pinned");
    assert_eq!(keep["fail-0"], "cited by a committed shard");
    assert!(keep["old-pass"].starts_with("firmware replayed"));
    assert!(keep["replayer"].starts_with("latest pass"));
    assert!(keep["recent"].starts_with("younger"));
    let deleted = runs
        .iter()
        .filter(|run| !keep.contains_key(run.id()))
        .map(Run::id)
        .collect::<Vec<_>>();
    // The two newest failures are `recent` and `baseline`.
    assert_eq!(deleted, ["new-pass", "fail-1", "fail-2", "fail-3"]);
}

#[test]
fn a_prune_lists_then_deletes_what_no_rule_keeps_and_holds_the_budget() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    let day = 24 * 3600 * 1000;
    bundle(&store.runs(), "old-pass", day, true, None);
    bundle(&store.runs(), "new-pass", 2 * day, true, None);
    store
        .note(
            Sidecar::Pins,
            "old-pass",
            Some(oer_hil_run_bundle::store::Note {
                by: "wifi".into(),
                reason: "baseline".into(),
                unix_millis: 0,
            }),
        )
        .unwrap();
    let rule = Retention {
        keep_days: 0,
        keep_failed: 0,
    };
    // `old-pass` is pinned and `new-pass` the latest pass: nothing goes.
    let listed = prune(&store, directory.path(), &rule, None, false).unwrap();
    assert!(listed.removed.is_empty());
    assert_eq!((listed.kept, listed.total), (2, 2));
    // Over a budget of nothing, a run only its age keeps still stays: the
    // budget never overrides another rule.
    let pruned = prune(&store, directory.path(), &rule, Some(0), true).unwrap();
    assert!(pruned.removed.is_empty(), "{:?}", pruned.removed.len());
    store.note(Sidecar::Pins, "old-pass", None).unwrap();
    let pruned = prune(&store, directory.path(), &rule, None, true).unwrap();
    assert_eq!(
        pruned
            .removed
            .iter()
            .map(|(run, _)| run.id())
            .collect::<Vec<_>>(),
        ["old-pass"]
    );
    assert!(!store.run("old-pass").exists());
    assert!(store.run("new-pass").is_dir());
}
