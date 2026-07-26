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

// Exact store pairs in the pinned net80211_data_ptr_init (first 12) and
// wdev_data_init (remaining 31) disassemblies.
const ROM_ABI_BACKINGS: &[(&str, &str)] = &[
    ("g_wifi_nvs", "s_wifi_nvs"),
    ("g_scan", "gScanStruct"),
    ("g_chm", "gChmCxt"),
    ("g_ic_ptr", "g_ic"),
    ("g_hmac_cnt_ptr", "g_hmac_cnt"),
    ("g_tx_cacheq_ptr", "s_tx_cacheq"),
    ("g_mac_sleep_en_ptr", "g_mac_sleep_en"),
    ("g_esp_mesh_quick_funcs_ptr", "esp_mesh_quick_funcs"),
    ("g_mesh_init_ps_type_ptr", "g_mesh_init_ps_type"),
    ("g_mesh_is_started_ptr", "g_mesh_is_started"),
    ("g_mesh_is_root_ptr", "g_mesh_is_root"),
    ("g_mesh_topology_ptr", "g_mesh_topology"),
    ("pTxRx", "TxRxCxt"),
    ("lmacConfMib_ptr", "lmacConfMib"),
    ("wDevCtrl_ptr", "wDevCtrl"),
    ("wDevMacSleep_ptr", "wDevMacSleep"),
    ("g_lmac_cnt_ptr", "g_lmac_cnt"),
    ("pp_sig_cnt_ptr", "pp_sig_cnt"),
    ("g_wifi_menuconfig_ptr", "g_wifi_menuconfig"),
    ("g_eb_list_desc_ptr", "g_eb_list_desc"),
    ("s_fragment_ptr", "s_fragment"),
    ("if_ctrl_ptr", "if_ctrl"),
    ("ap_no_lr_ptr", "ap_no_lr"),
    ("rcLoRaSchedTbl_ptr", "rcLoRaSchedTbl"),
    ("rc11NSchedTbl_ptr", "rc11NSchedTbl"),
    ("rc11BSchedTbl_ptr", "rc11BSchedTbl"),
    ("BasicOFDMSched_ptr", "BasicOFDMSched"),
    ("trc_ctl_ptr", "trc_ctl"),
    ("g_pm_cfg_ptr", "g_pm_cfg"),
    ("g_pm_ptr", "g_pm"),
    ("g_txop_queue_status_ptr", "wifi_strict_txop_queue_status"),
    ("g_pm_cnt_ptr", "g_pm_cnt"),
    ("g_pp_timer_info_ptr", "g_pp_timer_info"),
    ("g_rts_threshold_bytes_ptr", "g_rts_threshold_bytes"),
    ("g_pm_twt_ptr", "g_pm_twt"),
    ("g_he_max_apep_length_tab_ptr", "g_he_max_apep_length_tab"),
    ("g_wdev_dbg_rx_ptr", "g_wdev_dbg_rx"),
    ("s_pm_beacon_offset_ptr", "s_pm_beacon_offset"),
    ("s_pm_beacon_offset_config_ptr", "s_pm_beacon_offset_config"),
    ("s_tbttstart_ptr", "s_tbttstart"),
    ("s_offchan_tx_progress_in_ptr", "offchan_tx_progress_in"),
    ("g_offchan_packet_lifetime_ptr", "g_offchan_packet_lifetime"),
    ("g_send_wake_null_timer_ptr", "send_wake_null_timer"),
];

// Public data names still referenced by pinned vendor objects, but whose
// storage and initial value are owned by Rust. These aliases are only removed
// from the blob-state inventory after their final-link identity, size and SRAM
// placement have been validated.
const RUST_OWNED_ABI_DATA_ALIASES: &[(&str, &str, u64)] =
    &[("g_phyFuns", "wifi_strict_phy_rom_function_table_binding", 4)];

// The primary deblob profile performs a full cold calibration and owns any
// future calibration record in Rust. It deliberately omits vendor formatting,
// logging, and calibration-record persistence rather than porting them.
const WIFI_FULL_CAL_OMITTED_COLD_BOUNDARIES: &[&str] = &[
    "phy_printf",
    "syslog",
    "phy_get_rf_cal_version",
    "phy_rfcal_data_check_new",
    "phy_rf_cal_data_backup_new",
    "phy_rf_cal_data_recovery_new",
];

#[derive(Clone)]
struct Symbol {
    address: u64,
    size: u64,
    kind: char,
}

#[derive(Default)]
struct ArchiveInventory {
    data_owners: BTreeMap<String, BTreeSet<String>>,
    function_owners: BTreeMap<String, BTreeSet<String>>,
    function_sizes: BTreeMap<String, BTreeMap<String, u64>>,
    calls: BTreeMap<String, BTreeSet<String>>,
    references: BTreeMap<String, BTreeSet<String>>,
}

struct Section {
    name: String,
    address: u64,
    size: u64,
}

fn main() -> Result<()> {
    let mut elf = None;
    let mut rom_elf = None;
    let mut write = None;
    let mut enforce_primary_baseline = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--elf" => {
                elf = Some(PathBuf::from(
                    arguments.next().context("--elf requires a path")?,
                ));
            }
            "--rom-elf" => {
                rom_elf = Some(PathBuf::from(
                    arguments.next().context("--rom-elf requires a path")?,
                ));
            }
            "--write" => {
                write = Some(PathBuf::from(
                    arguments.next().context("--write requires a path")?,
                ));
            }
            "--enforce-primary-baseline" => enforce_primary_baseline = true,
            _ => bail!("unknown argument: {argument}"),
        }
    }
    let elf = elf.context("--elf is required")?;
    if !elf.is_file() {
        bail!("ELF does not exist: {}", elf.display());
    }
    if rom_elf.as_ref().is_some_and(|path| !path.is_file()) {
        bail!(
            "ROM ELF does not exist: {}",
            rom_elf.as_ref().unwrap().display()
        );
    }

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask must be inside the workspace")?
        .to_path_buf();
    let library_dir = workspace.join("esp-wifi-sys-esp32s31/libs");
    let report = build_report(
        &library_dir,
        &elf,
        rom_elf.as_deref(),
        enforce_primary_baseline,
    )?;
    if let Some(path) = write {
        let path = if path.is_absolute() {
            path
        } else {
            workspace.join(path)
        };
        fs::write(&path, &report)?;
        println!("wrote {}", path.display());
    } else {
        print!("{report}");
    }
    Ok(())
}

fn build_report(
    library_dir: &Path,
    elf: &Path,
    rom_elf: Option<&Path>,
    enforce_primary_baseline: bool,
) -> Result<String> {
    let inventory = inventory_archives(library_dir)?;
    let final_symbols = parse_posix_symbols(&text(checked(
        Command::new("llvm-nm")
            .arg("-S")
            .arg("-P")
            .arg("--defined-only")
            .arg(elf),
    )?)?);
    let sections = parse_sections(&text(checked(
        Command::new("llvm-readelf").arg("-S").arg("-W").arg(elf),
    )?)?);
    let rom_symbols = match rom_elf {
        Some(path) => parse_posix_symbols(&text(checked(
            Command::new("llvm-nm")
                .arg("-S")
                .arg("-P")
                .arg("--defined-only")
                .arg(path),
        )?)?),
        None => BTreeMap::new(),
    };
    let rust_owned_data_aliases = validate_rust_owned_data_aliases(&final_symbols, &sections)?;
    let reachable = reachable_vendor_functions(&inventory.calls);
    let cold_phy_reachable = reachable_from_roots(&inventory.calls, &["register_chipv7_phy"], &[]);
    let wifi_full_cal_reachable = reachable_from_roots(
        &inventory.calls,
        &["register_chipv7_phy"],
        WIFI_FULL_CAL_OMITTED_COLD_BOUNDARIES,
    );
    let runtime_archive_functions = archive_function_rows(
        &reachable,
        &inventory.function_owners,
        &inventory.function_sizes,
    );
    let runtime_external_frontier = reachable
        .iter()
        .filter(|name| !inventory.function_owners.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    let cold_phy_archive_functions = archive_function_rows(
        &cold_phy_reachable,
        &inventory.function_owners,
        &inventory.function_sizes,
    );
    let cold_phy_external_frontier = cold_phy_reachable
        .iter()
        .filter(|name| !inventory.function_owners.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    let wifi_full_cal_archive_functions = archive_function_rows(
        &wifi_full_cal_reachable,
        &inventory.function_owners,
        &inventory.function_sizes,
    );
    let wifi_full_cal_external_frontier = wifi_full_cal_reachable
        .iter()
        .filter(|name| !inventory.function_owners.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    let runtime_archive_function_bytes = runtime_archive_functions
        .iter()
        .map(|(_, _, size)| size)
        .sum::<u64>();
    let cold_phy_archive_function_bytes = cold_phy_archive_functions
        .iter()
        .map(|(_, _, size)| size)
        .sum::<u64>();
    let wifi_full_cal_archive_function_bytes = wifi_full_cal_archive_functions
        .iter()
        .map(|(_, _, size)| size)
        .sum::<u64>();
    let (runtime_rom_frontier_count, runtime_rom_frontier_bytes) =
        rom_frontier_metrics(&runtime_external_frontier, &rom_symbols);
    let (cold_phy_rom_frontier_count, cold_phy_rom_frontier_bytes) =
        rom_frontier_metrics(&cold_phy_external_frontier, &rom_symbols);
    let (wifi_full_cal_rom_frontier_count, wifi_full_cal_rom_frontier_bytes) =
        rom_frontier_metrics(&wifi_full_cal_external_frontier, &rom_symbols);
    let runtime_unresolved_external_count =
        runtime_external_frontier.len() - runtime_rom_frontier_count;
    let cold_phy_unresolved_external_count =
        cold_phy_external_frontier.len() - cold_phy_rom_frontier_count;
    let wifi_full_cal_unresolved_external_count =
        wifi_full_cal_external_frontier.len() - wifi_full_cal_rom_frontier_count;
    let mut reverse_references = reverse_references(&inventory.references);
    let pointer_backings = augment_pointer_backing_references(
        &mut reverse_references,
        &inventory.data_owners,
        &final_symbols,
    );
    let elf_digest = digest(elf)?;

    let mut runtime_globals = Vec::new();
    let mut linked_other_globals = Vec::new();
    let mut cold_phy_globals = Vec::new();
    for (name, owners) in &inventory.data_owners {
        if rust_owned_data_aliases.contains(name) {
            continue;
        }
        let Some(symbol) = final_symbols.get(name) else {
            continue;
        };
        if !is_mutable_data(symbol.kind) || symbol.address == 0 || name.starts_with('.') {
            continue;
        }
        let all_referrers = reverse_references.get(name).cloned().unwrap_or_default();
        let linked_referrers = linked_code_referrers(&all_referrers, &final_symbols);
        let runtime_referrers = linked_referrers
            .intersection(&reachable)
            .cloned()
            .collect::<BTreeSet<_>>();
        let cold_phy_referrers = linked_referrers
            .intersection(&cold_phy_reachable)
            .cloned()
            .collect::<BTreeSet<_>>();
        if !cold_phy_referrers.is_empty() {
            cold_phy_globals.push((
                name.clone(),
                symbol.clone(),
                owners.clone(),
                cold_phy_referrers,
            ));
        }
        let row = (
            name.clone(),
            symbol.clone(),
            owners.clone(),
            runtime_referrers,
            linked_referrers,
        );
        if row.3.is_empty() {
            linked_other_globals.push(row);
        } else {
            runtime_globals.push(row);
        }
    }

    runtime_globals.sort_by_key(|(_, symbol, ..)| symbol.address);
    linked_other_globals.sort_by_key(|(_, symbol, ..)| symbol.address);
    cold_phy_globals.sort_by_key(|(_, symbol, ..)| symbol.address);

    let mut runtime_indirections = reverse_references
        .iter()
        .filter_map(|(name, referrers)| {
            let symbol = final_symbols.get(name)?;
            let runtime_referrers = referrers
                .intersection(&reachable)
                .cloned()
                .collect::<BTreeSet<_>>();
            (symbol.kind == 'A'
                && is_rom_data_indirection(symbol.address)
                && !runtime_referrers.is_empty())
            .then(|| {
                (
                    name.clone(),
                    symbol.clone(),
                    pointer_backings.get(name).cloned(),
                    runtime_referrers,
                )
            })
        })
        .collect::<Vec<_>>();
    runtime_indirections.sort_by_key(|(_, symbol, ..)| symbol.address);

    let wrappers = final_symbols
        .iter()
        .filter(|(name, symbol)| name.starts_with("__wrap_") && is_code(symbol.kind))
        .collect::<Vec<_>>();
    let strict_sections = sections
        .iter()
        .filter(|section| {
            section.name.contains("wifi_strict")
                || section.name == ".data.wifi"
                || section.name == ".bss.esp_wifi_async_net80211"
        })
        .collect::<Vec<_>>();

    let runtime_bytes = runtime_globals
        .iter()
        .map(|(_, symbol, ..)| symbol.size)
        .sum::<u64>();
    let cold_phy_bytes = cold_phy_globals
        .iter()
        .map(|(_, symbol, ..)| symbol.size)
        .sum::<u64>();
    let other_bytes = linked_other_globals
        .iter()
        .map(|(_, symbol, ..)| symbol.size)
        .sum::<u64>();
    let strict_static_bytes = strict_sections
        .iter()
        .map(|section| section.size)
        .sum::<u64>();
    let live_static_bindings = ROM_ABI_BACKINGS
        .iter()
        .filter(|(cell, backing)| {
            final_symbols.contains_key(*cell) && final_symbols.contains_key(*backing)
        })
        .count();

    if enforce_primary_baseline {
        enforce_primary_state_baseline(StateMetrics {
            vendor_roots: ROOTS.len(),
            reachable_vendor_functions: reachable.len(),
            runtime_mutable_blob_symbols: runtime_globals.len(),
            runtime_mutable_blob_bytes: runtime_bytes,
            runtime_rom_indirections: runtime_indirections.len(),
            cold_phy_mutable_blob_bytes: cold_phy_bytes,
            linked_other_mutable_blob_bytes: other_bytes,
            strict_static_bytes,
        })?;
    }

    let mut report = String::new();
    pushln(
        &mut report,
        "# ESP32-S31 linked state and interposition audit",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        &format!(
            "- final ELF: `{}`",
            elf.file_name().unwrap().to_string_lossy()
        ),
    );
    pushln(&mut report, &format!("- ELF SHA-256: `{elf_digest}`"));
    if let Some(path) = rom_elf {
        pushln(
            &mut report,
            &format!(
                "- ROM ELF: `{}` / SHA-256 `{}`",
                path.file_name().unwrap().to_string_lossy(),
                digest(path)?
            ),
        );
    }
    pushln(
        &mut report,
        &format!("- strict vendor roots: {}", ROOTS.len()),
    );
    pushln(
        &mut report,
        &format!(
            "- Rust boundaries retaining vendor fallback: {}",
            RUST_BOUNDARIES_WITH_VENDOR_FALLBACK.len()
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- stateful or not-yet-proven runtime roots: {}",
            STATEFUL_OR_UNPROVEN_RUNTIME_ROOTS.len()
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- temporary evidenced MMIO-only roots: {}",
            TEMPORARY_EVIDENCED_MMIO_ROOTS.len()
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- reference-only control-flow roots: `{}`",
            STRICT_REFERENCE_ROOTS.join("`, `")
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- separately auditable static-binding roots: `{}`",
            STATIC_BINDING_ROOTS.join("`, `")
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- separately auditable static-PM root: `{}`",
            STATIC_PM_INIT_ROOTS.join("`, `")
        ),
    );
    pushln(
        &mut report,
        "- separately auditable Rust caller-task cold init: `wifi_init_in_caller_task`, `wifi_deinit_in_caller_task`",
    );
    pushln(
        &mut report,
        &format!(
            "- vendor functions reachable from those roots: {}",
            reachable.len()
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- strict runtime archive functions: {} definitions / {} bytes",
            runtime_archive_functions.len(),
            runtime_archive_function_bytes
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- cold-PHY archive functions: {} definitions / {} bytes",
            cold_phy_archive_functions.len(),
            cold_phy_archive_function_bytes
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- strict runtime direct ROM frontier: {} functions / {} bytes; unresolved externals: {}",
            runtime_rom_frontier_count,
            runtime_rom_frontier_bytes,
            runtime_unresolved_external_count
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- cold-PHY direct ROM frontier: {} functions / {} bytes; unresolved externals: {}",
            cold_phy_rom_frontier_count,
            cold_phy_rom_frontier_bytes,
            cold_phy_unresolved_external_count
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- focused Wi-Fi full-cal radio graph: {} archive definitions / {} bytes; {} direct ROM functions / {} bytes; unresolved externals: {}",
            wifi_full_cal_archive_functions.len(),
            wifi_full_cal_archive_function_bytes,
            wifi_full_cal_rom_frontier_count,
            wifi_full_cal_rom_frontier_bytes,
            wifi_full_cal_unresolved_external_count
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- live mutable blob globals reached by strict leaves: {} symbols / {} bytes",
            runtime_globals.len(),
            runtime_bytes
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- live mutable blob globals reached from `register_chipv7_phy`: {} symbols / {} bytes",
            cold_phy_globals.len(),
            cold_phy_bytes
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- ROM-ABI mutable indirection cells reached by strict leaves: {} cells / {} cell bytes",
            runtime_indirections.len(),
            runtime_indirections.len() * 4
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- fixed cold-init bindings live in this ELF: {live_static_bindings} / {}",
            ROM_ABI_BACKINGS.len()
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- validated Rust-owned ABI data aliases: {} / {}",
            rust_owned_data_aliases.len(),
            RUST_OWNED_ABI_DATA_ALIASES.len()
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- live mutable blob globals outside the strict-root graph: {} symbols / {} bytes",
            linked_other_globals.len(),
            other_bytes
        ),
    );
    pushln(
        &mut report,
        &format!(
            "- Rust strict static sections: {} sections / {} bytes",
            strict_sections.len(),
            strict_static_bytes
        ),
    );
    pushln(
        &mut report,
        &format!("- retained code wrappers: {}", wrappers.len()),
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "The archive relocation graph supplies `vendor function -> data symbol`; the final ELF supplies liveness, address, size and section. A wrapper boundary stops traversal into the replaced vendor body. “Outside strict roots” means linked but not proven runtime-reachable by this vendor-leaf graph; it is not automatically safe to delete because cold initialization and non-Wi-Fi owners can still use it.",
    );
    pushln(
        &mut report,
        "Run `audit-strict-esp32s31 --include-static-binding-init --include-static-pm-init --enforce` to prove the fixed-storage cold-init leaves together with the runtime roots.",
    );
    pushln(
        &mut report,
        "Run this auditor with `--enforce-primary-baseline` to reject growth beyond the qualified heap-free image while allowing vendor roots, linked blob state, and Rust static storage to shrink.",
    );
    pushln(
        &mut report,
        "The application `wifi-rust-static-cold-init-hil` final-ELF audit additionally proves the three fixed SRAM locks, the exact direct init/deinit call targets, the taskless PP tail calls, and the absence of control-flow cycles.",
    );

    pushln(&mut report, "");
    pushln(&mut report, "## Strict runtime archive function frontier");
    pushln(&mut report, "");
    pushln(
        &mut report,
        "These are the exact archive definitions remaining below the strict runtime root after stopping at every Rust interposition boundary. Sizes are the original archive text sizes, not the replacement sizes.",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| function | archive text bytes | archive owner |",
    );
    pushln(&mut report, "|---|---:|---|");
    push_function_rows(&mut report, &runtime_archive_functions);

    pushln(&mut report, "");
    pushln(
        &mut report,
        "### Strict runtime direct ROM/external frontier",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "These are direct calls leaving the pinned static archives. A ROM text size and address are reported only when the separately supplied ROM ELF defines the symbol.",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| function | ROM text bytes | ROM address / status |",
    );
    pushln(&mut report, "|---|---:|---|");
    push_frontier_rows(&mut report, &runtime_external_frontier, &rom_symbols);

    pushln(&mut report, "");
    pushln(&mut report, "## PHY cold-init archive function graph");
    pushln(&mut report, "");
    pushln(
        &mut report,
        "This is the complete direct relocation graph rooted at `register_chipv7_phy` for definitions present in the pinned static archives. An archive function may call the external/ROM frontier below; calls internal to the ROM image are not expanded here.",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| function | archive text bytes | archive owner |",
    );
    pushln(&mut report, "|---|---:|---|");
    push_function_rows(&mut report, &cold_phy_archive_functions);

    pushln(&mut report, "");
    pushln(
        &mut report,
        "### PHY cold-init direct ROM/external frontier",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "These symbols are called by the cold-PHY archive graph but have no definition in the pinned static archives. A supplied ROM ELF proves direct ROM text sizes and addresses. Calls internal to those ROM bodies are still outside this direct-frontier inventory.",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| function | ROM text bytes | ROM address / status |",
    );
    pushln(&mut report, "|---|---:|---|");
    push_frontier_rows(&mut report, &cold_phy_external_frontier, &rom_symbols);

    pushln(&mut report, "");
    pushln(&mut report, "## Focused Wi-Fi full-calibration radio graph");
    pushln(&mut report, "");
    pushln(
        &mut report,
        "This is the porting workset for the primary no-NVS Wi-Fi profile. Traversal stops before vendor logging/formatting and calibration-record check, backup, or recovery. Those omitted boundaries are deleted policy, not replacement targets. The table still includes BT/coexistence-named descendants reached unconditionally by the original parent; they remain candidates until register evidence or hardware qualification proves that a Wi-Fi-only parent may omit them.",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        &format!(
            "Omitted boundaries: `{}`.",
            WIFI_FULL_CAL_OMITTED_COLD_BOUNDARIES.join("`, `")
        ),
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| function | archive text bytes | archive owner |",
    );
    pushln(&mut report, "|---|---:|---|");
    push_function_rows(&mut report, &wifi_full_cal_archive_functions);

    pushln(&mut report, "");
    pushln(
        &mut report,
        "### Focused Wi-Fi direct ROM/external frontier",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| function | ROM text bytes | ROM address / status |",
    );
    pushln(&mut report, "|---|---:|---|");
    push_frontier_rows(&mut report, &wifi_full_cal_external_frontier, &rom_symbols);

    pushln(&mut report, "");
    pushln(
        &mut report,
        "## Mutable blob state reached by strict vendor leaves",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| symbol | size | placement | archive owner | strict referrers |",
    );
    pushln(&mut report, "|---|---:|---|---|---|");
    for (name, symbol, owners, runtime_referrers, _) in &runtime_globals {
        pushln(
            &mut report,
            &format!(
                "| `{name}` | {} | `{}` / `{}` | {} | {} |",
                symbol.size,
                placement(symbol.address),
                section_name(symbol.address, &sections),
                code_set(owners, 3),
                code_set(runtime_referrers, 5),
            ),
        );
    }
    if runtime_globals.is_empty() {
        pushln(&mut report, "| _none_ | 0 | - | - | - |");
    }

    pushln(&mut report, "");
    pushln(
        &mut report,
        "## Mutable blob state reached by PHY cold initialization",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "This is the direct archive call graph rooted at `register_chipv7_phy`. It does not prove indirect ROM callbacks unreachable.",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| symbol | size | placement | archive owner | cold PHY referrers |",
    );
    pushln(&mut report, "|---|---:|---|---|---|");
    for (name, symbol, owners, cold_referrers) in &cold_phy_globals {
        pushln(
            &mut report,
            &format!(
                "| `{name}` | {} | `{}` / `{}` | {} | {} |",
                symbol.size,
                placement(symbol.address),
                section_name(symbol.address, &sections),
                code_set(owners, 3),
                code_set(cold_referrers, 5),
            ),
        );
    }
    if cold_phy_globals.is_empty() {
        pushln(&mut report, "| _none_ | 0 | - | - | - |");
    }

    pushln(&mut report, "");
    pushln(
        &mut report,
        "## Mutable ROM-ABI indirection cells reached by strict leaves",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "These absolute symbols name four-byte pointer/callback cells in the S31 ROM ABI RAM table. They are state even though `llvm-nm` reports linker kind `A`. A conventional `*_ptr -> *` backing is shown when the backing object is present in the ELF.",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| cell | address | inferred backing | strict referrers |",
    );
    pushln(&mut report, "|---|---:|---|---|");
    for (name, symbol, backing, referrers) in &runtime_indirections {
        pushln(
            &mut report,
            &format!(
                "| `{name}` | `0x{:08x}` | {} | {} |",
                symbol.address,
                backing
                    .as_ref()
                    .map_or_else(|| cell_role(name).to_owned(), |name| format!("`{name}`")),
                code_set(referrers, 5)
            ),
        );
    }

    pushln(&mut report, "");
    pushln(&mut report, "## Fixed cold-init state bindings");
    pushln(&mut report, "");
    pushln(
        &mut report,
        "These are the exact direct stores recovered from the two separately audited cold-init leaves. The Rust interposition path publishes the same backing addresses without calling either vendor body.",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| published pointer cell | address | fixed backing | bytes | placement |",
    );
    pushln(&mut report, "|---|---:|---|---:|---|");
    for (cell, backing) in ROM_ABI_BACKINGS {
        let Some(cell_symbol) = final_symbols.get(*cell) else {
            continue;
        };
        let Some(backing_symbol) = final_symbols.get(*backing) else {
            continue;
        };
        pushln(
            &mut report,
            &format!(
                "| `{cell}` | `0x{:08x}` | `{backing}` | {} | `{}` / `{}` |",
                cell_symbol.address,
                backing_symbol.size,
                placement(backing_symbol.address),
                section_name(backing_symbol.address, &sections),
            ),
        );
    }

    pushln(&mut report, "");
    pushln(&mut report, "## Rust-owned ABI data aliases");
    pushln(&mut report, "");
    pushln(
        &mut report,
        "Pinned vendor objects may still load these public C data names. The final link proves that each name resolves directly to explicit Rust-owned storage of the required size in internal SRAM; no separate blob allocation remains.",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| public ABI name | Rust-owned backing | address | bytes | placement |",
    );
    pushln(&mut report, "|---|---|---:|---:|---|");
    for (public_name, backing_name, expected_size) in RUST_OWNED_ABI_DATA_ALIASES {
        let public = &final_symbols[*public_name];
        let backing = &final_symbols[*backing_name];
        pushln(
            &mut report,
            &format!(
                "| `{public_name}` | `{backing_name}` | `0x{:08x}` | {} | `{}` / `{}` |",
                public.address,
                expected_size,
                placement(backing.address),
                section_name(backing.address, &sections),
            ),
        );
    }

    pushln(&mut report, "");
    pushln(&mut report, "## Rust-owned strict static storage");
    pushln(&mut report, "");
    pushln(&mut report, "| section | address | bytes | placement |");
    pushln(&mut report, "|---|---:|---:|---|");
    for section in &strict_sections {
        pushln(
            &mut report,
            &format!(
                "| `{}` | `0x{:08x}` | {} | `{}` |",
                section.name,
                section.address,
                section.size,
                placement(section.address)
            ),
        );
    }

    pushln(&mut report, "");
    pushln(&mut report, "## Final-link interposition");
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| boundary | replacement | mode | `__real_*` target |",
    );
    pushln(&mut report, "|---|---:|---|---|");
    for (wrapper_name, wrapper) in wrappers {
        let public_name = wrapper_name.trim_start_matches("__wrap_");
        let real_name = format!("__real_{public_name}");
        let public = final_symbols.get(public_name);
        let real = final_symbols.get(&real_name);
        let mode = match public {
            Some(public) if public.address == wrapper.address => "direct public alias",
            Some(_) => "GNU `--wrap` boundary",
            None => "retained replacement only",
        };
        let real_target = real.map_or_else(
            || "-".to_owned(),
            |symbol| {
                format!(
                    "`0x{:08x}` ({})",
                    symbol.address,
                    target_placement(symbol.address)
                )
            },
        );
        pushln(
            &mut report,
            &format!(
                "| `{public_name}` | `0x{:08x}` | {mode} | {real_target} |",
                wrapper.address
            ),
        );
    }
    for (public_name, replacement_name) in REQUIRED_RUNTIME_ALIASES {
        if replacement_name.starts_with("__wrap_") {
            continue;
        }
        let public = final_symbols
            .get(*public_name)
            .with_context(|| format!("missing required public alias {public_name}"))?;
        let replacement = final_symbols
            .get(*replacement_name)
            .with_context(|| format!("missing required replacement {replacement_name}"))?;
        let real_name = format!("__real_{public_name}");
        let real_target = final_symbols.get(&real_name).map_or_else(
            || "-".to_owned(),
            |symbol| {
                format!(
                    "`0x{:08x}` ({})",
                    symbol.address,
                    target_placement(symbol.address)
                )
            },
        );
        let mode = if public.address == replacement.address {
            "direct public alias"
        } else {
            "ERROR: public/replacement mismatch"
        };
        pushln(
            &mut report,
            &format!(
                "| `{public_name}` | `0x{:08x}` | {mode} | {real_target} |",
                replacement.address
            ),
        );
    }

    pushln(&mut report, "");
    pushln(
        &mut report,
        "## Linked mutable blob state outside the strict-root graph",
    );
    pushln(&mut report, "");
    pushln(
        &mut report,
        "| symbol | size | placement | archive owner | linked referrers |",
    );
    pushln(&mut report, "|---|---:|---|---|---|");
    for (name, symbol, owners, _, linked_referrers) in &linked_other_globals {
        pushln(
            &mut report,
            &format!(
                "| `{name}` | {} | `{}` / `{}` | {} | {} |",
                symbol.size,
                placement(symbol.address),
                section_name(symbol.address, &sections),
                code_set(owners, 3),
                code_set(linked_referrers, 5),
            ),
        );
    }

    Ok(report)
}

/// Improvement-friendly limits from the qualified heap-free primary image.
///
/// Reducing any upper bound is allowed. The corrected archive parser retains
/// local-label ownership and counts absolute ROM code aliases, exposing the
/// remaining fallback's state rather than incorrectly reporting exact zero.
/// Blob-to-Rust ownership transfers are measured by their combined static
/// footprint, so a byte may change owner without weakening the no-growth
/// invariant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StateMetrics {
    vendor_roots: usize,
    reachable_vendor_functions: usize,
    runtime_mutable_blob_symbols: usize,
    runtime_mutable_blob_bytes: u64,
    runtime_rom_indirections: usize,
    cold_phy_mutable_blob_bytes: u64,
    linked_other_mutable_blob_bytes: u64,
    strict_static_bytes: u64,
}

const PRIMARY_STATE_BASELINE: StateMetrics = StateMetrics {
    vendor_roots: 26,
    reachable_vendor_functions: 39,
    runtime_mutable_blob_symbols: 4,
    runtime_mutable_blob_bytes: 1_412,
    runtime_rom_indirections: 3,
    cold_phy_mutable_blob_bytes: 512,
    // `BAROFDMSched` adds one 12-byte immutable-contents compatibility
    // schedule to the linked mutable-data inventory. The Rust TX-PER adapter
    // validates every schedule pointer before reading it; eliminating these
    // C schedule arenas is the next rate-control ownership slice.
    linked_other_mutable_blob_bytes: 22_215,
    // Two multi-descriptor RX owners add 288 bytes of ISR-visible ESF headers
    // and eight bytes of ownership state. Seventeen temporary outer-RX
    // fallback counters add another 68 bytes while the remaining vendor
    // routes are measured. The Rust-owned TX-PER transition and its validated
    // ABI projection add 320 bytes of internal executable/read-only storage.
    // The 32-KiB aggregate payload arena is in PSRAM and is intentionally
    // excluded from the internal-SRAM metric. Rust-owned TXOP publication
    // adds the exact three-byte queue-class pool plus 128 bytes for its two
    // finite ABI boundaries; the former vendor three-byte global is removed
    // from the linked mutable-blob inventory.
    strict_static_bytes: 312_441,
};

fn enforce_primary_state_baseline(actual: StateMetrics) -> Result<()> {
    let baseline = PRIMARY_STATE_BASELINE;
    let regressions = [
        (
            actual.vendor_roots > baseline.vendor_roots,
            format!(
                "strict vendor roots {} > {}",
                actual.vendor_roots, baseline.vendor_roots
            ),
        ),
        (
            actual.reachable_vendor_functions > baseline.reachable_vendor_functions,
            format!(
                "reachable vendor functions {} > {}",
                actual.reachable_vendor_functions, baseline.reachable_vendor_functions
            ),
        ),
        (
            actual.runtime_mutable_blob_symbols > baseline.runtime_mutable_blob_symbols
                || actual.runtime_mutable_blob_bytes > baseline.runtime_mutable_blob_bytes,
            format!(
                "runtime mutable blob state is {} symbols / {} bytes, baseline {} symbols / {} bytes",
                actual.runtime_mutable_blob_symbols,
                actual.runtime_mutable_blob_bytes,
                baseline.runtime_mutable_blob_symbols,
                baseline.runtime_mutable_blob_bytes
            ),
        ),
        (
            actual.runtime_rom_indirections > baseline.runtime_rom_indirections,
            format!(
                "runtime ROM indirection cells {}, baseline {}",
                actual.runtime_rom_indirections, baseline.runtime_rom_indirections
            ),
        ),
        (
            actual.cold_phy_mutable_blob_bytes > baseline.cold_phy_mutable_blob_bytes,
            format!(
                "cold PHY mutable blob state {} > {} bytes",
                actual.cold_phy_mutable_blob_bytes, baseline.cold_phy_mutable_blob_bytes
            ),
        ),
        (
            actual.linked_other_mutable_blob_bytes > baseline.linked_other_mutable_blob_bytes,
            format!(
                "linked mutable blob state outside strict roots {} > {} bytes",
                actual.linked_other_mutable_blob_bytes, baseline.linked_other_mutable_blob_bytes
            ),
        ),
        (
            actual
                .strict_static_bytes
                .saturating_add(actual.linked_other_mutable_blob_bytes)
                > baseline
                    .strict_static_bytes
                    .saturating_add(baseline.linked_other_mutable_blob_bytes),
            format!(
                "combined Rust/blob static storage {} > {} bytes (Rust {}, blob {})",
                actual
                    .strict_static_bytes
                    .saturating_add(actual.linked_other_mutable_blob_bytes),
                baseline
                    .strict_static_bytes
                    .saturating_add(baseline.linked_other_mutable_blob_bytes),
                actual.strict_static_bytes,
                actual.linked_other_mutable_blob_bytes,
            ),
        ),
    ]
    .into_iter()
    .filter_map(|(failed, message)| failed.then_some(message))
    .collect::<Vec<_>>();

    if regressions.is_empty() {
        Ok(())
    } else {
        bail!(
            "ESP32-S31 primary state baseline regressed:\n- {}",
            regressions.join("\n- ")
        )
    }
}

fn inventory_archives(library_dir: &Path) -> Result<ArchiveInventory> {
    let mut inventory = ArchiveInventory::default();
    let mut archives = fs::read_dir(library_dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "a"))
        .collect::<Vec<_>>();
    archives.sort();

    for archive in archives {
        let archive_name = archive
            .file_name()
            .and_then(|name| name.to_str())
            .context("archive name is not UTF-8")?;
        let nm = text(checked(
            Command::new("llvm-nm")
                .arg("-a")
                .arg("-A")
                .arg("-S")
                .arg("-P")
                .arg("--defined-only")
                .arg(&archive),
        )?)?;
        let mut member_symbols = BTreeMap::<String, Vec<(String, Symbol)>>::new();
        for line in nm.lines() {
            let Some((source, (name, symbol))) = parse_archive_symbol(line) else {
                continue;
            };
            let owner = short_owner(source, archive_name);
            member_symbols
                .entry(owner.clone())
                .or_default()
                .push((name.to_owned(), symbol.clone()));
            if is_mutable_data(symbol.kind) && !name.starts_with('.') {
                inventory
                    .data_owners
                    .entry(name.to_owned())
                    .or_default()
                    .insert(owner.clone());
            } else if is_code(symbol.kind) && !name.starts_with('.') {
                inventory
                    .function_owners
                    .entry(name.to_owned())
                    .or_default()
                    .insert(owner.clone());
                inventory
                    .function_sizes
                    .entry(name.to_owned())
                    .or_default()
                    .insert(owner, symbol.size);
            }
        }
        let local_data_aliases = local_data_aliases(&member_symbols);

        let disassembly = text(checked(
            Command::new("llvm-objdump")
                .arg("-dr")
                .arg("--no-show-raw-insn")
                .arg(&archive),
        )?)?;
        parse_archive_relocations(
            &disassembly,
            archive_name,
            &local_data_aliases,
            &mut inventory,
        );
    }
    Ok(inventory)
}

fn archive_function_rows(
    reachable: &BTreeSet<String>,
    owners: &BTreeMap<String, BTreeSet<String>>,
    sizes: &BTreeMap<String, BTreeMap<String, u64>>,
) -> Vec<(String, String, u64)> {
    let mut rows = Vec::new();
    for function in reachable {
        let Some(function_owners) = owners.get(function) else {
            continue;
        };
        for owner in function_owners {
            let size = sizes
                .get(function)
                .and_then(|definitions| definitions.get(owner))
                .copied()
                .unwrap_or(0);
            rows.push((function.clone(), owner.clone(), size));
        }
    }
    rows
}

fn push_function_rows(report: &mut String, rows: &[(String, String, u64)]) {
    if rows.is_empty() {
        pushln(report, "| _none_ | 0 | - |");
        return;
    }
    for (function, owner, size) in rows {
        pushln(report, &format!("| `{function}` | {size} | `{owner}` |"));
    }
}

fn rom_frontier_metrics(
    frontier: &[String],
    rom_symbols: &BTreeMap<String, Symbol>,
) -> (usize, u64) {
    frontier
        .iter()
        .filter_map(|name| rom_symbols.get(name).filter(|symbol| is_code(symbol.kind)))
        .fold((0, 0), |(count, bytes), symbol| {
            (count + 1, bytes + symbol.size)
        })
}

fn push_frontier_rows(
    report: &mut String,
    frontier: &[String],
    rom_symbols: &BTreeMap<String, Symbol>,
) {
    if frontier.is_empty() {
        pushln(report, "| _none_ | 0 | - |");
        return;
    }
    for function in frontier {
        match rom_symbols
            .get(function)
            .filter(|symbol| is_code(symbol.kind))
        {
            Some(symbol) => pushln(
                report,
                &format!(
                    "| `{function}` | {} | `0x{:08x}` |",
                    symbol.size, symbol.address
                ),
            ),
            None => pushln(
                report,
                &format!("| `{function}` | - | unresolved external |"),
            ),
        }
    }
}

fn parse_archive_symbol(line: &str) -> Option<(&str, (&str, Symbol))> {
    let (source, fields) = line.rsplit_once(": ")?;
    let fields = fields.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 4 {
        return None;
    }
    let kind = fields[1].chars().next()?;
    let address = u64::from_str_radix(fields[2], 16).ok()?;
    let size = u64::from_str_radix(fields[3], 16).ok()?;
    Some((
        source,
        (
            fields[0],
            Symbol {
                address,
                size,
                kind,
            },
        ),
    ))
}

fn short_owner(source: &str, fallback_archive: &str) -> String {
    let file = source
        .rsplit(std::path::MAIN_SEPARATOR)
        .next()
        .unwrap_or(source);
    if file.contains('[') {
        file.to_owned()
    } else {
        fallback_archive.to_owned()
    }
}

fn local_data_aliases(
    member_symbols: &BTreeMap<String, Vec<(String, Symbol)>>,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut aliases = BTreeMap::new();
    for (owner, symbols) in member_symbols {
        for (local_name, local_symbol) in symbols
            .iter()
            .filter(|(name, symbol)| name.starts_with(".LANCHOR") && is_mutable_data(symbol.kind))
        {
            let matches = symbols
                .iter()
                .filter(|(name, symbol)| {
                    !name.starts_with('.')
                        && is_mutable_data(symbol.kind)
                        && symbol.address == local_symbol.address
                        && data_kind(symbol.kind) == data_kind(local_symbol.kind)
                })
                .map(|(name, _)| name)
                .collect::<Vec<_>>();
            if let [canonical] = matches.as_slice() {
                aliases
                    .entry(owner.clone())
                    .or_insert_with(BTreeMap::new)
                    .insert(local_name.clone(), (*canonical).clone());
            }
        }
    }
    aliases
}

fn data_kind(kind: char) -> char {
    match kind {
        'B' | 'b' | 'C' | 'c' => 'b',
        'D' | 'd' | 'G' | 'g' | 'S' | 's' => 'd',
        _ => '?',
    }
}

fn parse_archive_relocations(
    disassembly: &str,
    archive_name: &str,
    local_data_aliases: &BTreeMap<String, BTreeMap<String, String>>,
    inventory: &mut ArchiveInventory,
) {
    let mut function = None::<String>;
    let mut owner = archive_name.to_owned();
    for line in disassembly.lines() {
        if let Some((source, _)) = line.split_once(":\tfile format ") {
            if let Some((_, member)) = source.rsplit_once('(') {
                if let Some(member) = member.strip_suffix(')') {
                    owner = format!("{archive_name}[{member}]");
                    function = None;
                    continue;
                }
            }
        }
        if let Some(name) = definition_name(line) {
            // Local assembler labels remain inside the current function.
            // Clearing the owner here silently dropped every relocation after
            // the first `.L*` control-flow target and under-reported both the
            // reachable call graph and its mutable-state references.
            if !name.starts_with('.') {
                function = Some(normalize_symbol(name));
            }
            continue;
        }
        let Some(caller) = function.as_ref() else {
            continue;
        };
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let Some(index) = fields
            .iter()
            .position(|field| field.starts_with("R_RISCV_"))
        else {
            continue;
        };
        let Some(target) = fields.get(index + 1) else {
            continue;
        };
        let mut target = normalize_symbol(target);
        if target.starts_with(".LANCHOR") {
            let Some(canonical) = local_data_aliases
                .get(&owner)
                .and_then(|aliases| aliases.get(&target))
            else {
                continue;
            };
            target = canonical.clone();
        }
        if target.starts_with('.') || target == "*ABS*" {
            continue;
        }
        inventory
            .references
            .entry(caller.clone())
            .or_default()
            .insert(target.clone());
        if matches!(
            fields.get(index).copied(),
            Some("R_RISCV_CALL" | "R_RISCV_CALL_PLT" | "R_RISCV_JAL")
        ) {
            inventory
                .calls
                .entry(caller.clone())
                .or_default()
                .insert(target);
        }
    }
}

fn reachable_vendor_functions(calls: &BTreeMap<String, BTreeSet<String>>) -> BTreeSet<String> {
    reachable_from_roots(calls, ROOTS, WRAPPED_VENDOR_BOUNDARIES)
}

fn reachable_from_roots(
    calls: &BTreeMap<String, BTreeSet<String>>,
    roots: &[&str],
    boundaries: &[&str],
) -> BTreeSet<String> {
    let mut reachable = BTreeSet::new();
    let mut pending = VecDeque::from_iter(roots.iter().map(|root| (*root).to_owned()));
    while let Some(function) = pending.pop_front() {
        if !roots.contains(&function.as_str()) && boundaries.contains(&function.as_str()) {
            continue;
        }
        if !reachable.insert(function.clone()) {
            continue;
        }
        if let Some(targets) = calls.get(&function) {
            pending.extend(targets.iter().cloned());
        }
    }
    reachable
}

fn reverse_references(
    references: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut reverse = BTreeMap::<String, BTreeSet<String>>::new();
    for (function, symbols) in references {
        for symbol in symbols {
            reverse
                .entry(symbol.clone())
                .or_default()
                .insert(function.clone());
        }
    }
    reverse
}

fn augment_pointer_backing_references(
    reverse: &mut BTreeMap<String, BTreeSet<String>>,
    data_owners: &BTreeMap<String, BTreeSet<String>>,
    final_symbols: &BTreeMap<String, Symbol>,
) -> BTreeMap<String, String> {
    let aliases = reverse
        .iter()
        .filter_map(|(alias, referrers)| {
            let alias_symbol = final_symbols.get(alias)?;
            let backing = ROM_ABI_BACKINGS
                .iter()
                .find_map(|(cell, backing)| (*cell == alias).then_some(*backing))
                .or_else(|| alias.strip_suffix("_ptr"))?;
            (alias_symbol.kind == 'A'
                && is_rom_data_indirection(alias_symbol.address)
                && data_owners.contains_key(backing)
                && final_symbols
                    .get(backing)
                    .is_some_and(|symbol| is_mutable_data(symbol.kind)))
            .then(|| (alias.clone(), backing.to_owned(), referrers.clone()))
        })
        .collect::<Vec<_>>();
    let mut backings = BTreeMap::new();
    for (alias, backing, referrers) in aliases {
        reverse
            .entry(backing.clone())
            .or_default()
            .extend(referrers);
        backings.insert(alias, backing);
    }
    backings
}

fn parse_posix_symbols(nm: &str) -> BTreeMap<String, Symbol> {
    let mut symbols = BTreeMap::new();
    for line in nm.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 4 {
            continue;
        }
        let Some(kind) = fields[1].chars().next() else {
            continue;
        };
        let Ok(address) = u64::from_str_radix(fields[2], 16) else {
            continue;
        };
        let Ok(size) = u64::from_str_radix(fields[3], 16) else {
            continue;
        };
        symbols.insert(
            fields[0].to_owned(),
            Symbol {
                address,
                size,
                kind,
            },
        );
    }
    symbols
}

fn parse_sections(readelf: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    for line in readelf.lines() {
        let Some((_, tail)) = line.split_once(']') else {
            continue;
        };
        let fields = tail.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 5 {
            continue;
        }
        let Ok(address) = u64::from_str_radix(fields[2], 16) else {
            continue;
        };
        let Ok(size) = u64::from_str_radix(fields[4], 16) else {
            continue;
        };
        sections.push(Section {
            name: fields[0].to_owned(),
            address,
            size,
        });
    }
    sections
}

fn validate_rust_owned_data_aliases(
    final_symbols: &BTreeMap<String, Symbol>,
    sections: &[Section],
) -> Result<BTreeSet<String>> {
    let mut validated = BTreeSet::new();
    for (public_name, backing_name, expected_size) in RUST_OWNED_ABI_DATA_ALIASES {
        let public = final_symbols
            .get(*public_name)
            .with_context(|| format!("missing Rust-owned ABI data alias {public_name}"))?;
        let backing = final_symbols
            .get(*backing_name)
            .with_context(|| format!("missing Rust-owned ABI data backing {backing_name}"))?;
        if public.address == 0 || public.address != backing.address {
            bail!(
                "Rust-owned ABI data alias {public_name} at 0x{:08x} does not resolve to \
                 {backing_name} at 0x{:08x}",
                public.address,
                backing.address
            );
        }
        if backing.size != *expected_size {
            bail!(
                "Rust-owned ABI data backing {backing_name} has {} bytes, expected {}",
                backing.size,
                expected_size
            );
        }
        if !is_mutable_data(backing.kind)
            || placement(backing.address) != "internal SRAM"
            || section_name(backing.address, sections) == "unknown"
        {
            bail!(
                "Rust-owned ABI data backing {backing_name} is not writable internal-SRAM \
                 storage in a final ELF section"
            );
        }
        validated.insert((*public_name).to_owned());
    }
    Ok(validated)
}

fn section_name(address: u64, sections: &[Section]) -> &str {
    sections
        .iter()
        .find(|section| {
            section.size != 0
                && (section.address..section.address.saturating_add(section.size))
                    .contains(&address)
        })
        .map_or("unknown", |section| section.name.as_str())
}

fn placement(address: u64) -> &'static str {
    match address {
        0x2f00_0000..=0x2fff_ffff => "internal SRAM",
        0x5000_0000..=0x5fff_ffff => "PSRAM",
        0x4000_0000..=0x4fff_ffff => "flash-mapped",
        _ => "other",
    }
}

fn target_placement(address: u64) -> &'static str {
    match address {
        // esp32s31_rev0_rom.elf has one executable LOAD segment covering
        // .fixed.text, .init.text and .text: 0x2f80_0000..0x2f83_f700.
        0x2f80_0000..=0x2f83_f6ff => "ROM export",
        _ => placement(address),
    }
}

fn is_rom_data_indirection(address: u64) -> bool {
    (0x2f07_fc00..0x2f08_0000).contains(&address)
}

fn cell_role(name: &str) -> &'static str {
    match name {
        "g_osi_funcs_p" => "Rust-installed strict OSI table pointer",
        "s_netstack_free" => "registered netstack-free callback",
        "esp_test_rx_error_occurs" => "RX diagnostic scalar",
        _ => "unresolved ROM ABI cell",
    }
}

fn is_mutable_data(kind: char) -> bool {
    matches!(
        kind,
        'B' | 'b' | 'C' | 'c' | 'D' | 'd' | 'G' | 'g' | 'S' | 's'
    )
}

fn is_code(kind: char) -> bool {
    matches!(kind, 'T' | 't' | 'W' | 'w')
}

fn is_linked_code(symbol: &Symbol) -> bool {
    is_code(symbol.kind) || (symbol.kind == 'A' && target_placement(symbol.address) == "ROM export")
}

fn linked_code_referrers(
    referrers: &BTreeSet<String>,
    final_symbols: &BTreeMap<String, Symbol>,
) -> BTreeSet<String> {
    referrers
        .iter()
        .filter(|referrer| final_symbols.get(*referrer).is_some_and(is_linked_code))
        .cloned()
        .collect()
}

fn definition_name(line: &str) -> Option<&str> {
    let start = line.find('<')? + 1;
    let end = line[start..].find(">:")? + start;
    line[..start - 1]
        .trim()
        .chars()
        .all(|character| character.is_ascii_hexdigit())
        .then_some(&line[start..end])
}

fn normalize_symbol(symbol: &str) -> String {
    symbol
        .split(['+', '@'])
        .next()
        .unwrap_or(symbol)
        .trim()
        .to_owned()
}

fn code_set(values: &BTreeSet<String>, limit: usize) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    let shown = values
        .iter()
        .take(limit)
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if values.len() > limit {
        format!("{shown}, +{}", values.len() - limit)
    } else {
        shown
    }
}

fn digest(path: &Path) -> Result<String> {
    let output = text(checked(Command::new("sha256sum").arg(path))?)?;
    output
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .context("sha256sum returned no digest")
}

fn pushln(output: &mut String, line: &str) {
    output.push_str(line);
    output.push('\n');
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
    use super::{
        definition_name, enforce_primary_state_baseline, linked_code_referrers, local_data_aliases,
        parse_archive_relocations, parse_archive_symbol, parse_posix_symbols, parse_sections,
        placement, reachable_vendor_functions, target_placement, validate_rust_owned_data_aliases,
        ArchiveInventory, Section, StateMetrics, Symbol, PRIMARY_STATE_BASELINE, ROM_ABI_BACKINGS,
        ROOTS, RUST_BOUNDARIES_WITH_VENDOR_FALLBACK, STATEFUL_OR_UNPROVEN_RUNTIME_ROOTS,
        TEMPORARY_EVIDENCED_MMIO_ROOTS,
    };
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn pinned_cold_init_has_43_unique_bindings() {
        assert_eq!(ROM_ABI_BACKINGS.len(), 43);
        let cells = ROM_ABI_BACKINGS
            .iter()
            .map(|(cell, _)| *cell)
            .collect::<std::collections::BTreeSet<_>>();
        let backings = ROM_ABI_BACKINGS
            .iter()
            .map(|(_, backing)| *backing)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(cells.len(), ROM_ABI_BACKINGS.len());
        assert_eq!(backings.len(), ROM_ABI_BACKINGS.len());
    }

    #[test]
    fn ownership_debt_classes_partition_every_runtime_root() {
        let roots = ROOTS.iter().copied().collect::<BTreeSet<_>>();
        let classified = RUST_BOUNDARIES_WITH_VENDOR_FALLBACK
            .iter()
            .chain(STATEFUL_OR_UNPROVEN_RUNTIME_ROOTS)
            .chain(TEMPORARY_EVIDENCED_MMIO_ROOTS)
            .copied()
            .collect::<BTreeSet<_>>();
        let classified_count = RUST_BOUNDARIES_WITH_VENDOR_FALLBACK.len()
            + STATEFUL_OR_UNPROVEN_RUNTIME_ROOTS.len()
            + TEMPORARY_EVIDENCED_MMIO_ROOTS.len();

        assert_eq!(classified_count, classified.len(), "debt classes overlap");
        assert_eq!(classified, roots, "every runtime root needs a contract");
    }

    #[test]
    fn primary_state_baseline_accepts_equal_or_smaller_graphs() {
        enforce_primary_state_baseline(PRIMARY_STATE_BASELINE).unwrap();
        enforce_primary_state_baseline(StateMetrics {
            vendor_roots: 20,
            reachable_vendor_functions: 30,
            runtime_mutable_blob_symbols: 0,
            runtime_mutable_blob_bytes: 0,
            runtime_rom_indirections: 0,
            cold_phy_mutable_blob_bytes: 0,
            linked_other_mutable_blob_bytes: 10_000,
            strict_static_bytes: 250_000,
        })
        .unwrap();

        // An exact ownership transfer is not a memory regression: the same
        // bytes moved from opaque blob data into an explicit Rust section.
        enforce_primary_state_baseline(StateMetrics {
            linked_other_mutable_blob_bytes: PRIMARY_STATE_BASELINE.linked_other_mutable_blob_bytes
                - 852,
            strict_static_bytes: PRIMARY_STATE_BASELINE.strict_static_bytes + 852,
            ..PRIMARY_STATE_BASELINE
        })
        .unwrap();
    }

    #[test]
    fn primary_state_baseline_rejects_runtime_blob_state() {
        let error = enforce_primary_state_baseline(StateMetrics {
            runtime_mutable_blob_symbols: PRIMARY_STATE_BASELINE.runtime_mutable_blob_symbols + 1,
            runtime_mutable_blob_bytes: PRIMARY_STATE_BASELINE.runtime_mutable_blob_bytes + 4,
            ..PRIMARY_STATE_BASELINE
        })
        .unwrap_err();

        assert!(error.to_string().contains("baseline"));
    }

    #[test]
    fn primary_state_baseline_rejects_net_static_growth_during_transfer() {
        let error = enforce_primary_state_baseline(StateMetrics {
            linked_other_mutable_blob_bytes: PRIMARY_STATE_BASELINE.linked_other_mutable_blob_bytes
                - 851,
            strict_static_bytes: PRIMARY_STATE_BASELINE.strict_static_bytes + 852,
            ..PRIMARY_STATE_BASELINE
        })
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("combined Rust/blob static storage"));
    }

    #[test]
    fn parses_archive_posix_symbol() {
        let line = "libs/libpp.a[pp.o]: pp_sig_cnt D 0 24";
        let (source, (name, symbol)) = parse_archive_symbol(line).unwrap();
        assert_eq!(source, "libs/libpp.a[pp.o]");
        assert_eq!(name, "pp_sig_cnt");
        assert_eq!(symbol.size, 0x24);
    }

    #[test]
    fn parses_final_posix_symbol() {
        let symbols = parse_posix_symbols("pp_sig_cnt D 2f01750c 24\n");
        assert_eq!(symbols["pp_sig_cnt"].address, 0x2f01_750c);
        assert_eq!(symbols["pp_sig_cnt"].size, 0x24);
    }

    #[test]
    fn parses_wide_section_table() {
        let sections = parse_sections(
            "  [ 7] .critical.data.wifi_strict.commands PROGBITS 2f017668 017858 00432c 00 WA 0 0 4\n",
        );
        assert_eq!(sections[0].name, ".critical.data.wifi_strict.commands");
        assert_eq!(sections[0].address, 0x2f01_7668);
        assert_eq!(sections[0].size, 0x432c);
    }

    #[test]
    fn recognizes_function_definitions_and_memory() {
        assert_eq!(
            definition_name("00000000 <wDev_ProcessRxSucData>:"),
            Some("wDev_ProcessRxSucData")
        );
        assert_eq!(placement(0x2f01_0000), "internal SRAM");
        assert_eq!(placement(0x5000_0000), "PSRAM");
    }

    #[test]
    fn recognizes_the_complete_rev0_rom_code_segment() {
        assert_eq!(target_placement(0x2f80_0000), "ROM export");
        assert_eq!(target_placement(0x2f82_b9f8), "ROM export");
        assert_eq!(target_placement(0x2f83_f6ff), "ROM export");
        assert_eq!(target_placement(0x2f83_f700), "internal SRAM");
    }

    #[test]
    fn rust_owned_phy_callback_alias_requires_exact_sram_backing() {
        let symbols = BTreeMap::from([
            (
                "g_phyFuns".to_owned(),
                Symbol {
                    address: 0x2f06_7a90,
                    size: 0,
                    kind: 'D',
                },
            ),
            (
                "wifi_strict_phy_rom_function_table_binding".to_owned(),
                Symbol {
                    address: 0x2f06_7a90,
                    size: 4,
                    kind: 'D',
                },
            ),
        ]);
        let sections = vec![Section {
            name: ".critical.data.wifi_strict.phy_rom_function_table_binding".to_owned(),
            address: 0x2f06_7a90,
            size: 4,
        }];

        assert_eq!(
            validate_rust_owned_data_aliases(&symbols, &sections).unwrap(),
            BTreeSet::from(["g_phyFuns".to_owned()])
        );

        let mut wrong_address = symbols.clone();
        wrong_address.get_mut("g_phyFuns").unwrap().address += 4;
        assert!(validate_rust_owned_data_aliases(&wrong_address, &sections).is_err());
    }

    #[test]
    fn wrapped_vendor_boundary_is_not_a_reachable_state_owner() {
        let calls = BTreeMap::from([
            (
                "wDev_ProcessRxSucData".to_owned(),
                BTreeSet::from(["esp_test_set_rx_error_occurs".to_owned()]),
            ),
            (
                "esp_test_set_rx_error_occurs".to_owned(),
                BTreeSet::from(["vendor_body_child".to_owned()]),
            ),
        ]);

        let reachable = reachable_vendor_functions(&calls);
        assert!(reachable.contains("wDev_ProcessRxSucData"));
        assert!(!reachable.contains("esp_test_set_rx_error_occurs"));
        assert!(!reachable.contains("vendor_body_child"));
    }

    #[test]
    fn outside_state_lists_only_final_link_code_referrers() {
        let referrers = BTreeSet::from([
            "cold_live".to_owned(),
            "rom_live".to_owned(),
            "discarded_archive_function".to_owned(),
            "linked_data".to_owned(),
        ]);
        let final_symbols = BTreeMap::from([
            (
                "cold_live".to_owned(),
                Symbol {
                    address: 0x4000_0000,
                    size: 4,
                    kind: 'T',
                },
            ),
            (
                "rom_live".to_owned(),
                Symbol {
                    address: 0x2f80_1000,
                    size: 0,
                    kind: 'A',
                },
            ),
            (
                "linked_data".to_owned(),
                Symbol {
                    address: 0x2f00_0000,
                    size: 4,
                    kind: 'D',
                },
            ),
        ]);

        assert_eq!(
            linked_code_referrers(&referrers, &final_symbols),
            BTreeSet::from(["cold_live".to_owned(), "rom_live".to_owned()])
        );
    }

    #[test]
    fn local_labels_preserve_the_current_archive_function_owner() {
        let mut inventory = ArchiveInventory::default();
        parse_archive_relocations(
            "libs/libphy.a(phy_init.o):\tfile format elf32-littleriscv\n\
             00000000 <register_chipv7_phy>:\n\
             \t20: R_RISCV_CALL phy_get_romfunc_addr\n\
             000000b2 <.L90>:\n\
             \tb2: R_RISCV_CALL register_chipv7_phy_init_param\n",
            "libphy.a",
            &BTreeMap::new(),
            &mut inventory,
        );

        assert_eq!(
            inventory.calls["register_chipv7_phy"],
            BTreeSet::from([
                "phy_get_romfunc_addr".to_owned(),
                "register_chipv7_phy_init_param".to_owned(),
            ])
        );
    }

    #[test]
    fn resolves_member_local_data_anchor_to_global_state() {
        let owner = "libphy.a[phy_init.o]".to_owned();
        let symbols = BTreeMap::from([(
            owner,
            vec![
                (
                    ".LANCHOR0".to_owned(),
                    Symbol {
                        address: 0,
                        size: 0,
                        kind: 'd',
                    },
                ),
                (
                    "phy_param".to_owned(),
                    Symbol {
                        address: 0,
                        size: 508,
                        kind: 'D',
                    },
                ),
            ],
        )]);
        let aliases = local_data_aliases(&symbols);
        let mut inventory = ArchiveInventory::default();
        parse_archive_relocations(
            "libs/libphy.a(phy_init.o):\tfile format elf32-littleriscv\n\
             00000000 <register_chipv7_phy>:\n\
             \t72: R_RISCV_HI20 .LANCHOR0\n",
            "libphy.a",
            &aliases,
            &mut inventory,
        );

        assert_eq!(
            inventory.references["register_chipv7_phy"],
            BTreeSet::from(["phy_param".to_owned()])
        );
    }
}
