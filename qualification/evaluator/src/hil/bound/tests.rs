use super::*;
use serde_json::json;

fn bound(document: Value) -> Result<Bound> {
    serde_json::from_value::<BoundDocument>(document)?.validated("capability")
}

/// The connection-establishment loss bound: at most `limit` basis points
/// of the attempts over the repetition set, from `minimum` attempts on.
fn loss(limit: u64, minimum: u64) -> Bound {
    bound(json!({
        "ratio": {
            "numerator": { "name": "connections-not-established", "unit": "count", "semantics": 1, "better": "lower" },
            "denominator": { "name": "connection-attempts", "unit": "count", "semantics": 1 },
        },
        "over": "current-runs",
        "at-most-basis-points": limit,
        "minimum-denominator": minimum,
    }))
    .unwrap()
}

fn repetition(lost: u64, attempts: u64) -> Vec<Value> {
    vec![
        json!({ "name": "connection-attempts", "value": attempts, "unit": "count", "semantics": 1, "better": null, "threshold": null, "verdict": null }),
        json!({ "name": "connections-not-established", "value": lost, "unit": "count", "semantics": 1, "better": "lower", "threshold": null, "verdict": null }),
    ]
}

#[test]
fn a_ratio_sums_the_repetition_set_and_holds_at_its_limit() {
    // 15 of 100 is exactly 1500 basis points.
    let finding = loss(1_500, 100).assess(&[repetition(5, 40), repetition(10, 60)]);
    assert_eq!(finding.assessment, Assessment::Passed);
    assert_eq!(finding.values, [(15, Some(100))]);
    let finding = loss(1_500, 100).assess(&[repetition(6, 40), repetition(10, 60)]);
    assert_eq!(finding.assessment, Assessment::Failed);
}

#[test]
fn a_ratio_below_its_minimum_denominator_is_insufficient_never_a_pass() {
    let finding = loss(1_500, 100).assess(&[repetition(0, 9)]);
    assert_eq!(finding.assessment, Assessment::InsufficientSample);
    // Exactly the minimum decides.
    assert_eq!(
        loss(1_500, 9).assess(&[repetition(0, 9)]).assessment,
        Assessment::Passed
    );
}

#[test]
fn a_measurement_missing_or_with_another_identity_is_unavailable() {
    let renamed = vec![repetition(0, 100)[0].clone()];
    assert_eq!(
        loss(1_500, 1)
            .assess(&[repetition(0, 50), renamed])
            .assessment,
        Assessment::Unavailable
    );
    for (field, value) in [
        ("unit", json!("bytes")),
        ("semantics", json!(2)),
        ("better", json!(null)),
    ] {
        let mut retyped = repetition(0, 100);
        retyped[1][field] = value;
        assert_eq!(
            loss(1_500, 1).assess(&[retyped]).assessment,
            Assessment::Unavailable,
            "{field}"
        );
    }
    let mut twice = repetition(0, 100);
    twice.push(twice[1].clone());
    assert_eq!(
        loss(1_500, 1).assess(&[twice]).assessment,
        Assessment::Unavailable
    );
    assert_eq!(
        loss(1_500, 1).assess(&[]).assessment,
        Assessment::Unavailable
    );
}

#[test]
fn a_value_bound_holds_in_every_repetition() {
    let early = bound(json!({
        "measurement": { "name": "connections-lost-before-att", "unit": "count", "semantics": 1, "better": "lower" },
        "over": "repetition",
        "at-most": 0,
    }))
    .unwrap();
    let lost = |value: u64| {
        vec![
            json!({ "name": "connections-lost-before-att", "value": value, "unit": "count", "semantics": 1, "better": "lower", "threshold": null, "verdict": null }),
        ]
    };
    let finding = early.assess(&[lost(0), lost(0)]);
    assert_eq!(finding.assessment, Assessment::Passed);
    assert_eq!(finding.values, [(0, None), (0, None)]);
    assert_eq!(
        early.assess(&[lost(0), lost(1)]).assessment,
        Assessment::Failed
    );
    assert_eq!(
        early.to_string(),
        "connections-lost-before-att<=0count/repetition"
    );
}

#[test]
fn a_declaration_needs_one_quantity_one_limit_and_a_matching_direction() {
    let metric = json!({ "name": "a", "unit": "count", "semantics": 1, "better": "lower" });
    let ratio = json!({ "numerator": metric, "denominator": { "name": "b", "unit": "count", "semantics": 1 } });
    for (document, reason) in [
        (
            json!({ "over": "repetition", "at-most": 1 }),
            "one of measurement",
        ),
        (
            json!({ "measurement": metric, "ratio": ratio, "over": "repetition", "at-most": 1 }),
            "one of measurement",
        ),
        (
            json!({ "measurement": metric, "over": "repetition" }),
            "exactly one limit",
        ),
        (
            json!({ "measurement": metric, "over": "repetition", "at-most": 1, "at-least": 0 }),
            "exactly one limit",
        ),
        (
            json!({ "measurement": metric, "over": "repetition", "at-most-basis-points": 1 }),
            "own unit",
        ),
        (
            json!({ "measurement": metric, "over": "repetition", "at-least": 1 }),
            "better = \"higher\"",
        ),
        (
            json!({ "measurement": metric, "over": "current-runs", "at-most": 1 }),
            "per repetition",
        ),
        (
            json!({ "ratio": ratio, "over": "current-runs", "at-most": 1, "minimum-denominator": 1 }),
            "basis points",
        ),
        (
            json!({ "ratio": ratio, "over": "current-runs", "at-most-basis-points": 1 }),
            "minimum-denominator",
        ),
        (
            json!({ "ratio": ratio, "over": "current-runs", "at-most-basis-points": 1, "minimum-denominator": 0 }),
            "minimum-denominator",
        ),
        (
            json!({ "ratio": ratio, "over": "current-runs", "at-most-basis-points": 10_001, "minimum-denominator": 1 }),
            "10000",
        ),
    ] {
        let error = bound(document.clone()).unwrap_err().to_string();
        assert!(error.contains(reason), "{document}: {error}");
    }
    assert!(
        serde_json::from_value::<BoundDocument>(
            json!({ "measurement": metric, "over": "repetition", "at-most": 1, "threshold": 1 })
        )
        .is_err()
    );
}

#[test]
fn a_repetition_outside_the_limit_outranks_another_one_s_small_sample() {
    let per_repetition = bound(json!({
        "ratio": {
            "numerator": { "name": "connections-not-established", "unit": "count", "semantics": 1, "better": "lower" },
            "denominator": { "name": "connection-attempts", "unit": "count", "semantics": 1 },
        },
        "over": "repetition",
        "at-most-basis-points": 1_000,
        "minimum-denominator": 10,
    }))
    .unwrap();
    for repetitions in [
        [repetition(5, 10), repetition(0, 2)],
        [repetition(0, 2), repetition(5, 10)],
    ] {
        assert_eq!(
            per_repetition.assess(&repetitions).assessment,
            Assessment::Failed
        );
    }
    assert_eq!(
        per_repetition
            .assess(&[repetition(0, 10), repetition(0, 2)])
            .assessment,
        Assessment::InsufficientSample
    );
}
