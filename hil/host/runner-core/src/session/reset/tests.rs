use super::*;
use std::cell::RefCell;

#[test]
fn events_emitted_during_reset_setup_do_not_become_the_new_boot() {
    struct Port {
        reset: bool,
        input: Vec<&'static str>,
    }
    let port = RefCell::new(Port {
        reset: false,
        input: vec!["previous event"],
    });
    sequence(
        |step| {
            let mut port = port.borrow_mut();
            match step {
                Step::Rts(reset) => {
                    if port.reset && !reset {
                        port.input.push("new Hello");
                    }
                    port.reset = reset;
                }
                Step::ClearInput => port.input.clear(),
                Step::Dtr(_) => {}
            }
            Ok::<_, ()>(())
        },
        || {
            let mut port = port.borrow_mut();
            if !port.reset {
                port.input.push("late ServiceReady");
            }
        },
    )
    .unwrap();
    assert_eq!(port.into_inner().input, ["new Hello"]);
}

#[test]
fn failed_input_drain_does_not_release_an_ambiguous_boot() {
    let mut released = false;
    let result = sequence(
        |step| match step {
            Step::ClearInput => Err("drain failed"),
            Step::Rts(false) => {
                released = true;
                Ok(())
            }
            _ => Ok(()),
        },
        || {},
    );
    assert_eq!(result, Err("drain failed"));
    assert!(!released);
}
