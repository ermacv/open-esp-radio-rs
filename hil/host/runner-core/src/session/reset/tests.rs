use super::*;
use std::cell::RefCell;

/// A port whose chip restarts on RTS's rising edge and speaks at once, as
/// the esp32c5 and esp32s31 USB Serial/JTAG do.
struct Port {
    rts: bool,
    input: Vec<&'static str>,
}

fn reset(port: &RefCell<Port>) {
    sequence(
        |step| {
            let mut port = port.borrow_mut();
            match step {
                Step::Rts(level) => {
                    if level && !port.rts {
                        port.input.push("new boot");
                    }
                    port.rts = level;
                }
                Step::ClearInput => port.input.clear(),
                Step::Dtr(_) => {}
            }
            Ok::<_, ()>(())
        },
        || {
            let mut port = port.borrow_mut();
            if !port.rts {
                port.input.push("old event");
            }
        },
    )
    .unwrap();
}

#[test]
fn the_new_boot_keeps_what_it_sends_before_the_release() {
    let port = RefCell::new(Port {
        rts: false,
        input: vec!["previous event"],
    });
    reset(&port);
    assert_eq!(port.into_inner().input, ["new boot"]);
}

#[test]
fn failed_input_drain_does_not_reset_into_an_ambiguous_boot() {
    let mut asserted = false;
    let result = sequence(
        |step| match step {
            Step::ClearInput => Err("drain failed"),
            Step::Rts(true) => {
                asserted = true;
                Ok(())
            }
            _ => Ok(()),
        },
        || {},
    );
    assert_eq!(result, Err("drain failed"));
    assert!(!asserted);
}
