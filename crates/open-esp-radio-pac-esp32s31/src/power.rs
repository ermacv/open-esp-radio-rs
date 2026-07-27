//! ESP32-S31 modem power, clock and reset registers.
//!
//! This module describes register identity only. The ordered cold-start
//! transaction and its ownership transition belong to the HAL.

use crate::Register32;

pub mod modem_syscon {
    use crate::Register32;

    pub const CLK_CONF: Register32 = Register32::new(0x2010_9c04);
    pub const CLK_CONF_POWER_ST: Register32 = Register32::new(0x2010_9c0c);
    pub const RST_CONF: Register32 = Register32::new(0x2010_9c10);
    pub const CLK_CONF1: Register32 = Register32::new(0x2010_9c14);
}

pub mod modem_lpcon {
    use crate::Register32;

    pub const CLK_CONF: Register32 = Register32::new(0x2010_f018);
    pub const CLK_CONF_POWER_ST: Register32 = Register32::new(0x2010_f020);
}

pub mod hp_modem {
    use crate::Register32;

    pub const CTRL0: Register32 = Register32::new(0x2058_7040);
    pub const CONF: Register32 = Register32::new(0x2058_71e0);
}

pub mod pmu {
    use crate::Register32;

    pub const HP_ACTIVE_ICG_MODEM: Register32 = Register32::new(0x2070_4014);
    pub const IMM_SLEEP_SYSCLK: Register32 = Register32::new(0x2070_40f8);
    pub const IMM_MODEM_ICG: Register32 = Register32::new(0x2070_4104);
}

/// Complete register allow-list used by the HAL power transition.
pub const ALL: [Register32; 11] = [
    modem_syscon::CLK_CONF,
    modem_syscon::CLK_CONF_POWER_ST,
    modem_syscon::RST_CONF,
    modem_syscon::CLK_CONF1,
    modem_lpcon::CLK_CONF,
    modem_lpcon::CLK_CONF_POWER_ST,
    hp_modem::CTRL0,
    hp_modem::CONF,
    pmu::HP_ACTIVE_ICG_MODEM,
    pmu::IMM_SLEEP_SYSCLK,
    pmu::IMM_MODEM_ICG,
];
