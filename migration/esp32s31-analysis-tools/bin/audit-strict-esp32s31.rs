use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{bail, Context, Result};

#[path = "../esp32s31_strict_policy.rs"]
mod strict_policy;
use strict_policy::{
    REQUIRED_RUNTIME_ALIASES, ROOTS, RUST_BOUNDARIES_WITH_VENDOR_FALLBACK,
    STATEFUL_OR_UNPROVEN_RUNTIME_ROOTS, STATIC_BINDING_ROOTS, STATIC_PM_INIT_ROOTS,
    STRICT_REFERENCE_ROOTS, TEMPORARY_EVIDENCED_MMIO_ROOTS, WRAPPED_VENDOR_BOUNDARIES,
};

const REPLACED_VENDOR_ROOTS: &[&str] = &[
    "wdevProcessRxSucDataAll",
    "ic_get_next_tbtt",
    "pp_timer_do_process",
    "pp_default_event_handler",
    "pp_coex_tx_release",
    "ppProcessTxQ",
    "lmacProcessTxTimeout",
    "lmacDiscardFrameExchangeSequence",
    "lmacDiscardMSDU",
    "ppProcTxDone",
    "lmacTxDone",
    "hal_mac_get_txq_state",
    "hal_mac_get_txq_complete",
    "lmacRequestTxopQueue",
    "lmacReleaseTxopQueue",
    "hal_get_tsf_time",
    "hal_mac_rx_get_last_dscr",
    "hal_mac_tx_set_cca",
    "hal_mac_is_txq_valid",
    "hal_mac_set_txq_invalid",
    "hal_mac_txq_disable",
    "hal_mac_set_csi_cbw",
    "ic_set_mac",
    "ic_set_rx_policy",
    "ic_set_rx_policy_ubssid_check",
    "ieee80211_getmgtframe",
    "ic_set_key",
    "ic_del_key",
    "wDev_Insert_KeyEntry",
    "phy_set_rx_comp_new",
    "phy_dc_mem_clr",
    "phy_set_tx_gain_mem_new",
    "lmacProcessTxComplete",
    "lmacProcessTxSuccess",
    "lmacProcessCtsTimeout",
    "lmacProcessAckTimeout",
    "lmacProcessTxRtsError",
    "lmacProcessTxError",
    "lmacProcessCollisions_task",
    "ppRxPkt",
    "ppRxProtoProc",
    "rc_get_trc",
    "rcUpdateRxDone",
    "rcUpdateAckSnr",
    "rcTxUpdatePer",
    "ieee80211_output_process",
    "ppTxPkt",
    "ppMapTxQueue",
    "ppDequeueTxQ",
    "ieee80211_hostapd_beacon_txcb",
    "ieee80211_tx_mgt_cb",
    "wDev_record_ftm_data",
    "pm_on_beacon_rx",
    "pm_on_data_rx",
    "pm_on_data_tx",
    "pm_set_beacon_duration",
    "dbg_read_tx_ppdu",
    "dbg_dump_rx_ppdu",
    "dbg_dump_rx_sigb",
    "wifi_gpio_debug",
    "esp_test_tx_enab_statistics",
    "esp_test_set_rx_error_occurs",
    "esp_test_rx_parse_mu",
    "esp_test_rx_process_complete",
    "wDev_SnifferRxData",
    "wdev_csi_rx_process",
    "wDev_ftm_set_t1t4",
    "wDev_isNANPktInValidSlot",
    "wDev_AppendRxBlocks",
    "wDev_DiscardFrame",
    "ppRecycleRxPkt",
    "esp_wifi_internal_free_rx_buffer",
    "wDev_IndicateCtrlFrame",
    "wpa_sm_rx_eapol",
    "wpa_ap_rx_eapol",
    "hal_crypto_set_key_entry",
    "wifi_log",
    "wifi_assert",
    "pp_post",
    "ieee80211_timer_process",
    "ieee80211_timer_do_process",
    "chm_start_op",
    "chm_return_home_channel",
    "esf_buf_alloc",
    "esf_buf_recycle",
    "ieee80211_mgmt_output",
    "ieee80211_set_tx_pti",
    "ieee80211_classify",
    "ieee80211_align_eb",
    "ieee80211_crypto_encap",
    "ieee80211_search_node",
    "cnx_node_alloc",
    "cnx_node_search",
    "rcGetSched",
    "ppTxProtoProc",
    "ppProcTxSecFrame",
];

// This pinned register-indirect site is excluded only after its live guard has
// been reproduced at the strict call site: ESF frame[0] is forced null before
// cache recycle.
const INVARIANT_EXCLUDED_INDIRECTS: &[&str] = &["ieee80211_recycle_cache_eb"];

// Preparation calls the pinned one-store
// `esp_wifi_set_sta_rx_probe_req(NULL)` leaf and verifies the ROM-BSS pointer
// before strict RX is armed. This callback only exports observed Probe Request
// frames; ordinary AP/STA management delivery does not depend on it.
const INVARIANT_EXCLUDED_INDIRECT_SITES: &[(&str, u64)] = &[("wDev_ProcessRxSucData", 0x296)];

// `phy_get_romfunc_addr` overwrites these exact slots after obtaining the ROM
// table. The pinned S31 object writes offset 20 to `phy_set_rx_comp_new` and
// offset 36 to `phy_wifi_get_tx_tab_new`; both targets audit cleanly.
const PINNED_INDIRECT_TARGETS: &[(&str, &str)] = &[
    ("phy_chip_set_chan", "phy_set_rx_comp_new"),
    ("phy_wifi_set_tx_gain_new", "phy_wifi_get_tx_tab_new"),
];

// The vendor RX-success object calls OSI slot 1 at this exact instruction.
// `wifi_osi_funcs_t` places `_env_is_chip` at byte offset 4, and strict handoff
// replaces and verifies that slot with the constant SRAM leaf below. Keep this
// proof site-specific: `wDev_ProcessRxSucData` has another unrelated indirect
// callback at 0x296 which remains subject to the audit.
const PINNED_INDIRECT_SITES: &[(&str, u64, &str)] =
    &[("wDev_ProcessRxSucData", 0x5fe, "wifi_strict_env_is_chip")];

// `phy_change_channel` remains a reference-only oracle for the pre-handoff
// vendor sequence. Its archived `phy_set_tx_gain_mem_new` descendant has an
// exact caller count of 32 and an inner four-halfword copy, so these cycles
// stay proven for that oracle even though runtime resolves the public leaf to
// Rust and no longer classifies it as ownership debt.
// `rc_get_trc` clears one set bit from a local u32 peer bitmap per iteration,
// and compares exactly six address bytes, so it exits after at most 32 steps.
// `is_ndpa_to_dut` scans four-byte HE user-info records. Its record count is
// `(frame_len - 21) >> 2`, explicitly narrowed to u8 before the do-while loop.
// The zero case wraps once through all u8 values, making the exact worst case
// 256 finite data records. It never polls a register or waits for external
// state; the per-record `hal_he_get_aid` call is an audited direct leaf.
// Strict Rust calls `wDev_ProcessRxSucData` only after its SRAM outer walk has
// followed the completed descriptor segment, checked every payload and found
// the final marker within 64 links. That exact tail and count are passed into
// `wDev_IndicateFrame`; its two backedges only copy the already-owned segment.
// The ROM-to-ROM call is not GNU-wrap interposable, so this proof belongs at
// the real Rust caller rather than behind a link-only wrapper.
const PINNED_BOUNDED_CYCLE_SITES: &[(&str, u64)] = &[
    ("phy_set_tx_gain_mem_new", 0xaa),
    ("phy_set_tx_gain_mem_new", 0x12e),
    ("rc_get_trc", 0x74),
    ("is_ndpa_to_dut", 0x66),
    ("wDev_IndicateFrame", 0x184),
    ("wDev_IndicateFrame", 0x32a),
];

const REQUIRED_RUNTIME_WRAPPERS: &[&str] = &[
    "__wrap_ic_get_next_tbtt",
    "__wrap_lmacTxDone",
    "__wrap_hal_mac_get_txq_state",
    "__wrap_hal_mac_get_txq_complete",
    "__wrap_ieee80211_hostapd_beacon_txcb",
    "__wrap_ieee80211_tx_mgt_cb",
    "__wrap_wDev_record_ftm_data",
    "__wrap_pm_on_beacon_rx",
    "__wrap_pm_on_data_rx",
    "__wrap_pm_on_data_tx",
    "__wrap_pm_on_coex_schm_status_config",
    "__wrap_pm_set_beacon_duration",
    "__wrap_cnx_check_bssid_in_blacklist",
    "__wrap_cnx_add_to_blacklist",
    "__wrap_cnx_remove_from_blacklist",
    "__wrap_cnx_clear_blacklist",
    "__wrap_dbg_read_tx_ppdu",
    "__wrap_dbg_dump_rx_ppdu",
    "__wrap_dbg_dump_rx_sigb",
    "__wrap_wifi_gpio_debug",
    "__wrap_esp_test_tx_enab_statistics",
    "__wrap_esp_test_rx_parse_mu",
    "__wrap_esp_test_rx_process_complete",
    "__wrap_wDev_SnifferRxData",
    "__wrap_wdev_csi_rx_process",
    "__wrap_wDev_ftm_set_t1t4",
    "__wrap_wDev_isNANPktInValidSlot",
    "__wrap_wDev_AppendRxBlocks",
    "wifi_strict_wdev_discard_frame",
    "wifi_strict_pp_rx_proto_proc",
    "wifi_strict_pp_recycle_rx_pkt",
    "wifi_strict_esp_wifi_internal_free_rx_buffer",
    "wifi_strict_rc_get_trc",
    "wifi_strict_rc_update_rx_done",
    "__wrap_wDev_IndicateCtrlFrame",
    "__wrap_wpa_sm_rx_eapol",
    "__wrap_wpa_ap_rx_eapol",
    "__wrap_hal_crypto_set_key_entry",
    "__wrap_wifi_log",
    "__wrap_wifi_assert",
    "__wrap_pp_post",
    "__wrap_ieee80211_timer_process",
    "__wrap_chm_start_op",
    "__wrap_chm_return_home_channel",
    "__esp_scan_op_end",
    "__esp_scan_op_end_end",
    "__wrap_esf_buf_alloc",
    "__wrap_esf_buf_recycle",
    "__wrap_ieee80211_mgmt_output",
    "__wrap_ieee80211_set_tx_pti",
    "__wrap_ieee80211_classify",
    "wifi_strict_ieee80211_align_eb",
    "wifi_strict_ieee80211_crypto_encap",
    "__wrap_ieee80211_search_node",
    "__wrap_cnx_node_alloc",
    "__wrap_cnx_node_search",
    "__wrap_rcGetSched",
    "__wrap_ppTxPkt",
    "__wrap_ets_delay_us",
    "__wrap_vTaskDelay",
    "__wrap_os_sleep",
    "__wrap_sleep",
    "__wrap_usleep",
    "__esp_hostap_sta_join",
    "__esp_hostap_sta_join_end",
    "__esp_wifi_async_wpa2_ap_join",
    "__esp_wifi_async_wpa2_ap_remove",
    "__esp_wifi_async_wpa2_ap_get_peer_spp_msg",
    "__esp_wifi_async_wpa2_ap_init",
    "__esp_wifi_async_wpa2_ap_deinit",
    "__esp_wifi_async_wpa2_ap_get_rsn",
    "__esp_wifi_async_wpa2_sta_txdone",
    "__esp_wpa_sta_connected_cb",
    "__esp_wpa_sta_connected_cb_end",
    "__esp_wpa_sta_disconnected_cb",
    "__esp_wpa_sta_disconnected_cb_end",
    "__esp_wifi_async_wpa2_sta_connected",
    "__esp_wifi_async_wpa2_sta_disconnected",
    "__esp_wifi_async_wpa2_sta_in_4way",
    "__esp_wifi_async_data_rx_sta",
    "__esp_wifi_async_data_rx_ap",
];

// These ROM exports cannot use GNU --wrap because the ROM linker script would
// also assign the generated wrapper name. The late linker fragment aliases the
// public symbol directly to a uniquely named Rust function instead.
// This is the one-action, fail-closed replacement reached by strict PP events
// 0..=4. It must remain executable from internal SRAM, and the final image must
// not contain an instruction which transfers control to the absolute ROM
// `ppProcessTxQ` export.
const REQUIRED_SRAM_CODE: &[&str] = &[
    "process_tx_queue",
    "wifi_strict_env_is_chip",
    "__wrap_wDev_AppendRxBlocks",
    "wifi_strict_wdev_discard_frame",
    "wifi_strict_pp_rx_proto_proc",
    "wifi_strict_pp_recycle_rx_pkt",
    "wifi_strict_esp_wifi_internal_free_rx_buffer",
    "wifi_strict_rc_get_trc",
    "wifi_strict_rc_update_rx_done",
    "__wrap_ppTxPkt",
    "wifi_strict_lmac_rx_done",
    "wifi_strict_wake_internal_consumer",
];
const REQUIRED_RADIO_WAKER_SRAM_CODE: &[&str] = &[
    "wifi_strict_radio_executor_interrupt",
    "wifi_strict_radio_waker_clone",
    "wifi_strict_radio_waker_wake",
    "wifi_strict_radio_waker_wake_by_ref",
    "wifi_strict_radio_waker_drop",
    "wifi_strict_radio_try_suspend_cached_executor",
    "wifi_strict_radio_resume_cached_executor",
];
const RADIO_WAKER_VTABLE: &str = "WIFI_STRICT_RADIO_WAKER_VTABLE";
const RADIO_WAKER_SECTION: &str = ".critical.data.wifi_strict.radio_executor";
const REPLACED_ROOTS_FORBIDDEN_IN_FINAL_CALLS: &[&str] = &[
    "ic_get_next_tbtt",
    "pp_timer_do_process",
    "ppProcessTxQ",
    "pp_default_event_handler",
    "pp_coex_tx_release",
    "ppTxPkt",
    "ppMapTxQueue",
    "ppDequeueTxQ",
    "ppDequeueRxq_Locked",
    "ppRxProtoProc",
    "wDev_DiscardFrame",
    "esp_wifi_internal_free_rx_buffer",
    "rc_get_trc",
    "rcUpdateRxDone",
    "lmacRxDone",
    "pm_on_coex_schm_status_config",
    "pm_set_beacon_duration",
    "cnx_check_bssid_in_blacklist",
    "cnx_add_to_blacklist",
    "cnx_remove_from_blacklist",
    "cnx_clear_blacklist",
];
const INTERNAL_SRAM_START: u64 = 0x2f00_0000;
const INTERNAL_SRAM_END: u64 = 0x3000_0000;

const DIRECT_HEAP_WRAPPERS: [(&str, &str); 4] = [
    ("malloc", "__wrap_malloc"),
    ("calloc", "__wrap_calloc"),
    ("realloc", "__wrap_realloc"),
    ("free", "__wrap_free"),
];

const DIRECT_DELAY_WRAPPERS: [(&str, &str); 5] = [
    ("ets_delay_us", "__wrap_ets_delay_us"),
    ("vTaskDelay", "__wrap_vTaskDelay"),
    ("os_sleep", "__wrap_os_sleep"),
    ("sleep", "__wrap_sleep"),
    ("usleep", "__wrap_usleep"),
];

const FORBIDDEN: &[(&str, &str)] = &[
    ("malloc", "heap"),
    ("calloc", "heap"),
    ("realloc", "heap"),
    ("free", "heap"),
    ("vTaskDelay", "delay"),
    ("ets_delay_us", "delay"),
    ("sleep", "delay"),
    ("usleep", "delay"),
    ("os_sleep", "delay"),
    ("taskYIELD", "scheduler"),
    ("xQueueReceive", "RTOS wait"),
    ("xSemaphoreTake", "RTOS wait"),
    ("xEventGroupWaitBits", "RTOS wait"),
    ("esp_event_post", "event-loop wait/allocation"),
    ("nvs_commit", "flash wait"),
    ("nvs_set_blob", "flash wait/allocation"),
    ("nvs_erase_key", "flash wait"),
    ("puts", "unbounded logging"),
    ("putchar", "unbounded logging"),
    ("printf", "unbounded logging"),
    ("abort", "non-returning"),
    ("__assert_func", "non-returning"),
    ("esp_dport_access_stall_other_cpu_start", "other-core stall"),
    ("dport_access_stall_other_cpu_start", "other-core stall"),
];

// These symbols remain linked for initialization or stock WPA code which the
// strict runtime never enters. Do not waive them merely by name: the final ELF
// is accepted only while every direct call remains in this exact owner set.
// Empty owner sets require zero call instructions in the final image.
const PINNED_DORMANT_FINAL_CALLERS: &[(&str, &[&str])] = &[
    (
        "esp_event_post",
        &["sm_WPA_PTK_PTKCALCNEGOTIATING_Enter.constprop.0"],
    ),
    ("__assert_func", &["wpa_gen_wpa_ie"]),
    (
        "puts",
        &["wifi_osi_funcs_register", "esp_wifi_init_internal"],
    ),
    ("putchar", &[]),
    ("printf", &[]),
];

#[derive(Default)]
struct FunctionInfo {
    direct: BTreeSet<String>,
    indirect_sites: BTreeSet<String>,
    control_flow_cycles: BTreeSet<String>,
    objects: BTreeSet<String>,
}

#[derive(Clone)]
struct Instruction {
    address: u64,
    mnemonic: String,
    target: Option<u64>,
    text: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Violation {
    Forbidden {
        root: String,
        category: &'static str,
        path: Vec<String>,
    },
    Indirect {
        root: String,
        function: String,
        site: String,
        path: Vec<String>,
    },
    ControlFlowCycle {
        root: String,
        function: String,
        site: String,
        path: Vec<String>,
    },
    MissingRoot(String),
    ElfSymbol {
        category: &'static str,
        symbol: String,
    },
}

fn main() -> Result<()> {
    let mut enforce = false;
    let mut verbose = false;
    let mut include_static_binding_init = false;
    let mut include_static_pm_init = false;
    let mut elf = None;
    let mut requested_roots = Vec::<String>::new();
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--enforce" => enforce = true,
            "--verbose" => verbose = true,
            "--include-static-binding-init" => include_static_binding_init = true,
            "--include-static-pm-init" => include_static_pm_init = true,
            "--elf" => {
                elf = Some(PathBuf::from(
                    arguments.next().context("--elf requires a path")?,
                ));
            }
            "--root" => requested_roots.push(arguments.next().context("--root requires a symbol")?),
            _ => bail!("unknown argument: {argument}"),
        }
    }
    let mut roots = if requested_roots.is_empty() {
        ROOTS
            .iter()
            .chain(STRICT_REFERENCE_ROOTS)
            .map(|root| (*root).to_owned())
            .collect()
    } else {
        requested_roots
    };
    if include_static_binding_init {
        for root in STATIC_BINDING_ROOTS {
            if !roots.iter().any(|existing| existing == root) {
                roots.push((*root).to_owned());
            }
        }
    }
    if include_static_pm_init {
        for root in STATIC_PM_INIT_ROOTS {
            if !roots.iter().any(|existing| existing == root) {
                roots.push((*root).to_owned());
            }
        }
    }

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask must be inside the workspace")?
        .to_path_buf();
    let library_dir = workspace.join("esp-wifi-sys-esp32s31/libs");
    let temporary =
        env::temp_dir().join(format!("esp-wifi-s31-strict-audit-{}", std::process::id()));
    if temporary.exists() {
        fs::remove_dir_all(&temporary)?;
    }
    fs::create_dir(&temporary)?;

    let result = run(
        &library_dir,
        elf.as_deref(),
        &temporary,
        &roots,
        enforce,
        verbose,
    );
    fs::remove_dir_all(&temporary)?;
    result
}

fn run(
    library_dir: &Path,
    elf: Option<&Path>,
    temporary: &Path,
    roots: &[String],
    enforce: bool,
    verbose: bool,
) -> Result<()> {
    let graph = build_graph(library_dir, temporary)?;
    let mut violations = audit_graph(&graph, roots);
    if let Some(elf) = elf {
        violations.extend(audit_elf(elf)?);
    }

    print_report(&graph, &violations, roots, elf, verbose);
    if enforce && !violations.is_empty() {
        bail!(
            "strict ESP32-S31 audit rejected {} reachable or final-link paths",
            violations.len()
        );
    }
    Ok(())
}

fn build_graph(library_dir: &Path, temporary: &Path) -> Result<BTreeMap<String, FunctionInfo>> {
    let mut graph = BTreeMap::<String, FunctionInfo>::new();
    let mut archives = fs::read_dir(library_dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "a"))
        .collect::<Vec<_>>();
    archives.sort();

    for archive in archives {
        let archive_name = archive
            .file_stem()
            .and_then(|name| name.to_str())
            .context("archive has no UTF-8 stem")?;
        let output_dir = temporary.join(archive_name);
        fs::create_dir(&output_dir)?;
        checked(
            Command::new("llvm-ar")
                .current_dir(&output_dir)
                .arg("x")
                .arg(
                    archive
                        .canonicalize()
                        .with_context(|| format!("canonicalizing {}", archive.display()))?,
                ),
        )?;

        let mut objects = fs::read_dir(&output_dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "o" || extension == "obj")
            })
            .collect::<Vec<_>>();
        objects.sort();
        for object in objects {
            let object_name = format!(
                "{}[{}]",
                archive
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(archive_name),
                object
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("?")
            );
            let disassembly = text(checked(
                Command::new("llvm-objdump")
                    .arg("-dr")
                    .arg("--no-show-raw-insn")
                    .arg(&object),
            )?)?;
            parse_object(&disassembly, &object_name, &mut graph);
        }
    }
    Ok(graph)
}

fn parse_object(disassembly: &str, object: &str, graph: &mut BTreeMap<String, FunctionInfo>) {
    let mut function = None::<String>;
    let mut instructions = Vec::<Instruction>::new();
    for line in disassembly.lines() {
        if line.starts_with("Disassembly of section ") {
            record_control_flow_cycles(function.as_deref(), &instructions, graph);
            function = None;
            instructions.clear();
            continue;
        }
        if let Some(name) = definition_name(line) {
            if !name.starts_with('.') {
                record_control_flow_cycles(function.as_deref(), &instructions, graph);
                instructions.clear();
                function = Some(normalize_symbol(name));
                if let Some(function) = &function {
                    graph
                        .entry(function.clone())
                        .or_default()
                        .objects
                        .insert(object.to_owned());
                }
            }
            continue;
        }

        let Some(caller) = function.as_ref() else {
            continue;
        };
        if let Some(instruction) = parse_instruction(line) {
            instructions.push(instruction);
        }
        if let Some(target) = direct_relocation_target(line) {
            graph
                .entry(caller.clone())
                .or_default()
                .direct
                .insert(target);
        }
        if let Some(site) = indirect_site(line) {
            graph
                .entry(caller.clone())
                .or_default()
                .indirect_sites
                .insert(site);
        }
    }
    record_control_flow_cycles(function.as_deref(), &instructions, graph);
}

fn definition_name(line: &str) -> Option<&str> {
    let start = line.find('<')? + 1;
    // Demangled Rust symbols may contain nested `<Type<Args>>::method`
    // delimiters. Only the final `>:` terminates the objdump definition.
    let end = line[start..].rfind(">:")? + start;
    line[..start - 1]
        .trim()
        .chars()
        .all(|character| character.is_ascii_hexdigit())
        .then_some(&line[start..end])
}

fn direct_relocation_target(line: &str) -> Option<String> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    let relocation = fields
        .iter()
        .position(|field| matches!(*field, "R_RISCV_CALL" | "R_RISCV_CALL_PLT" | "R_RISCV_JAL"))?;
    let target = *fields.get(relocation + 1)?;
    (!target.starts_with('.')).then(|| normalize_symbol(target))
}

fn indirect_site(line: &str) -> Option<String> {
    if line.contains('<') {
        return None;
    }
    let fields = line.split_whitespace().collect::<Vec<_>>();
    let instruction = fields.get(1)?;
    matches!(*instruction, "jalr" | "jr").then(|| line.trim().to_owned())
}

fn instruction_site_address(site: &str) -> Option<u64> {
    u64::from_str_radix(site.split_whitespace().next()?.trim_end_matches(':'), 16).ok()
}

fn is_pinned_bounded_cycle(function: &str, site: &str) -> bool {
    instruction_site_address(site)
        .is_some_and(|address| PINNED_BOUNDED_CYCLE_SITES.contains(&(function, address)))
}

fn pinned_indirect_site_target(function: &str, site: &str) -> Option<&'static str> {
    let address = instruction_site_address(site)?;
    PINNED_INDIRECT_SITES
        .iter()
        .find_map(|(caller, pinned_address, target)| {
            (*caller == function && *pinned_address == address).then_some(*target)
        })
}

fn is_invariant_excluded_indirect_site(function: &str, site: &str) -> bool {
    instruction_site_address(site)
        .is_some_and(|address| INVARIANT_EXCLUDED_INDIRECT_SITES.contains(&(function, address)))
}

fn parse_instruction(line: &str) -> Option<Instruction> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    let address = u64::from_str_radix(fields.first()?.trim_end_matches(':'), 16).ok()?;
    let mnemonic = *fields.get(1)?;
    if mnemonic.starts_with("R_") {
        return None;
    }
    let has_control_target = mnemonic == "j" || (mnemonic.starts_with('b') && mnemonic != "break");
    let target = has_control_target
        .then(|| {
            fields
                .iter()
                .skip(2)
                .find_map(|field| field.strip_prefix("0x"))
                .and_then(|target| u64::from_str_radix(target.trim_end_matches(','), 16).ok())
        })
        .flatten();
    Some(Instruction {
        address,
        mnemonic: mnemonic.to_owned(),
        target,
        text: line.trim().to_owned(),
    })
}

fn record_control_flow_cycles(
    function: Option<&str>,
    instructions: &[Instruction],
    graph: &mut BTreeMap<String, FunctionInfo>,
) {
    let Some(function) = function else {
        return;
    };
    let positions = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.address, index))
        .collect::<BTreeMap<_, _>>();
    let mut edges = vec![Vec::<usize>::new(); instructions.len()];

    for (index, instruction) in instructions.iter().enumerate() {
        let conditional =
            instruction.mnemonic.starts_with('b') && !instruction.mnemonic.starts_with("break");
        let unconditional = instruction.mnemonic == "j";
        if conditional || unconditional {
            if let Some(target) = instruction.target.and_then(|target| positions.get(&target)) {
                edges[index].push(*target);
            }
        }
        let terminates =
            unconditional || matches!(instruction.mnemonic.as_str(), "ret" | "jr" | "tail");
        if !terminates && index + 1 < instructions.len() {
            edges[index].push(index + 1);
        }
    }

    for (source, instruction) in instructions.iter().enumerate() {
        let Some(target_address) = instruction.target else {
            continue;
        };
        if target_address > instruction.address {
            continue;
        }
        let Some(&target) = positions.get(&target_address) else {
            continue;
        };
        if is_reachable(target, source, &edges) {
            graph
                .entry(function.to_owned())
                .or_default()
                .control_flow_cycles
                .insert(instruction.text.clone());
        }
    }
}

fn is_reachable(start: usize, target: usize, edges: &[Vec<usize>]) -> bool {
    let mut pending = vec![start];
    let mut visited = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if node == target {
            return true;
        }
        if visited.insert(node) {
            pending.extend(edges[node].iter().copied());
        }
    }
    false
}

fn normalize_symbol(symbol: &str) -> String {
    symbol
        .split(['+', '@'])
        .next()
        .unwrap_or(symbol)
        .trim()
        .to_owned()
}

fn audit_graph(graph: &BTreeMap<String, FunctionInfo>, roots: &[String]) -> BTreeSet<Violation> {
    let forbidden = FORBIDDEN.iter().copied().collect::<BTreeMap<_, _>>();
    let mut violations = BTreeSet::new();
    for root in roots {
        let root = root.as_str();
        if !graph.contains_key(root) {
            violations.insert(Violation::MissingRoot(root.to_owned()));
            continue;
        }

        let mut queue = VecDeque::from([root.to_owned()]);
        let mut predecessor = BTreeMap::<String, String>::new();
        let mut visited = BTreeSet::new();
        while let Some(function) = queue.pop_front() {
            if !visited.insert(function.clone()) {
                continue;
            }
            let path = reconstruct_path(root, &function, &predecessor);
            if let Some(category) = forbidden.get(function.as_str()) {
                violations.insert(Violation::Forbidden {
                    root: root.to_owned(),
                    category,
                    path,
                });
                continue;
            }
            if function != root && WRAPPED_VENDOR_BOUNDARIES.contains(&function.as_str()) {
                continue;
            }
            let Some(info) = graph.get(&function) else {
                continue;
            };
            let pinned_indirect = PINNED_INDIRECT_TARGETS
                .iter()
                .find_map(|(caller, target)| (*caller == function).then_some(*target));
            let excludes_all_indirects = INVARIANT_EXCLUDED_INDIRECTS.contains(&function.as_str());
            for site in &info.indirect_sites {
                if let Some(target) = pinned_indirect_site_target(&function, site) {
                    if !predecessor.contains_key(target) && target != root {
                        predecessor.insert(target.to_owned(), function.clone());
                    }
                    queue.push_back(target.to_owned());
                } else if pinned_indirect.is_none()
                    && !excludes_all_indirects
                    && !is_invariant_excluded_indirect_site(&function, site)
                {
                    violations.insert(Violation::Indirect {
                        root: root.to_owned(),
                        function: function.clone(),
                        site: site.clone(),
                        path: path.clone(),
                    });
                }
            }
            for site in &info.control_flow_cycles {
                if !is_pinned_bounded_cycle(&function, site) {
                    violations.insert(Violation::ControlFlowCycle {
                        root: root.to_owned(),
                        function: function.clone(),
                        site: site.clone(),
                        path: path.clone(),
                    });
                }
            }
            if let Some(target) = pinned_indirect {
                if !predecessor.contains_key(target) && target != root {
                    predecessor.insert(target.to_owned(), function.clone());
                }
                queue.push_back(target.to_owned());
            }
            for target in &info.direct {
                if !predecessor.contains_key(target) && target != root {
                    predecessor.insert(target.clone(), function.clone());
                }
                queue.push_back(target.clone());
            }
        }
    }
    violations
}

fn reconstruct_path(
    root: &str,
    function: &str,
    predecessor: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut path = vec![function.to_owned()];
    while path.last().is_some_and(|current| current != root) {
        let Some(previous) = path.last().and_then(|current| predecessor.get(current)) else {
            break;
        };
        path.push(previous.clone());
    }
    path.reverse();
    path
}

fn audit_elf(elf: &Path) -> Result<BTreeSet<Violation>> {
    let symbols = text(checked(
        Command::new("llvm-nm").arg("-C").arg("-g").arg(elf),
    )?)?;
    let forbidden = FORBIDDEN.iter().copied().collect::<BTreeMap<_, _>>();
    let direct_heap_wrappers = DIRECT_HEAP_WRAPPERS
        .iter()
        .copied()
        .collect::<BTreeMap<_, _>>();
    let linked_symbol_kinds = symbols
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let symbol = normalize_symbol(fields.last()?);
            let kind = (*fields.get(fields.len().checked_sub(2)?)?).to_owned();
            Some((symbol, kind))
        })
        .collect::<BTreeMap<_, _>>();
    let linked_symbol_addresses = symbols
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 3 {
                return None;
            }
            let address = u64::from_str_radix(fields[0], 16).ok()?;
            Some((normalize_symbol(fields.last()?), address))
        })
        .collect::<BTreeMap<_, _>>();
    let mut violations = BTreeSet::new();

    let all_symbols = text(checked(
        Command::new("llvm-nm").arg("-C").arg("-n").arg(elf),
    )?)?;
    let all_linked_symbols = all_symbols
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 3 {
                return None;
            }
            let address = u64::from_str_radix(fields[0], 16).ok()?;
            let kind = (*fields.get(fields.len().checked_sub(2)?)?).to_owned();
            Some((normalize_symbol(fields.last()?), (kind, address)))
        })
        .collect::<BTreeMap<_, _>>();
    let linked_code_locations = parse_linked_code_locations(&all_symbols);
    for required in REQUIRED_SRAM_CODE {
        match all_linked_symbols.get(*required) {
            None => {
                violations.insert(Violation::ElfSymbol {
                    category: "missing strict SRAM code",
                    symbol: (*required).to_owned(),
                });
            }
            Some((kind, address)) if !is_internal_sram_code(kind, *address) => {
                violations.insert(Violation::ElfSymbol {
                    category: "strict code outside internal SRAM",
                    symbol: format!("{required}@0x{address:08x}"),
                });
            }
            Some(_) => {}
        }
    }
    let has_radio_waker = all_linked_symbols.contains_key(RADIO_WAKER_VTABLE);
    if has_radio_waker {
        for required in REQUIRED_RADIO_WAKER_SRAM_CODE {
            match all_linked_symbols.get(*required) {
                None => {
                    violations.insert(Violation::ElfSymbol {
                        category: "missing strict radio waker code",
                        symbol: (*required).to_owned(),
                    });
                }
                Some((kind, address)) if !is_internal_sram_code(kind, *address) => {
                    violations.insert(Violation::ElfSymbol {
                        category: "strict radio waker outside internal SRAM",
                        symbol: format!("{required}@0x{address:08x}"),
                    });
                }
                Some(_) => {}
            }
        }

        match read_radio_waker_vtable(elf) {
            Ok(words) => {
                let expected = [
                    "wifi_strict_radio_waker_clone",
                    "wifi_strict_radio_waker_wake",
                    "wifi_strict_radio_waker_wake_by_ref",
                    "wifi_strict_radio_waker_drop",
                ]
                .map(|symbol| linked_symbol_addresses.get(symbol).copied());
                if expected.iter().any(Option::is_none)
                    || words
                        != expected.map(|address| {
                            u32::try_from(address.unwrap_or_default()).unwrap_or_default()
                        })
                {
                    violations.insert(Violation::ElfSymbol {
                        category: "invalid strict radio waker vtable",
                        symbol: format!(
                            "{RADIO_WAKER_VTABLE}={words:08x?}, expected={expected:08x?}"
                        ),
                    });
                }
            }
            Err(error) => {
                violations.insert(Violation::ElfSymbol {
                    category: "unreadable strict radio waker vtable",
                    symbol: error.to_string(),
                });
            }
        }
    }

    let disassembly = text(checked(
        Command::new("llvm-objdump")
            .arg("-d")
            .arg("-C")
            .arg("--no-show-raw-insn")
            .arg(elf),
    )?)?;
    let (_, command_claim_cycles) = final_command_claim_cycles(&disassembly);
    // A final image without a RadioCommandQueue monomorph contains no command
    // claim which could retry. When one is linked, inspect every generated
    // body below and reject any LR/SC or other control-flow cycle.
    for (function, site) in command_claim_cycles {
        violations.insert(Violation::ElfSymbol {
            category: "retrying strict command claim",
            symbol: format!("{function}: {site}"),
        });
    }
    for replaced in REPLACED_ROOTS_FORBIDDEN_IN_FINAL_CALLS {
        if calls_symbol(&disassembly, replaced) {
            violations.insert(Violation::ElfSymbol {
                category: "call to replaced vendor root",
                symbol: (*replaced).to_owned(),
            });
        }
    }
    for (_, wrapper) in DIRECT_HEAP_WRAPPERS {
        let violation = match linked_symbol_kinds.get(wrapper) {
            None => Some(Violation::ElfSymbol {
                category: "missing strict direct-heap wrapper",
                symbol: wrapper.to_owned(),
            }),
            Some(kind) if !is_code_symbol_kind(kind) => Some(Violation::ElfSymbol {
                category: "non-code strict direct-heap wrapper",
                symbol: wrapper.to_owned(),
            }),
            Some(_) => None,
        };
        violations.extend(violation);
    }
    for wrapper in REQUIRED_RUNTIME_WRAPPERS {
        let violation = match linked_symbol_kinds.get(*wrapper) {
            None => Some(Violation::ElfSymbol {
                category: "missing strict runtime wrapper",
                symbol: (*wrapper).to_owned(),
            }),
            Some(kind) if !is_code_symbol_kind(kind) => Some(Violation::ElfSymbol {
                category: "non-code strict runtime wrapper",
                symbol: (*wrapper).to_owned(),
            }),
            Some(_) => None,
        };
        violations.extend(violation);
    }
    for (public, replacement) in REQUIRED_RUNTIME_ALIASES {
        let public_address = linked_symbol_addresses.get(*public);
        let replacement_address = linked_symbol_addresses.get(*replacement);
        let replacement_is_code = linked_symbol_kinds
            .get(*replacement)
            .is_some_and(|kind| is_code_symbol_kind(kind));
        if public_address.is_none() || public_address != replacement_address || !replacement_is_code
        {
            violations.insert(Violation::ElfSymbol {
                category: "missing or mismatched strict runtime alias",
                symbol: format!("{public}={replacement}"),
            });
        }
    }
    for line in symbols.lines() {
        let Some(symbol) = line.split_whitespace().last() else {
            continue;
        };
        let symbol = normalize_symbol(symbol);
        if let Some(category) = forbidden.get(symbol.as_str()) {
            if *category == "heap"
                && direct_heap_wrappers
                    .get(symbol.as_str())
                    .and_then(|wrapper| linked_symbol_kinds.get(*wrapper))
                    .is_some_and(|kind| is_code_symbol_kind(kind))
            {
                continue;
            }
            if *category == "delay"
                && DIRECT_DELAY_WRAPPERS
                    .iter()
                    .find(|(entry, _)| *entry == symbol)
                    .is_some_and(|(_, wrapper)| {
                        linked_symbol_kinds
                            .get(*wrapper)
                            .is_some_and(|kind| is_code_symbol_kind(kind))
                            && (symbol.as_str() != "ets_delay_us"
                                || linked_symbol_addresses.get(symbol.as_str())
                                    == linked_symbol_addresses.get(*wrapper))
                    })
            {
                continue;
            }
            if PINNED_DORMANT_FINAL_CALLERS
                .iter()
                .find(|(entry, _)| *entry == symbol)
                .is_some_and(|(_, allowed)| {
                    final_call_owners(&disassembly, &symbol, &linked_code_locations).is_some_and(
                        |owners| owners.iter().all(|owner| allowed.contains(&owner.as_str())),
                    )
                })
            {
                continue;
            }
            violations.insert(Violation::ElfSymbol { category, symbol });
        }
        // Do not reject a replaced entry merely because its symbol exists.
        // Strict wrappers deliberately retain `__real_*` delegation for init,
        // and ROM entries alias their public names to the wrapper while
        // pinning `__real_*` to absolute ROM addresses. Wrapper presence is
        // checked above; runtime reachability is checked from the pinned
        // archive graph. Symbol-table presence alone cannot distinguish an
        // inbound bypass from valid initialization delegation.
    }
    Ok(violations)
}

fn is_code_symbol_kind(kind: &str) -> bool {
    matches!(kind, "T" | "t" | "W" | "w")
}

fn is_internal_sram_code(kind: &str, address: u64) -> bool {
    is_code_symbol_kind(kind) && (INTERNAL_SRAM_START..INTERNAL_SRAM_END).contains(&address)
}

fn read_radio_waker_vtable(elf: &Path) -> Result<[u32; 4]> {
    let dump = text(checked(
        Command::new("llvm-readelf")
            .arg("-x")
            .arg(RADIO_WAKER_SECTION)
            .arg(elf),
    )?)?;
    let line = dump
        .lines()
        .find(|line| {
            line.split_whitespace()
                .next()
                .is_some_and(|field| field.starts_with("0x"))
        })
        .context("radio waker section has no data")?;
    let mut words = [0_u32; 4];
    let fields = line.split_whitespace().skip(1).take(4).collect::<Vec<_>>();
    if fields.len() != words.len() {
        bail!("radio waker vtable is shorter than four words");
    }
    for (word, field) in words.iter_mut().zip(fields) {
        *word = parse_readelf_le_word(field)?;
    }
    Ok(words)
}

fn parse_readelf_le_word(field: &str) -> Result<u32> {
    if field.len() != 8 || !field.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid llvm-readelf word `{field}`");
    }
    let mut bytes = [0_u8; 4];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&field[index * 2..index * 2 + 2], 16)?;
    }
    Ok(u32::from_le_bytes(bytes))
}

fn parse_linked_code_locations(symbols: &str) -> Vec<(u64, String)> {
    let mut locations = symbols
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 3 || !is_code_symbol_kind(fields[1]) {
                return None;
            }
            let address = u64::from_str_radix(fields[0], 16).ok()?;
            let name = fields[2..].join(" ");
            (!name.starts_with('.')).then_some((address, normalize_symbol(&name)))
        })
        .collect::<Vec<_>>();
    locations.sort_by_key(|(address, _)| *address);
    locations
}

fn final_call_owners(
    disassembly: &str,
    target: &str,
    code_locations: &[(u64, String)],
) -> Option<BTreeSet<String>> {
    let reference = format!("<{target}>");
    let mut owners = BTreeSet::new();
    for line in disassembly.lines().filter(|line| {
        line.contains(&reference) && !line.trim_end().ends_with(&format!("{reference}:"))
    }) {
        let address = line
            .split_once(':')
            .and_then(|(address, _)| u64::from_str_radix(address.trim(), 16).ok())?;
        let owner = code_locations
            .iter()
            .rev()
            .find(|(start, _)| *start <= address)
            .map(|(_, owner)| owner.clone())?;
        owners.insert(owner);
    }
    Some(owners)
}

fn calls_symbol(disassembly: &str, symbol: &str) -> bool {
    let reference = format!("<{symbol}>");
    disassembly.lines().any(|line| {
        line.contains(&reference) && !line.trim_end().ends_with(&format!("{reference}:"))
    })
}

fn final_command_claim_cycles(disassembly: &str) -> (bool, Vec<(String, String)>) {
    let mut graph = BTreeMap::new();
    parse_object(disassembly, "final ELF", &mut graph);
    let mut found = false;
    let mut cycles = Vec::new();
    for (function, info) in graph {
        if function.contains("RadioCommandQueue") && function.ends_with("::try_submit") {
            found = true;
            cycles.extend(
                info.control_flow_cycles
                    .into_iter()
                    .map(|site| (function.clone(), site)),
            );
        }
    }
    (found, cycles)
}

fn print_report(
    graph: &BTreeMap<String, FunctionInfo>,
    violations: &BTreeSet<Violation>,
    roots: &[String],
    elf: Option<&Path>,
    verbose: bool,
) {
    println!("# ESP32-S31 strict no-wait/no-heap audit\n");
    println!("- roots: {} (`{}`)", roots.len(), roots.join("`, `"));
    println!(
        "- replaced vendor roots: `{}`",
        REPLACED_VENDOR_ROOTS.join("`, `")
    );
    println!(
        "- ownership debt: {} fallback / {} stateful-or-unproven / {} temporary MMIO",
        RUST_BOUNDARIES_WITH_VENDOR_FALLBACK.len(),
        STATEFUL_OR_UNPROVEN_RUNTIME_ROOTS.len(),
        TEMPORARY_EVIDENCED_MMIO_ROOTS.len()
    );
    println!("- discovered functions: {}", graph.len());
    println!("- violations: {}", violations.len());
    if let Some(elf) = elf {
        println!("- final ELF: `{}`", elf.display());
    }
    println!(
        "\nAn indirect `jalr`/`jr` and a control-flow cycle are rejected until their target or bound is explicitly proven.\n"
    );

    let mut categories = BTreeMap::<&str, usize>::new();
    let mut roots = BTreeMap::<&str, usize>::new();
    for violation in violations {
        match violation {
            Violation::Forbidden { root, category, .. } => {
                *categories.entry(category).or_default() += 1;
                *roots.entry(root).or_default() += 1;
            }
            Violation::Indirect { root, .. } => {
                *categories.entry("unproven indirect call").or_default() += 1;
                *roots.entry(root).or_default() += 1;
            }
            Violation::ControlFlowCycle { root, .. } => {
                *categories.entry("unproven control-flow cycle").or_default() += 1;
                *roots.entry(root).or_default() += 1;
            }
            Violation::MissingRoot(root) => {
                *categories.entry("missing root").or_default() += 1;
                *roots.entry(root).or_default() += 1;
            }
            Violation::ElfSymbol { category, .. } => {
                *categories.entry(category).or_default() += 1;
            }
        }
    }
    println!("## Summary by category\n");
    for (category, count) in categories {
        println!("- {category}: {count}");
    }
    println!("\n## Summary by root\n");
    for (root, count) in roots {
        println!("- `{root}`: {count}");
    }
    println!("\n## Paths\n");

    let limit = if verbose { usize::MAX } else { 64 };
    for violation in violations.iter().take(limit) {
        match violation {
            Violation::Forbidden {
                root,
                category,
                path,
            } => println!(
                "- FORBIDDEN `{category}` from `{root}`: `{}`",
                path.join(" -> ")
            ),
            Violation::Indirect {
                root,
                function,
                site,
                path,
            } => println!(
                "- INDIRECT from `{root}` at `{function}` (`{site}`): `{}`",
                path.join(" -> ")
            ),
            Violation::ControlFlowCycle {
                root,
                function,
                site,
                path,
            } => println!(
                "- CONTROL-FLOW-CYCLE from `{root}` at `{function}` (`{site}`): `{}`",
                path.join(" -> ")
            ),
            Violation::MissingRoot(root) => println!("- MISSING ROOT `{root}`"),
            Violation::ElfSymbol { category, symbol } => {
                println!("- FINAL ELF `{category}` symbol: `{symbol}`")
            }
        }
    }
    if violations.len() > limit {
        println!(
            "\n- ... {} additional violations omitted; rerun with `--verbose`.",
            violations.len() - limit
        );
    }
}

fn checked(command: &mut Command) -> Result<Output> {
    let output = command
        .output()
        .with_context(|| format!("running {command:?}"))?;
    if !output.status.success() {
        bail!(
            "command failed: {command:?}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output)
}

fn text(output: Output) -> Result<String> {
    String::from_utf8(output.stdout).context("tool output was not UTF-8")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        calls_symbol, definition_name, direct_relocation_target, final_call_owners,
        final_command_claim_cycles, indirect_site, is_code_symbol_kind, is_internal_sram_code,
        is_invariant_excluded_indirect_site, is_pinned_bounded_cycle, parse_linked_code_locations,
        parse_object, parse_readelf_le_word, pinned_indirect_site_target,
    };

    #[test]
    fn parses_function_and_call_relocations() {
        assert_eq!(
            definition_name("00000000 <ppProcessTxQ>:"),
            Some("ppProcessTxQ")
        );
        assert_eq!(
            direct_relocation_target("  24: R_RISCV_CALL_PLT ets_delay_us+0"),
            Some("ets_delay_us".to_owned())
        );
        assert!(direct_relocation_target("  24: R_RISCV_BRANCH .L2+0").is_none());
    }

    #[test]
    fn only_unresolved_register_calls_are_indirect() {
        assert!(indirect_site("  18:       jalr a5").is_some());
        assert!(indirect_site("  18:       jalr ra <function+0x4>").is_none());
    }

    #[test]
    fn parses_little_endian_readelf_words() {
        assert_eq!(parse_readelf_le_word("b80d002f").unwrap(), 0x2f00_0db8);
        assert!(parse_readelf_le_word("not-hex!").is_err());
    }

    #[test]
    fn bounded_cycle_proofs_are_instruction_specific() {
        assert!(is_pinned_bounded_cycle(
            "phy_set_tx_gain_mem_new",
            "aa: bne a5, s8, 0x94 <.L10>"
        ));
        assert!(!is_pinned_bounded_cycle(
            "phy_set_tx_gain_mem_new",
            "ac: j 0xac <.Lassert>"
        ));
        assert!(!is_pinned_bounded_cycle(
            "different_function",
            "aa: bne a5, s8, 0x94 <.L10>"
        ));
    }

    #[test]
    fn indirect_target_proofs_are_instruction_specific() {
        assert_eq!(
            pinned_indirect_site_target("wDev_ProcessRxSucData", "5fe: jalr a5"),
            Some("wifi_strict_env_is_chip")
        );
        assert_eq!(
            pinned_indirect_site_target("wDev_ProcessRxSucData", "296: jalr a4"),
            None
        );
        assert_eq!(
            pinned_indirect_site_target("different_function", "5fe: jalr a5"),
            None
        );
    }

    #[test]
    fn indirect_invariant_exclusions_are_instruction_specific() {
        assert!(is_invariant_excluded_indirect_site(
            "wDev_ProcessRxSucData",
            "296: jalr a4"
        ));
        assert!(!is_invariant_excluded_indirect_site(
            "wDev_ProcessRxSucData",
            "5fe: jalr a5"
        ));
        assert!(!is_invariant_excluded_indirect_site(
            "different_function",
            "296: jalr a4"
        ));
    }

    #[test]
    fn absolute_rom_alias_is_not_accepted_as_a_wrapper() {
        assert!(is_code_symbol_kind("T"));
        assert!(is_code_symbol_kind("W"));
        assert!(!is_code_symbol_kind("A"));
    }

    #[test]
    fn final_elf_rejects_only_calls_to_replaced_rom_root() {
        let disassembly = "2f003828 <replacement>:\n\
                           2f00382c: jalr ra <ppProcessTxQ>\n";
        assert!(calls_symbol(disassembly, "ppProcessTxQ"));
        assert!(!calls_symbol("2f800f8c <ppProcessTxQ>:\n", "ppProcessTxQ"));
        assert!(calls_symbol(
            "40000000: jal ra <pm_set_beacon_duration>\n",
            "pm_set_beacon_duration"
        ));
        assert!(calls_symbol(
            "2f000100: jal ra <pp_timer_do_process>\n",
            "pp_timer_do_process"
        ));
        assert!(calls_symbol(
            "40000000: jal ra <ic_get_next_tbtt>\n",
            "ic_get_next_tbtt"
        ));
    }

    #[test]
    fn dormant_final_calls_are_bound_to_exact_owners() {
        let symbols = "40001000 T init_only\n\
                       40001100 t dormant_wpa_state\n\
                       40002000 T next_function\n";
        let locations = parse_linked_code_locations(symbols);
        let disassembly = "40001000 <init_only>:\n\
                           40001020: jal ra <puts>\n\
                           40001100 <dormant_wpa_state>:\n\
                           40001140: jal ra <esp_event_post>\n";
        assert_eq!(
            final_call_owners(disassembly, "puts", &locations),
            Some(["init_only".to_owned()].into_iter().collect())
        );
        assert_eq!(
            final_call_owners(disassembly, "esp_event_post", &locations),
            Some(["dormant_wpa_state".to_owned()].into_iter().collect())
        );
        assert_eq!(
            final_call_owners(disassembly, "printf", &locations),
            Some(Default::default())
        );
    }

    #[test]
    fn strict_queue_action_must_be_code_in_internal_sram() {
        assert!(is_internal_sram_code("t", 0x2f00_3828));
        assert!(!is_internal_sram_code("A", 0x2f00_3828));
        assert!(!is_internal_sram_code("t", 0x400c_5abc));
    }

    #[test]
    fn distinguishes_cycles_from_backward_layout_edges() {
        let mut graph = BTreeMap::new();
        parse_object(
            "Disassembly of section .text.loop:\n\
             00000000 <looping>:\n\
                    0: li a0, 0x1\n\
                    2: beqz a0, 0x8 <.Lexit>\n\
                    4: addi a0, a0, -0x1\n\
                    6: j 0x2 <.Lloop>\n\
             00000008 <.Lexit>:\n\
                    8: ret\n\
             Disassembly of section .text.layout:\n\
             00000000 <layout>:\n\
                    0: j 0x6 <.Lmerge>\n\
                    2: ret\n\
             00000006 <.Lmerge>:\n\
                    6: beqz a0, 0x2 <.Lreturn>\n\
                    8: ret\n",
            "test.o",
            &mut graph,
        );
        assert_eq!(graph["looping"].control_flow_cycles.len(), 1);
        assert!(graph["layout"].control_flow_cycles.is_empty());
    }

    #[test]
    fn final_command_claim_rejects_lr_sc_retry_but_accepts_one_attempt() {
        let retrying = r#"
00000010 <esp::command::RadioCommandQueue<u8, 2>::try_submit>:
      10: lr.w a0, (a1)
      14: sc.w a2, a3, (a1)
      18: bnez a2, 0x10
      1c: ret
"#;
        let (found, cycles) = final_command_claim_cycles(retrying);
        assert!(found);
        assert_eq!(cycles.len(), 1);

        let one_attempt = r#"
00000010 <esp::command::RadioCommandQueue<u8, 2>::try_submit>:
      10: lr.w a0, (a1)
      14: bne a0, a2, 0x20
      18: sc.w a3, a4, (a1)
      1c: j 0x24
      20: li a3, 1
      24: ret
"#;
        let (found, cycles) = final_command_claim_cycles(one_attempt);
        assert!(found);
        assert!(cycles.is_empty());
    }

    #[test]
    fn data_immediates_are_not_control_flow_targets() {
        let mut graph = BTreeMap::new();
        parse_object(
            "Disassembly of section .text.no_loop:\n\
             00000000 <no_loop>:\n\
                    0: li a0, 0x0\n\
                    2: lui a1, 0x0\n\
                    4: ret\n",
            "test.o",
            &mut graph,
        );
        assert!(graph["no_loop"].control_flow_cycles.is_empty());
    }
}
