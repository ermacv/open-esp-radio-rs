//! Unresolved and scenario-modeled call regressions.

use super::*;

#[test]
fn unresolved_external_tail_call_fails_closed() {
    let image = tail_relocation_image(None);
    let inventory = image.coverage_inventory("wrapper").unwrap();
    assert_eq!(inventory.unresolved_edges.len(), 1);
    assert!(inventory.branch_sites.is_empty());

    let svd = MmioRegisterMap {
        registers: Vec::new(),
        windows: vec![crate::Window { start: 0, end: 1 }],
    };
    let error = execute(&image, &svd, "wrapper", Scenario::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unresolved external call callee")
    );
}

#[test]
fn reviewed_call_model_intercepts_linked_code_and_is_fully_consumed() {
    let mut image = tiny_image(
        vec![
            0x13, 0x84, 0x00, 0x00, // addi s0, ra, 0
            0xef, 0x00, 0x00, 0x01, // jal ra, 16
            0x93, 0x00, 0x04, 0x00, // addi ra, s0, 0
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // padding
            0x73, 0x00, 0x10, 0x00, // callee: ebreak (must not execute)
        ],
        24,
    );
    image
        .symbols_by_name
        .insert("platform_service".to_owned(), 0x1014);
    image
        .symbols_by_address
        .insert(0x1014, "platform_service".to_owned());
    let scenario = Scenario {
        call_returns: BTreeMap::from([(
            "platform_service".to_owned(),
            VecDeque::from([0x1234_5678]),
        )]),
        ..Scenario::default()
    };

    let result = execute(&image, &empty_svd(), "test", scenario).unwrap();
    assert_eq!(result.return_value, 0x1234_5678);
    assert_eq!(result.ordered_calls.len(), 1);
    assert_eq!(result.ordered_calls[0].symbol, "platform_service");
}

#[test]
fn reviewed_call_model_rejects_missing_and_unused_responses() {
    let mut image = tiny_image(
        vec![
            0x13, 0x84, 0x00, 0x00, // addi s0, ra, 0
            0xef, 0x00, 0x00, 0x01, // jal ra, 16
            0x93, 0x00, 0x04, 0x00, // addi ra, s0, 0
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // padding
            0x67, 0x80, 0x00, 0x00, // callee: ret
        ],
        24,
    );
    image
        .symbols_by_name
        .insert("platform_service".to_owned(), 0x1014);
    image
        .symbols_by_address
        .insert(0x1014, "platform_service".to_owned());

    let missing = Scenario {
        call_returns: BTreeMap::from([("platform_service".to_owned(), VecDeque::new())]),
        ..Scenario::default()
    };
    assert!(
        execute(&image, &empty_svd(), "test", missing)
            .unwrap_err()
            .to_string()
            .contains("without a remaining response")
    );

    let unused = Scenario {
        call_returns: BTreeMap::from([("platform_service".to_owned(), VecDeque::from([1, 2]))]),
        ..Scenario::default()
    };
    assert!(
        execute(&image, &empty_svd(), "test", unused)
            .unwrap_err()
            .to_string()
            .contains("unconsumed modeled call responses")
    );
}
