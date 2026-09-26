//! Bluetooth LE controller leaves of the pinned `esp32s31-bt-lib` and
//! `libbtbb.a` archives, compared with the compiled production Bluetooth
//! probes on the `wifi-mac` leaf machinery.
//!
//! The controller archives name their functions by obfuscated symbols that
//! change between releases; each leaf names the symbol of the pinned
//! release.
use crate::harness::Result;
use crate::mac::{
    Domain, Leaf, Objects, Suite, Vendor, in_archive, leaf, objects, ordered, prefix, quiet,
};

/// Session inputs of the second and third suite archives.
const BTDM_COMMON_INPUT: u64 = 4;
const BTBB_INPUT: u64 = 5;

/// `bt_bb_v2_init_cmplx` prints its version for a nonzero argument; the
/// claim covers argument one.
const PRINT_VERSION: &[u32] = &[1];
/// The `phy_param` byte at 0x120 the baseband reads: zero, a pattern and
/// all set.
const GAIN_PARAMETERS: &[u32] = &[0, 0x5a, 0xff];
const PHY_PARAM: &str = "phy_param";
const PHY_PARAM_GAIN: u32 = 0x120;

/// Memory-list selectors the production type admits.
const MEMORY_LIST_SELECTORS: &[u32] = &[1, 2, 3];
/// Memory-list pointers: none, and the first, an inner and the last word of
/// the encodable controller-SRAM window.
const MEMORY_LIST_POINTERS: &[u32] = &[0, 0x2f00_0000, 0x2f12_3454, 0x2f3f_fffc];

/// The scheduler's environment pointer cell in `libbtdm_common.a`, the
/// environment it points to, and the eight-byte argument it copies.
const SCHEDULER_ENVIRONMENT_CELL: &str = "r_sym_bt_YnZmsxu068uuC3PC4iu8";
const SCHEDULER_ENVIRONMENT: u32 = 0x3fff_3000;
const SCHEDULER_ENVIRONMENT_BYTES: usize = 0x20;
const SCHEDULER_ARGUMENT: u32 = 0x3fff_3100;
const SCHEDULER_ARGUMENT_BYTES: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// The scheduler leaf's environment and argument: the pointer cell in image
/// data names the environment, which the vendor clears and fills from the
/// argument after its hardware list-head edges.
fn scheduler_abi(_words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    Ok(Objects {
        vendor_words: vec![SCHEDULER_ARGUMENT],
        vendor: vec![
            (SCHEDULER_ENVIRONMENT, vec![0; SCHEDULER_ENVIRONMENT_BYTES]),
            (SCHEDULER_ARGUMENT, SCHEDULER_ARGUMENT_BYTES.to_vec()),
        ],
        image: vec![(
            vendor.symbol(SCHEDULER_ENVIRONMENT_CELL)?,
            SCHEDULER_ENVIRONMENT.to_le_bytes().to_vec(),
        )],
        ..Default::default()
    })
}

/// The baseband's `phy_param` gain byte, from the semantic words: the
/// version flag and the byte.
fn btbb_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [print_version, gain] = words else {
        unreachable!("baseband words: version flag, gain byte")
    };
    Ok(Objects {
        vendor_words: vec![*print_version],
        vendor: vec![(
            vendor.symbol(PHY_PARAM)? + PHY_PARAM_GAIN,
            vec![*gain as u8],
        )],
        ..Default::default()
    })
}

/// Every compared Bluetooth leaf.
pub const LEAVES: &[Leaf] = &[
    // Current-RX memory-list pointer (formerly `r_sym_ble_LboRu27EaU8MV8Q7UUfZ`).
    ordered(
        leaf(
            "r_sym_ble_jhaN9KOsH0u0RgIu2DI9",
            "open_ble_memory_list_a_trace_r_sym_ble_lbo_ru27_ea_u8_mv8_q7_uuf_z",
            &[
                ("selector", Domain::Words(MEMORY_LIST_SELECTORS)),
                ("pointer", Domain::Words(MEMORY_LIST_POINTERS)),
            ],
            false,
        ),
        1,
    ),
    // Next-RX memory-list pointer (formerly `r_sym_ble_ZzrExMrn8EDiTFI7PENK`).
    ordered(
        leaf(
            "r_sym_ble_95olV4pLl0ue84SnuzXf",
            "open_ble_memory_list_b_trace_r_sym_ble_zzr_ex_mrn8_edi_tfi7_penk",
            &[
                ("selector", Domain::Words(MEMORY_LIST_SELECTORS)),
                ("pointer", Domain::Words(MEMORY_LIST_POINTERS)),
            ],
            false,
        ),
        1,
    ),
    // NRT interrupt capture and acknowledgement, up to its log/broker suffix
    // (formerly `r_sym_ble_ywjh0f9yjTBeI7XgS5da`).
    ordered(
        prefix(
            leaf(
                "r_sym_nrtIsr_kgxM2CJfVperrlVjmB4B",
                "open_ble_interrupt_trace_r_sym_ble_ywjh0f9yj_t_be_i7_xg_s5da",
                &[],
                false,
            ),
            "r_ble_log_set_buf_index_flag",
        ),
        1,
    ),
    // Scheduler hardware list heads, up to the event initialization.
    in_archive(
        ordered(
            prefix(
                objects(
                    leaf(
                        "r_sym_bt_XPuqTHliEO5V9xpR7aJR",
                        "open_ble_scheduler_trace_r_sym_bt_x_puq_thli_eo5_v9xp_r7a_jr",
                        &[],
                        false,
                    ),
                    scheduler_abi,
                ),
                "wr_btdm_osal_event_init",
            ),
            1,
        ),
        BTDM_COMMON_INPUT,
    ),
    // Baseband v2 initialization with the version log answered without
    // effect, closed by the owner's device fence.
    in_archive(
        ordered(
            quiet(
                objects(
                    leaf(
                        "bt_bb_v2_init_cmplx",
                        "open_btbb_v2_init_trace_r_sym_bt_bb_v2_init_cmplx_x1",
                        &[
                            ("_print_version", Domain::Words(PRINT_VERSION)),
                            ("gain_parameter", Domain::Words(GAIN_PARAMETERS)),
                        ],
                        false,
                    ),
                    btbb_abi,
                ),
                &["bt_bb_v2_version"],
            ),
            1,
        ),
        BTBB_INPUT,
    ),
];

/// The Bluetooth controller suite.
pub const BLUETOOTH: Suite = Suite {
    title: "Bluetooth LE controller leaf comparison",
    archives: &["libble_app", "libbtdm_common", "libbtbb"],
    leaves: LEAVES,
    wifi: false,
    // ESP-IDF linker-script broker tables, which the interrupt and scheduler
    // reach only after their compared prefixes.
    absent: &[
        "_nrtIsr_linear_broker_flash",
        "_btdm_sched_linear_broker_flash",
    ],
};
