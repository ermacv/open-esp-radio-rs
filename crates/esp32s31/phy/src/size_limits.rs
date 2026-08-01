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
const PHY_REGISTER_TRANSITION_LIMIT: usize = 2_304;
const PHY_BB_INIT_TRANSITION_LIMIT: usize = 1_600;
const PHY_RX_GAIN_INIT_TRANSITION_LIMIT: usize = 1_088;
const PHY_RX_GAIN_PUBLISH_TRANSITION_LIMIT: usize = 832;

const _: () = {
    assert!(size_of::<PhyRegisterTransition>() <= PHY_REGISTER_TRANSITION_LIMIT);
    assert!(size_of::<PhyBbInitTransition>() <= PHY_BB_INIT_TRANSITION_LIMIT);
    assert!(size_of::<PhyRxGainInitTransition>() <= PHY_RX_GAIN_INIT_TRANSITION_LIMIT);
    assert!(size_of::<PhyRxGainPublishTransition>() <= PHY_RX_GAIN_PUBLISH_TRANSITION_LIMIT);
};
