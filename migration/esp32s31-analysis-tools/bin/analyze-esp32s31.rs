use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{bail, Context, Result};

const EVENT_ACTIONS: [&str; 34] = [
    "ppProcessTxQ(0)",
    "ppProcessTxQ(1)",
    "ppProcessTxQ(2)",
    "ppProcessTxQ(3)",
    "ppProcessTxQ(4)",
    "g_net80211_tx_func()",
    "g_config_func(argument)",
    "g_timer_func(argument)",
    "pp_timer_do_process(argument)",
    "pp_default_event_handler(kind, argument)",
    "pp_default_event_handler(kind, argument)",
    "pp_default_event_handler(kind, argument)",
    "pp_default_event_handler(kind, argument)",
    "ppProcessRxPktHdr(argument)",
    "fatal log + non-returning loop",
    "shutdown and drain",
    "ppProcTxDone(1)",
    "ppRxPkt()",
    "ppResortTxAMPDU(argument as u8)",
    "no-op",
    "no-op",
    "no-op",
    "lmacProcessTxTimeout()",
    "lmacProcessTxComplete()",
    "lmacProcessCollisions_task()",
    "wdevProcessRxSucDataAll()",
    "pm_on_tbtt(argument)",
    "pm_on_tsf_timer(argument)",
    "pp_default_event_handler(kind, argument)",
    "pm_on_beacon_rx(0, 0, 0, 1)",
    "wifi_process_bsscolor_collision()",
    "pm_on_mac_modem_beacon_miss(argument)",
    "wdevProcessModemStateRxBeacon(argument)",
    "pm_on_coex_preemption_end(argument)",
];

const EXPECTED_LABELS: [&str; 34] = [
    ".L1042", ".L1042", ".L1042", ".L1042", ".L1041", ".L1040", ".L1039", ".L1038", ".L1037",
    ".L1017", ".L1017", ".L1017", ".L1017", ".L1036", ".L1035", ".L1034", ".L1033", ".L1032",
    ".L1031", ".L1030", ".L1030", ".L1030", ".L1029", ".L1028", ".L1027", ".L1026", ".L1025",
    ".L1024", ".L1017", ".L1023", ".L1022", ".L1021", ".L1020", ".L1018",
];

const REQUIRED_GLOBALS: [(&str, &str); 53] = [
    ("ppTask", "0000023a"),
    ("pp_post", "00000160"),
    ("pp_sig_cnt", "00000024"),
    ("ppProcessTxQ", "0000018e"),
    ("ppProcessRxPktHdr", "00000042"),
    ("ppProcTxDone", "000001f4"),
    ("ppRxPkt", "00000178"),
    ("ppRxProtoProc", "00000154"),
    ("ppRecycleRxPkt", "0000000e"),
    ("ppResortTxAMPDU", "000005bc"),
    ("pp_default_event_handler", "0000002c"),
    ("pp_timer_do_process", "000000ba"),
    ("pp_create_task", "000001e8"),
    ("pp_delete_task", "00000094"),
    ("lmacProcessTxTimeout", "00000062"),
    ("lmacDisableTransmit", "000000ae"),
    ("lmacDiscardFrameExchangeSequence", "000000d6"),
    ("lmacDiscardMSDU", "000000d2"),
    ("pp_coex_tx_release", "00000074"),
    ("esf_buf_recycle", "00000156"),
    ("lmacTxDone", "000000fc"),
    ("hal_mac_get_txq_complete", "0000081e"),
    ("lmacProcessTxComplete", "0000025c"),
    ("lmacProcessTxSuccess", "00000102"),
    ("lmacProcessTxRtsError", "00000152"),
    ("lmacProcessCtsTimeout", "00000070"),
    ("lmacProcessTxError", "000000f8"),
    ("lmacProcessAckTimeout", "0000013c"),
    ("ppProcTxCallback", "0000006e"),
    ("ppEnqueueTxDone", "00000062"),
    ("rcUpdateTxDone", "000000a0"),
    ("rc_get_trc", "00000076"),
    ("rcUpdateRxDone", "00000066"),
    // Allocation-free static-buffer/key branches used as strict backend
    // preconditions. Their exact control flow is part of the pinned ABI.
    ("esf_buf_alloc", "00000184"),
    ("ic_set_key", "00000044"),
    ("wDev_Insert_KeyEntry", "0000008e"),
    ("hal_crypto_set_key_entry", "000001c2"),
    ("esp_wifi_internal_free_rx_buffer", "00000008"),
    ("ic_get_next_tbtt", "00000008"),
    ("wDev_record_ftm_data", "00000022"),
    ("pm_on_beacon_rx", "00000162"),
    ("pm_on_data_tx", "00000008"),
    ("dbg_read_tx_ppdu", "000003b6"),
    ("dbg_dump_rx_ppdu", "00000a36"),
    ("dbg_dump_rx_sigb", "000001e6"),
    ("wifi_gpio_debug", "0000000e"),
    ("esp_test_tx_enab_statistics", "00000126"),
    ("wDev_ftm_set_t1t4", "0000000e"),
    ("wDev_isNANPktInValidSlot", "00000026"),
    // RX ownership contract: the outer walk passes its current bit-30
    // descriptor to the aggregate decoder, which retains it as the recycle
    // tail while obtaining the unit head independently from wDevCtrl.
    ("get_sublen_offset", "00000146"),
    ("wdevProcessRxSucDataAll", "00000150"),
    ("wDev_ProcessRxSucData", "000006a0"),
    ("wDev_DiscardFrame", "00000020"),
];

const REQUIRED_WPA_SYMBOLS: [(&str, &str); 42] = [
    ("eloop_run", "0000012a"),
    ("eloop_run_timer", "00000024"),
    ("eloop_lifecycle_lock", "00000086"),
    ("eloop_destroy", "000001c4"),
    ("wpa2_task", "000002c2"),
    ("wpa_michael_mic_failure", "000000a0"),
    ("wpa_supplicant_stop_countermeasures", "0000003e"),
    ("wpa_sm_key_request", "00000146"),
    ("wpa_set_pmk", "0000008c"),
    ("eap_start_eapol", "0000000c"),
    ("wpa2_set_eap_state", "00000018"),
    ("eap_sm_process_request", "0000020c"),
    ("gWpaSm", "00000488"),
    ("wpa_cb", "00000004"),
    ("gEapSm", "00000004"),
    ("s_wpa2_rxq", "00000008"),
    ("s_wpa2_queue", "00000004"),
    ("s_wpa2_data_lock", "00000004"),
    ("eapol_txcb", "00000182"),
    // WPA2-Personal STA/AP entry points. These sizes pin the exact blob ABI
    // whose allocating/delaying paths are classified by the strict auditor.
    ("esp_supplicant_init", "0000014c"),
    ("wpa_sm_notify_assoc", "00000082"),
    ("wpa_sta_connected_cb", "00000008"),
    ("wpa_sta_disconnected_cb", "000000c0"),
    ("wpa_sta_in_4way_handshake", "0000001a"),
    ("wpa_sm_rx_eapol", "000009e6"),
    ("wpa_receive", "000005b6"),
    ("wpa_ap_rx_eapol", "00000020"),
    ("hostap_init", "00000328"),
    ("hostap_deinit", "00000036"),
    ("wpa_ap_get_wpa_ie", "00000028"),
    ("hostap_new_assoc_sta", "0000011a"),
    ("hostap_sta_join", "00000114"),
    ("wpa_ap_remove", "00000070"),
    ("wpa_ap_get_peer_spp_msg", "00000018"),
    ("esp_send_assoc_resp", "0000008c"),
    ("wpa_auth_sta_associated", "00000074"),
    ("wpa_pmk_to_ptk", "0000018c"),
    ("wpa_eapol_key_mic", "0000007a"),
    ("wpa_eapol_key_send", "000000b0"),
    ("wpa_sm_set_key", "00000094"),
    ("hostapd_send_eapol", "0000008c"),
    ("__wpa_send_eapol", "0000038e"),
];

const REQUIRED_NET80211_SYMBOLS: [(&str, &str); 46] = [
    ("esp_wifi_ipc_internal", "00000154"),
    ("ieee80211_ioctl", "000001e4"),
    ("wifi_ipc_process", "00000078"),
    ("ieee80211_output_process", "000002ac"),
    ("ieee80211_ioctl_process", "0000011a"),
    ("ieee80211_timer_process", "000000fa"),
    ("ieee80211_timer_do_process", "000000b0"),
    ("g_timer_info", "00000178"),
    ("ieee80211_tx_mgt_cb", "00000228"),
    ("ieee80211_hostapd_beacon_txcb", "000000f0"),
    ("ieee80211_hostapd_data_txcb", "0000006e"),
    ("ieee80211_hostapd_ps_txcb", "00000048"),
    ("cnx_probe_rc_tx_cb", "00000084"),
    ("sta_eapol_txdone_cb", "000000e0"),
    ("esp_wifi_register_eapol_txdonecb_internal", "00000014"),
    ("esp_wifi_internal_reg_rxcb", "00000076"),
    ("wifi_sta_reg_rxcb", "0000000a"),
    ("wifi_ap_reg_rxcb", "0000000a"),
    // WPA3 is deliberately not implemented by this runtime. These symbols
    // pin the observed boundary: net80211 has SAE glue, while the local WPA
    // callback table installed by this S31 supplicant leaves its SAE slots
    // empty.
    ("esp_wifi_register_wpa_cb_internal", "00000022"),
    ("sta_is_wpa3_enabled", "00000020"),
    ("sta_auth_sae", "000001a4"),
    ("esp_wifi_ap_notify_node_sae_auth_done", "00000054"),
    ("wifi_ap_sta_sae_auth_done_process", "000000fa"),
    ("esp_wifi_internal_tx", "00000036"),
    ("esp_wifi_set_key_internal", "00000056"),
    ("esp_wifi_set_sta_key_internal", "0000006e"),
    ("esp_wifi_set_ap_key_internal", "000001bc"),
    ("ppInstallKey", "0000017e"),
    ("ieee80211_output_do", "000001aa"),
    ("ieee80211_alloc_tx_buf", "0000007e"),
    ("ieee80211_search_node", "000000f6"),
    ("ieee80211_post_hmac_tx", "00000086"),
    ("wifi_init_key", "00000030"),
    ("ieee80211_set_key", "00000064"),
    ("ieee80211_get_ptk", "0000001c"),
    ("ieee80211_get_spp", "0000003c"),
    ("ieee80211_get_key", "0000001e"),
    ("ieee80211_set_sta_gtk_index", "0000001a"),
    ("ccmp", "00000018"),
    ("wifi_send_mgmt_frame", "000001f4"),
    ("ieee80211_getmgtframe", "0000005c"),
    ("ieee80211_assoc_resp_construct", "000003ca"),
    ("cnx_node_search", "000000b6"),
    ("ieee80211_set_tx_desc", "00000268"),
    ("ieee80211_set_tx_pti", "00000038"),
    ("ieee80211_mgmt_output", "000002a0"),
];

const REQUIRED_COEX_SYMBOLS: [(&str, &str); 3] = [
    ("coex_pti_get", "00000008"),
    ("coex_core_pti_get", "0000001e"),
    ("coex_pti_tab", "00000030"),
];

const BLOCKING_SYMBOLS: [&str; 20] = [
    "vTaskDelay",
    "sleep",
    "usleep",
    "ets_delay_us",
    "esp_event_post",
    "nvs_commit",
    "nvs_set_blob",
    "nvs_erase_key",
    "puts",
    "putchar",
    "__assert_func",
    "malloc",
    "calloc",
    "realloc",
    "free",
    "xQueueReceive",
    "xSemaphoreTake",
    "xEventGroupWaitBits",
    "gettimeofday",
    "abort",
];

fn main() -> Result<()> {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask must be inside the workspace")?
        .to_path_buf();
    let library_dir = workspace.join("esp-wifi-sys-esp32s31/libs");
    let pp_archive = library_dir.join("libpp.a");
    let wpa_archive = library_dir.join("libwpa_supplicant.a");
    let net80211_archive = library_dir.join("libnet80211.a");
    let coexist_archive = library_dir.join("libcoexist.a");
    let temporary =
        env::temp_dir().join(format!("esp-wifi-s31-blob-analysis-{}", std::process::id()));
    if temporary.exists() {
        fs::remove_dir_all(&temporary)?;
    }
    fs::create_dir(&temporary)?;

    let result = analyze(
        &workspace,
        &library_dir,
        &pp_archive,
        &wpa_archive,
        &net80211_archive,
        &coexist_archive,
        &temporary,
    );
    fs::remove_dir_all(&temporary)?;
    result
}

fn analyze(
    workspace: &Path,
    library_dir: &Path,
    pp_archive: &Path,
    wpa_archive: &Path,
    net80211_archive: &Path,
    coexist_archive: &Path,
    temporary: &Path,
) -> Result<()> {
    checked(
        Command::new("llvm-ar")
            .current_dir(temporary)
            .arg("x")
            .arg(pp_archive)
            .arg("pp.o"),
    )?;
    let object = temporary.join("pp.o");

    let relocations = text(checked(
        Command::new("llvm-readelf").arg("-r").arg(&object),
    )?)?;
    let labels = parse_pp_jump_table(&relocations)?;
    if labels.as_slice() != EXPECTED_LABELS {
        bail!("ppTask jump table changed\nexpected: {EXPECTED_LABELS:?}\nactual:   {labels:?}");
    }

    let symbols = text(checked(
        Command::new("llvm-nm")
            .arg("-S")
            .arg("--defined-only")
            .arg(pp_archive),
    )?)?;
    validate_required_symbols(&symbols, &REQUIRED_GLOBALS, false)?;

    let wpa_symbols = text(checked(
        Command::new("llvm-nm")
            .arg("-S")
            .arg("--defined-only")
            .arg(wpa_archive),
    )?)?;
    validate_required_symbols(&wpa_symbols, &REQUIRED_WPA_SYMBOLS, true)?;

    checked(
        Command::new("llvm-ar")
            .current_dir(temporary)
            .arg("x")
            .arg(wpa_archive)
            .arg("wpa.c.obj")
            .arg("esp_eap_client.c.obj")
            .arg("esp_wpa_main.c.obj"),
    )?;
    let linked_locals = temporary.join("wpa-async-locals.o");
    checked(
        Command::new("ld.lld")
            .arg("-r")
            .arg("-m")
            .arg("elf32lriscv")
            .arg("-T")
            .arg(workspace.join("esp-wifi-async-runtime-esp32s31/ld/esp32s31-eap-locals.x"))
            .arg("-T")
            .arg(workspace.join("esp-wifi-async-runtime-esp32s31/ld/esp32s31-wpa-locals.x"))
            .arg("-T")
            .arg(workspace.join("esp-wifi-async-runtime-esp32s31/ld/esp32s31-wpa2-ap-locals.x"))
            .arg("-T")
            .arg(workspace.join("esp-wifi-async-runtime-esp32s31/ld/esp32s31-wpa2-sta-locals.x"))
            .arg("-o")
            .arg(&linked_locals)
            .arg(temporary.join("esp_eap_client.c.obj"))
            .arg(temporary.join("esp_wpa_main.c.obj"))
            .arg(temporary.join("wpa.c.obj")),
    )?;
    let local_aliases = text(checked(Command::new("llvm-nm").arg(&linked_locals))?)?;
    validate_linker_span(
        &local_aliases,
        "__esp_wpa_sm_key_request",
        "__esp_wpa_sm_key_request_end",
        0x146,
    )?;
    validate_linker_span(
        &local_aliases,
        "__esp_eap_start_eapol",
        "__esp_eap_start_eapol_end",
        0x0c,
    )?;
    validate_linker_span(
        &local_aliases,
        "__esp_hostap_sta_join",
        "__esp_hostap_sta_join_end",
        0x114,
    )?;
    validate_linker_span(
        &local_aliases,
        "__esp_wpa_sta_connected_cb",
        "__esp_wpa_sta_connected_cb_end",
        0x08,
    )?;
    validate_linker_span(
        &local_aliases,
        "__esp_wpa_sta_disconnected_cb",
        "__esp_wpa_sta_disconnected_cb_end",
        0xc0,
    )?;
    validate_linker_span(
        &local_aliases,
        "__esp_wpa2_set_eap_state",
        "__esp_wpa2_set_eap_state_end",
        0x18,
    )?;
    for (start, end, size) in [
        ("__esp_s_wpa2_rxq", "__esp_s_wpa2_rxq_end", 8),
        (
            "__esp_s_wifi_wpa2_sync_sem",
            "__esp_s_wifi_wpa2_sync_sem_end",
            4,
        ),
        ("__esp_s_wpa2_queue", "__esp_s_wpa2_queue_end", 4),
        ("__esp_g_eap_sm", "__esp_g_eap_sm_end", 4),
        ("__esp_s_wpa2_data_lock", "__esp_s_wpa2_data_lock_end", 4),
    ] {
        validate_linker_span(&local_aliases, start, end, size)?;
    }

    let net80211_symbols = text(checked(
        Command::new("llvm-nm")
            .arg("-S")
            .arg("--defined-only")
            .arg(net80211_archive),
    )?)?;
    validate_required_symbols(&net80211_symbols, &REQUIRED_NET80211_SYMBOLS, true)?;

    let coexist_symbols = text(checked(
        Command::new("llvm-nm")
            .arg("-S")
            .arg("--defined-only")
            .arg(coexist_archive),
    )?)?;
    validate_required_symbols(&coexist_symbols, &REQUIRED_COEX_SYMBOLS, true)?;

    checked(
        Command::new("llvm-ar")
            .current_dir(temporary)
            .arg("x")
            .arg(net80211_archive)
            .arg("ieee80211_hostap.o")
            .arg("ieee80211_ht.o"),
    )?;
    let linked_net80211_locals = temporary.join("net80211-async-locals.o");
    checked(
        Command::new("ld.lld")
            .arg("-r")
            .arg("-m")
            .arg("elf32lriscv")
            .arg("-T")
            .arg(workspace.join("esp-wifi-async-runtime-esp32s31/ld/esp32s31-net80211-locals.x"))
            .arg("-o")
            .arg(&linked_net80211_locals)
            .arg(temporary.join("ieee80211_hostap.o"))
            .arg(temporary.join("ieee80211_ht.o")),
    )?;
    let net80211_local_aliases = text(checked(
        Command::new("llvm-nm").arg(&linked_net80211_locals),
    )?)?;
    validate_linker_span(
        &net80211_local_aliases,
        "__esp_s31_beacon_next_tbtt",
        "__esp_s31_beacon_next_tbtt_end",
        4,
    )?;
    validate_linker_span(
        &net80211_local_aliases,
        "__esp_s31_ap_rxcb",
        "__esp_s31_ap_rxcb_end",
        4,
    )?;

    let digest = text(checked(Command::new("sha256sum").arg(pp_archive))?)?;
    let digest = digest
        .split_whitespace()
        .next()
        .context("sha256sum returned no digest")?;
    let wpa_digest = text(checked(Command::new("sha256sum").arg(wpa_archive))?)?;
    let wpa_digest = wpa_digest
        .split_whitespace()
        .next()
        .context("sha256sum returned no WPA digest")?;
    let coexist_digest = text(checked(Command::new("sha256sum").arg(coexist_archive))?)?;
    let coexist_digest = coexist_digest
        .split_whitespace()
        .next()
        .context("sha256sum returned no coexistence digest")?;
    let blocking = blocking_references(library_dir)?;
    let report = report(digest, wpa_digest, coexist_digest, &labels, &blocking);

    if env::args().any(|argument| argument == "--write") {
        let path = workspace.join("docs/esp32s31-async-runtime-audit.md");
        fs::write(&path, &report)?;
        println!("wrote {}", path.display());
    } else {
        print!("{report}");
    }
    Ok(())
}

fn parse_pp_jump_table(readelf: &str) -> Result<Vec<&str>> {
    let mut in_table = false;
    let mut labels = Vec::new();

    for line in readelf.lines() {
        if line.starts_with("Relocation section '.rela.rodata'") {
            in_table = true;
            continue;
        }
        if in_table && line.starts_with("Relocation section '") {
            break;
        }
        if !in_table || !line.contains("R_RISCV_32") {
            continue;
        }

        let fields = line.split_whitespace().collect::<Vec<_>>();
        let offset = usize::from_str_radix(fields.first().context("missing offset")?, 16)?;
        let label = *fields.get(4).context("missing relocation symbol")?;
        if offset != labels.len() * 4 {
            bail!("non-contiguous ppTask table at offset {offset:#x}");
        }
        labels.push(label);
    }

    if labels.len() != EVENT_ACTIONS.len() {
        bail!(
            "expected {} ppTask events, found {}",
            EVENT_ACTIONS.len(),
            labels.len()
        );
    }
    Ok(labels)
}

fn validate_required_symbols(
    nm: &str,
    required: &[(&str, &str)],
    allow_local_text: bool,
) -> Result<()> {
    let mut symbols = BTreeMap::new();
    for line in nm.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() >= 4 {
            let fields = &fields[fields.len() - 4..];
            symbols.insert(fields[3], (fields[1], fields[2]));
        }
    }

    for &(name, expected_size) in required {
        let Some((size, visibility)) = symbols.get(name) else {
            bail!("required symbol {name} is missing");
        };
        let valid_visibility = matches!(*visibility, "T" | "W" | "D" | "B")
            || (allow_local_text && matches!(*visibility, "t" | "d" | "b"));
        if *size != expected_size || !valid_visibility {
            bail!(
                "symbol {name} changed: expected size {expected_size}, got {size} ({visibility})"
            );
        }
    }
    Ok(())
}

fn validate_linker_span(nm: &str, start: &str, end: &str, expected: u64) -> Result<()> {
    let mut addresses = BTreeMap::new();
    for line in nm.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() >= 3 {
            addresses.insert(fields[fields.len() - 1], fields[0]);
        }
    }
    let start_address = u64::from_str_radix(
        addresses
            .get(start)
            .with_context(|| format!("linker alias {start} is missing"))?,
        16,
    )?;
    let end_address = u64::from_str_radix(
        addresses
            .get(end)
            .with_context(|| format!("linker alias {end} is missing"))?,
        16,
    )?;
    let actual = end_address.wrapping_sub(start_address);
    if actual != expected {
        bail!("linker alias span {start}..{end} changed: expected {expected:#x}, got {actual:#x}");
    }
    Ok(())
}

fn blocking_references(library_dir: &Path) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut archives = fs::read_dir(library_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "a"))
        .collect::<Vec<_>>();
    archives.sort();

    let mut found = BTreeMap::<String, BTreeSet<String>>::new();
    for archive in archives {
        let output = text(checked(
            Command::new("llvm-nm")
                .arg("-A")
                .arg("-P")
                .arg("--undefined-only")
                .arg(&archive),
        )?)?;
        for line in output.lines() {
            for symbol in BLOCKING_SYMBOLS {
                if line.split_whitespace().any(|field| field == symbol) {
                    let relative = line
                        .strip_prefix(&format!(
                            "{}{}",
                            library_dir.display(),
                            std::path::MAIN_SEPARATOR
                        ))
                        .unwrap_or(line);
                    let source = relative
                        .split_once("]: ")
                        .map(|(member, _)| format!("{member}]"))
                        .unwrap_or_else(|| relative.to_owned());
                    found.entry(symbol.to_owned()).or_default().insert(source);
                }
            }
        }
    }
    Ok(found)
}

fn report(
    digest: &str,
    wpa_digest: &str,
    coexist_digest: &str,
    labels: &[&str],
    blocking: &BTreeMap<String, BTreeSet<String>>,
) -> String {
    let mut output = format!(
        "# ESP32-S31 stackless Wi-Fi runtime audit\n\n\
         Generated by `cargo +stable run -p xtask --bin analyze-esp32s31 -- --write`.\n\n\
         - `libpp.a` SHA-256: `{digest}`\n\
         - `libwpa_supplicant.a` SHA-256: `{wpa_digest}`\n\
         - `libcoexist.a` SHA-256: `{coexist_digest}`\n\
         - `ppTask` size: `0x23a` bytes\n\
         - queue item: 8 bytes (`u32 kind`, `void *argument`)\n\
         - jump table: 34 entries\n\
         - signal counters: exported `pp_sig_cnt[36]`\n\n\
         ## Recovered `ppTask` dispatch\n\n\
         | Event | Target label | Run-to-completion action |\n\
         |---:|---|---|\n"
    );
    for (event, (label, action)) in labels.iter().zip(EVENT_ACTIONS).enumerate() {
        output.push_str(&format!("| {event} | `{label}` | `{action}` |\n"));
    }
    output.push_str(
        "\nEvents 34 and above use `pp_default_event_handler`. After a successful receive, \
         `ppTask` decrements `pp_sig_cnt[kind]` for kinds 0 through 35 except kind 13. \
         The Rust dispatcher must preserve this accounting under the Wi-Fi interrupt lock.\n\n\
         Event 14 is deliberately reported as a runtime error instead of reproducing the vendor \
         infinite loop. Event 15 is a lifecycle boundary handled by the Rust runtime. Event 13 \
         is rejected before its promiscuous callback and two OSI frees. Events 5 through 7 are \
         strict boundaries: the stock output handler drains a shared list, the ioctl envelope \
         carries an arbitrary callback plus heap/semaphore/PM ownership, and the timer envelope \
         is heap allocated. A final-link wrapper replaces the timer producer with sixteen fixed \
         slots and a private one-action event. Only timer ID 0 is completed locally; `chm_dwell` \
         and all recovery timers fail closed because downstream paths contain dynamic callbacks, \
         OSI synchronization, MAC teardown, or synchronous channel switching.\n\n\
         ## WPA execution boundaries\n\n\
         `eloop_run` is a finite `0x12a`-byte function. It locks the eloop state, processes due \
         timeout callbacks, rearms an OS timer, unlocks, and returns. The OS timer callback \
         reaches `esp_wifi_ipc_internal`, which reaches `ieee80211_ioctl`. Outside Wi-Fi task \
         identity that ioctl posts PP event 6 and waits indefinitely on an OSI semaphore; under \
         the virtual PP identity it executes inline. Timer callbacks must therefore run from the \
         Rust Wi-Fi future, never directly from a hardware ISR or a foreign timer task.\n\n\
         `wpa2_task` is a separate `0x2c2`-byte Enterprise EAP worker. Its infinite \
         `_queue_recv(..., UINT32_MAX)` loop receives 8-byte messages and selects only kinds 0, \
         1, and 2. Kind 0 emits EAPOL-Start, kind 1 drains the private RX list, and kind 2 deletes \
         the queue and acknowledges teardown. The Rust adapter virtualizes the exact `(3, 8)` \
         queue and task entry, dispatches kinds 0 and 1 on the Wi-Fi future, and handles one RX \
         node per poll. Kind 2 is a bounded inline teardown because the calling vendor deinit \
         frees `gEapSm` immediately after its semaphore handshake.\n\n\
         `eloop_lifecycle_lock` calls `vTaskDelay(1)` only on lock contention. `eloop_destroy` \
         calls `vTaskDelay(10)` only while callbacks are in flight. A single stackless executor \
         prevents ordinary overlap, but these direct imports remain forbidden-path probes.\n\n\
         `wpa_michael_mic_failure` directly calls `os_sleep(0, 0, 10000, 0)` on the second \
         Michael MIC failure before registering the 60-second countermeasure timeout. The async \
         callback replacement preserves the state transitions and local `wpa_sm_key_request`, \
         then schedules that final timeout registration as a 10 ms continuation in the shared \
         timer pool. The link-time alias is guarded by the archive digest and exact `0x146`-byte \
         local-function size.\n\n\
         The pinned stock association bodies are `wpa_sm_notify_assoc` (`0x82`), \
         `hostap_init` (`0x328`), `hostap_new_assoc_sta` (`0x11a`), and `wpa_receive` \
         (`0x5b6`). The STA association callback remains a forbidden strict audit root. The \
         AP bodies are size-pinned but cease to be roots after the complete AP callback group \
         is replaced. The EAPOL receive entries no longer enter either stock state machine: \
         final-link wrappers for `wpa_sm_rx_eapol` (`0x9e6`) and `wpa_ap_rx_eapol` (`0x20`) \
         copy one validated packet plus peer identity into an eight-slot Rust channel.\n\n\
         The AP callback table is patched before AP start. `hostap_init`, `hostap_deinit`, \
         `hostap_sta_join`, `wpa_ap_remove`, `wpa_ap_get_wpa_ie`, and peer-SPP lookup are \
         replaced together, so no heap-backed hostap authenticator, station object, or eloop \
         rekey timer exists in the strict phase. A validated WPA2-PSK/CCMP RSN IE is held in \
         fixed Rust storage for beacon/association construction. Join claims one pinned \
         `0x28` station slot, copies an owned association event, and sends only subtype \
         `0x10`/`0x30` through `ieee80211_assoc_resp_construct`, fixed management buffers, and \
         `ieee80211_mgmt_output`; invalid RSN, missing node, or pool/channel exhaustion fails \
         immediately. The final link also makes `pm_on_data_tx` a no-op under the verified \
         `WIFI_PS_NONE` invariant, excluding the management TX power-save tail.\n\n\
         The stock AP TX wrapper `hostapd_send_eapol` (`0x8c`) allocates and frees an Ethernet \
         frame around `esp_wifi_internal_tx`. `esp_wifi_set_ap_key_internal` (`0x1bc`) and the \
         non-delete STA `ppInstallKey` branch allocate persistent software key objects. The \
         Rust handshake therefore builds owned M1-M4/Ethernet frames and emits owned TX/key \
         commands instead of entering those wrappers. `Wpa2IoQueue` transfers them to one \
         radio owner; its `TryWpa2Io` backend makes exactly one attempt and must return command \
         ownership on backpressure.\n\n\
         The pinned `hal_crypto_set_key_entry` (`0x1c2`) enters a malloc/copy/free branch when \
         the supplied key is not four-byte aligned. The mandatory final-link wrapper instead \
         reproduces its fixed key-table MMIO writes for at most 32 bytes without allocating, \
         while the surrounding vendor bookkeeping remains. This does not make the stock STA/AP \
         wrappers strict, because their separate software-key allocations remain. \
         `S31StaticWpa2Io` instead uses a pinned `0xb8` net80211 CCMP object in caller-provided \
         static storage. It reads the pinned `g_ic` key slot directly and proceeds only when the \
         slot is null or already equals that same object, then performs the exact pointer store; \
         neither the getter nor potentially freeing vendor setter remains a runtime root. AP \
         PTK/SPP lookup uses a mandatory finite `cnx_node_search` wrapper over the nine static \
         AP/BSS node entries, so its null-interface assert and eight-bit wraparound loop are \
         unreachable. The adjacent `ieee80211_search_node` wrapper accepts STA/AP only and rejects \
         NAN before its assert loop. STA GTK metadata is reduced to recovered bounded byte \
         loads/stores. The pinned `wifi_init_key` body is reproduced as \
         its exact two constant-length fills in Rust-owned storage and is no longer a runtime \
         root. Final-link `esf_buf_alloc/recycle` wrappers use \
         the initialized vendor kind-1 free list under local MIE masking and eight Rust-owned \
         1600-byte slots for management kinds 2 through 4; no strict path enters an OSI mutex, \
         malloc, free, or dynamic-buffer fallback. A management-output link wrapper accepts only \
         ordinary association/authentication/probe subtypes on the home channel and verifies \
         live non-mesh/non-NAN plus AP no-power-save invariants before entering the stock body; \
         rejection recycles its fixed ESF slot. The adjacent TX-PTI wrapper bypasses the OSI \
         callback and reproduces the pinned finite success branch as one bounds-checked volatile \
         byte read from exported `coex_pti_tab[48]`; an invalid event is consumed by the following \
         management gate and recycled once. TX bypasses `esp_wifi_internal_tx`, whose \
         `g_wifi_global_lock` callback can wait: the strict backend performs bounded peer lookup, \
         fixed-pool buffer acquisition, descriptor setup, and `ieee80211_post_hmac_tx` directly. \
         `prepare_strict_runtime` issues a proof only after every registered OSI allocator and \
         critical-section slot exactly equals its Rust replacement and `WIFI_PS_NONE` plus \
         `WIFI_LOG_NONE` have been read back. It requires the current `mhartid` to equal \
         `wifi_task_core_id`, then arms OSI/direct-C allocator guards and replaces strict critical \
         sections with local MIE masking, never a spin lock or other-core stall. The final link wraps \
         `lmacTxDone`, `hal_mac_get_txq_state`, AP-beacon completion, and management \
         completion: the first replaces its inline callback/PM tail, while the TXQ wrapper \
         exposes one completion/collision bitmap bit per executor event without entering the \
         HAL test/log hooks. Strict event 23 also replaces the stock completion loop and its \
         indirect jump table with one fixed MMIO decode and a direct Rust outcome match; the \
         five vendor outcome bodies remain explicit audit roots. The beacon wrapper only \
         rearms its fixed timer. The management \
         wrapper accepts ordinary auth/association/probe completion and rejects disconnect or \
         off-channel action subtypes before their node/key/channel state machines. \
         FTM capability bits are cleared and validated; `wDev_record_ftm_data` is additionally \
         wrapped so an unexpected FTM RX cannot enter its leading `ets_delay_us(50)`. \
         Under the verified `WIFI_PS_NONE` invariant, `pm_on_beacon_rx`, `pm_on_data_rx`, and \
         `pm_set_beacon_duration` are no-op wrappers: net80211 beacon/data parsing and the \
         independent RX rate update remain intact, while the PM TIM/radio-shutdown, \
         modem-sleep timer, Wi-Fi API-lock, and beacon-offset callback tails are absent. \
         The verbose TX/RX PPDU and SIG-B decoders are no-op wrappers under `WIFI_LOG_NONE`, removing \
         their formatting loops and `puts`/`putchar` leaves from normal completion paths. The \
         variadic `wifi_log` dispatcher is also a mandatory no-op wrapper, preventing error \
         paths from invoking a formatter or external logging callback. Optional GPIO tracing \
         and TX test statistics are also no-op wrappers, while the disabled-FTM TX timestamp \
         hook records a strict failure without invoking its registered callback. The NAN \
         valid-slot wrapper preserves the descriptor-kind test, accepts ordinary AP/STA frames, \
         and rejects NAN frames without entering the registered scheduler callback. \
         STA GTK uses the single pinned hardware slot one plus the finite \
         `ieee80211_set_sta_gtk_index` metadata store; AP GTK uses slots eight through eleven. \
         Because the blob exports no independent controlled-port setter, AP authorization is a \
         fixed Rust-owned peer table and ordinary data channels must query it for every frame.\n\n\
         The pinned STA TX completion dispatcher normally calls `eapol_txcb` (`0x182`). \
         Strict integration replaces that registered callback with an owned fixed-channel \
         bridge which accepts only M2/M4 metadata. `prepare_strict_runtime` now requires that \
         registration flag, so the stock callback is not an active strict root. The final link \
         also replaces `pp_post`: before strict mode it delegates to initialization, while the \
         armed path preserves recovered event coalescing and `pp_sig_cnt` rules and makes one \
         fixed-queue push. It rejects event 13 and wrong-hart callers, never enters an OSI queue \
         or yield hook, and rolls the counter back once on queue exhaustion.\n\n\
         The STA connected/disconnected callbacks and four-way-handshake query are also \
         replaced after `esp_supplicant_init`. Linker aliases pin the original local callback \
         sizes (`0x08` and `0xc0`) before the function-table slots are changed. The Rust \
         callbacks copy link events into a fixed channel and maintain an explicit atomic \
         handshake flag; disconnect can no longer enter the stock eloop timeout cancellation \
         path ending in `free`.\n\n\
         Ordinary STA/AP data RX is registered to Rust callbacks before runtime. They copy one \
         frame into an eight-slot, 1600-byte static pool, transfer a slot token through a \
         producer-woken channel, and recycle the vendor RX object immediately. No arbitrary \
         netstack callback executes on the radio stack, and exhaustion returns without retry. \
         The separate promiscuous event 13 is rejected before its optional sniffer callback and \
         OSI-owned payload/envelope frees.\n\n\
         Application TX has a matching fixed-slot channel. Its radio-owner method performs one \
         direct static-buffer submission attempt; AP frames recheck the live Rust controlled \
         port immediately before the peer lookup, while STA uses the same bounded lower path.\n\n\
         WPA3 SAE is not implemented by this runtime. This S31 build declares \
         `WIFI_ENABLE_WPA3_SAE = 0`; the pinned net80211 SAE symbols are glue around callback \
         slots left empty by `esp_supplicant_init`, not a usable local SAE engine.\n\n\
         ## Shutdown boundary\n\n\
         `pp_delete_task` allocates a semaphore, posts event 15, and waits forever for the task \
         acknowledgement before deleting the task and queue. The stackless runtime must post \
         event 15 itself and await the Rust future instead of entering this wrapper. Once event \
         15 has been drained, rejecting a duplicate event makes a subsequent vendor deinit select \
         its existing non-waiting `pp_delete_task_manually` cleanup path.\n\n\
         ## Direct potentially blocking imports\n\n\
         This is a symbol-level inventory, not proof of runtime reachability.\n\n\
         | Symbol | Referencing archive members |\n\
         |---|---|\n",
    );
    for (symbol, sources) in blocking {
        output.push_str(&format!(
            "| `{symbol}` | {} |\n",
            sources
                .iter()
                .map(|source| format!("`{source}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    output
}

fn checked(command: &mut Command) -> Result<Output> {
    let description = format!("{command:?}");
    let output = command
        .output()
        .with_context(|| format!("failed to run {description}"))?;
    if !output.status.success() {
        bail!(
            "{description} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output)
}

fn text(output: Output) -> Result<String> {
    String::from_utf8(output.stdout).context("tool output is not UTF-8")
}
