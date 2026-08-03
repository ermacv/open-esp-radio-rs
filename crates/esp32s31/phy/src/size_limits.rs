//! Compile-time SRAM budgets for the largest PHY transitions.

use core::mem::size_of;

use crate::{
    PhyRegisterTransition,
    phy_bb::PhyBbInitTransition,
    phy_rx_gain::{PhyRxGainInitTransition, PhyRxGainPublishTransition},
};

// These are reviewed RV32 budgets, rounded above the 1.97.1 layouts rather
// than snapshots of compiler-selected padding. A transition that crosses a
// boundary must be split or receive an explicit SRAM-budget review.
//
// `PhyRegisterTransition` now optionally owns the complete 524-byte retained
// calibration record while it also owns the live 508-byte PHY state. That
// overlap is required only across full calibration and its exact vendor-order
// backup; it replaces an equally sized caller buffer rather than introducing
// an unbounded allocation. Keep the rounded ceiling explicit so further state
// growth still fails at compile time.
const PHY_REGISTER_TRANSITION_LIMIT: usize = 3_072;
const PHY_BB_INIT_TRANSITION_LIMIT: usize = 1_600;
const PHY_RX_GAIN_INIT_TRANSITION_LIMIT: usize = 1_088;
const PHY_RX_GAIN_PUBLISH_TRANSITION_LIMIT: usize = 832;

const _: () = {
    assert!(size_of::<PhyRegisterTransition>() <= PHY_REGISTER_TRANSITION_LIMIT);
    assert!(size_of::<PhyBbInitTransition>() <= PHY_BB_INIT_TRANSITION_LIMIT);
    assert!(size_of::<PhyRxGainInitTransition>() <= PHY_RX_GAIN_INIT_TRANSITION_LIMIT);
    assert!(size_of::<PhyRxGainPublishTransition>() <= PHY_RX_GAIN_PUBLISH_TRANSITION_LIMIT);
};
