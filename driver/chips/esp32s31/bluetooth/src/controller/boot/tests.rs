use std::{cell::RefCell, rc::Rc, vec::Vec};

use super::prepare_output_then_start_timer;

#[test]
fn controller_output_precedes_the_single_runtime_timer_start() {
    let operations = Rc::new(RefCell::new(Vec::new()));
    let output_operations = Rc::clone(&operations);
    let timer_operations = Rc::clone(&operations);

    let (output, timer) = prepare_output_then_start_timer(
        "interrupt-owner",
        "timer-owner",
        |owner| {
            output_operations.borrow_mut().push("prepare-output");
            owner
        },
        |owner| {
            timer_operations.borrow_mut().push("start-timer");
            owner
        },
    );

    assert_eq!(output, "interrupt-owner");
    assert_eq!(timer, "timer-owner");
    assert_eq!(
        operations.borrow().as_slice(),
        ["prepare-output", "start-timer"]
    );
}
