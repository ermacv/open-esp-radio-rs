use core::cell::Cell;

use embassy_futures::{block_on, join::join};

use super::{CONTINUE_BUDGET, ContinueBudget};

#[test]
fn a_runner_that_always_continues_lets_a_sibling_run() {
    let sibling_ran = Cell::new(false);
    let passes = Cell::new(0_u32);
    let runner = async {
        let mut budget = ContinueBudget::new();
        // An endless run of continued passes ends only once the sibling
        // on the same executor has made progress.
        while !sibling_ran.get() {
            passes.set(passes.get() + 1);
            budget.spend().await;
        }
    };
    let sibling = async { sibling_ran.set(true) };
    block_on(join(runner, sibling));
    assert!(sibling_ran.get());
    assert!(passes.get() <= u32::from(CONTINUE_BUDGET) + 1);
}

#[test]
fn awaiting_refills_the_budget() {
    block_on(async {
        let mut budget = ContinueBudget::new();
        for _ in 1..CONTINUE_BUDGET {
            budget.spend().await;
        }
        budget.refill();
        assert_eq!(budget.0, CONTINUE_BUDGET);
    });
}
