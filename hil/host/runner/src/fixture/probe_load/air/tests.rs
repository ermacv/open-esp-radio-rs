use super::*;
const AP: &str = "32:00:00:00:00:01";

fn capture() -> String {
    let mut text = String::new();
    for index in 0..model::REQUESTS {
        let request = model::request(index).unwrap();
        text.push_str(&format!(
            "{}\t0x0004\t{}\tff:ff:ff:ff:ff:ff\t{}\tFalse\n",
            request.offset_us as f64 / 1e6,
            mac(request.source),
            index
        ));
    }
    text.push_str(&format!(
        "1.001\t0x0005\t{AP}\t02:4f:45:52:00:00\t40\tFalse\n"
    ));
    text.push_str(&format!(
        "6.001\t0x0005\t{AP}\t02:4f:45:52:00:01\t41\tFalse\n"
    ));
    text
}

#[test]
fn full_plan_requires_air_evidence_and_responses_from_the_selected_ap() {
    assert!(Evidence::default().validate().is_err());
    assert!(parse(&capture(), AP).unwrap().validate().is_ok());
    assert!(
        parse(&capture(), "02:00:00:00:00:02")
            .unwrap()
            .validate()
            .is_err()
    );
    let missing = capture().lines().skip(1).collect::<Vec<_>>().join("\n");
    assert!(parse(&missing, AP).unwrap().validate().is_err());
}

#[test]
fn retries_duplicates_and_invalid_metadata_fail_closed() {
    let original = capture();
    let retry = original.replace("41\tFalse", "41\tTrue");
    assert!(parse(&retry, AP).unwrap().validate().is_err());
    let invalid = original.replace("41\tFalse", "41\t");
    assert!(parse(&invalid, AP).is_err());
    assert!(parse(&original.replace("6.001", "NaN"), AP).is_err());
    let duplicate = original.clone() + original.lines().last().unwrap() + "\n";
    assert!(parse(&duplicate, AP).is_err());
    assert!(
        parse(
            &original.replace("02:4f:45:52:00:c8", "02:4f:45:52:00:01"),
            AP
        )
        .is_err()
    );
}

#[test]
fn response_budget_is_global_across_receivers() {
    let mut text = capture();
    for index in 0..106 {
        text.push_str(&format!(
            "3.{}\t0x0005\t{AP}\t{}\t{}\tFalse\n",
            format_args!("{index:03}"),
            mac(model::request(201 + index).unwrap().source),
            100 + index
        ));
    }
    let evidence = parse(&text, AP).unwrap();
    assert_eq!(evidence.maximum_responses_in_one_second, 106);
    assert!(evidence.validate().is_err());
}
