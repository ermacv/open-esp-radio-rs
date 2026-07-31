use std::{
    ffi::CString,
    fs, io,
    os::fd::{FromRawFd, OwnedFd},
    path::Path,
    thread,
    time::Duration,
};

use open_esp_radio_ieee80211::trigger::{
    BasicTriggerFrameEncoding, TRIGGER_BASIC_FRAME_LEN, TriggerBasicDependentInfo,
    TriggerCommonEncoding, TriggerGiLtf, TriggerScheduledUserEncoding,
};

use crate::{Result, bidirectional::SerialCapture};

const RADIOTAP_HEADER: [u8; 8] = [0, 0, 8, 0, 0, 0, 0, 0];
const ARPHRD_IEEE80211_RADIOTAP: u16 = 803;
const ETH_P_ALL: u16 = 0x0003;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacAddress([u8; 6]);

#[derive(Clone, Debug, Eq, PartialEq)]
struct Options {
    interface: String,
    transmitter: MacAddress,
    receiver: MacAddress,
    association_id: u16,
    uplink_length: u16,
    ru_allocation: u8,
    mcs: u8,
    gi_ltf: TriggerGiLtf,
    ldpc: bool,
    dcm: bool,
    ap_tx_power_encoding: u8,
    target_rssi_encoding: u8,
    count: u32,
    interval: Duration,
    dry_run: bool,
    serial: String,
    baseline: Duration,
    post_injection: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeTbStatistics {
    rx_trigger_count: u16,
    transmission_count: u16,
    qos_null_count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeTbUser {
    association_id: u16,
    ru_allocation: u8,
    mcs: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeTbSample {
    statistics: HeTbStatistics,
    user: Option<HeTbUser>,
    software_schedule_aid: Option<u16>,
    software_schedule_valid: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HilEvidence {
    before: HeTbStatistics,
    after: HeTbStatistics,
    user: HeTbUser,
}

pub(crate) fn run(args: &[String]) -> Result<()> {
    if matches!(
        args.first().map(String::as_str),
        None | Some("help" | "--help" | "-h")
    ) {
        print_help();
        return Ok(());
    }
    let options = parse_options(args, false)?;
    let packet = encode_packet(&options)?;
    if options.dry_run {
        println!(
            "OPENRADIOTRIGGER result=DRY-RUN interface={} bytes={} mpdu={}",
            options.interface,
            packet.len(),
            hex(&packet[RADIOTAP_HEADER.len()..]),
        );
        return Ok(());
    }

    ensure_monitor_interface(&options.interface)?;
    let socket = PacketSocket::bind(&options.interface)?;
    send_packets(&socket, &packet, &options)?;
    println!(
        "OPENRADIOTRIGGER result=SENT interface={} count={} aid={} ru={} mcs={} ldpc={} dcm={}",
        options.interface,
        options.count,
        options.association_id,
        options.ru_allocation,
        options.mcs,
        options.ldpc,
        options.dcm,
    );
    Ok(())
}

pub(crate) fn run_hil(args: &[String], root: &Path) -> Result<()> {
    if matches!(
        args.first().map(String::as_str),
        None | Some("help" | "--help" | "-h")
    ) {
        print_hil_help();
        return Ok(());
    }
    let options = parse_options(args, true)?;
    if options.dry_run {
        return Err("trigger-hil cannot use --dry-run".into());
    }
    let packet = encode_packet(&options)?;
    ensure_monitor_interface(&options.interface)?;
    let socket = PacketSocket::bind(&options.interface)?;

    let capture = SerialCapture::start(Path::new(&options.serial));
    thread::sleep(options.baseline);
    send_packets(&socket, &packet, &options)?;
    thread::sleep(options.post_injection);
    let uart_log = capture.finish();

    let output = root.join("target/hil/esp32s31/qualification/open-radio-trigger-hil");
    fs::create_dir_all(&output)?;
    fs::write(output.join("uart.log"), &uart_log)?;
    let evidence = match qualify_hil(&options, &uart_log) {
        Ok(evidence) => evidence,
        Err(error) => {
            write_hil_report(&output, &options, None, &error.to_string())?;
            return Err(format!(
                "{error}; UART evidence: {}",
                output.join("uart.log").display()
            )
            .into());
        }
    };
    write_hil_report(&output, &options, Some(evidence), "none")?;
    println!(
        "OPENRADIOTRIGGER result=PASS mode=he-tb-hil interface={} \
         sent={} aid={} ru={} mcs={} rx_trigger_delta={} \
         transmission_delta={} qos_null_delta={}",
        options.interface,
        options.count,
        options.association_id,
        options.ru_allocation,
        options.mcs,
        evidence
            .after
            .rx_trigger_count
            .wrapping_sub(evidence.before.rx_trigger_count),
        evidence
            .after
            .transmission_count
            .wrapping_sub(evidence.before.transmission_count),
        evidence
            .after
            .qos_null_count
            .wrapping_sub(evidence.before.qos_null_count),
    );
    println!("report={}", output.join("report.md").display());
    Ok(())
}

fn send_packets(socket: &PacketSocket, packet: &[u8], options: &Options) -> Result<()> {
    for index in 0..options.count {
        socket.send(packet)?;
        if index + 1 != options.count {
            thread::sleep(options.interval);
        }
    }
    Ok(())
}

fn print_help() {
    println!(
        "cargo hil traffic trigger <monitor-interface> \\\n\
           --transmitter <ap-bssid> --aid <association-id> [options]\n\
         \n\
         Options:\n\
           --receiver <mac>          Trigger RA (default: ff:ff:ff:ff:ff:ff)\n\
           --uplink-length <1..4095> HE TB PPDU UL Length field (default: 100)\n\
           --ru <0..127>             raw RU allocation (default: 61 / HE20 RU242)\n\
           --mcs <0..9>              scheduled HE MCS (default: 0)\n\
           --gi <1x1.6|2x1.6|4x3.2> Trigger GI/LTF (default: 2x1.6; numeric\n\
                                      aliases 1.6 and 3.2 are accepted)\n\
           --ldpc                     request LDPC instead of BCC\n\
           --dcm                      request DCM\n\
           --ap-tx-power-dbm <-20..43> (default: 22)\n\
           --target-rssi-dbm <-110..16> (default: reserved/no target)\n\
           --count <1..10000>        number of frames (default: 10)\n\
           --interval-ms <1..1000>   interval between frames (default: 100)\n\
           --dry-run                 encode and print without opening AF_PACKET\n\
         \n\
         HE Trigger Common Info does not encode a 0.8-us GI; its selectors\n\
         are 1xLTF/1.6us, 2xLTF/1.6us, 4xLTF/3.2us and reserved.\n\
         The command emits a radiotap-prefixed, one-user Basic Trigger MPDU.\n\
         The interface must already be a monitor interface on the AP channel;\n\
         AF_PACKET injection requires CAP_NET_RAW or root. The kernel/driver\n\
         adds the FCS because the radiotap header does not claim one."
    );
}

fn print_hil_help() {
    println!(
        "cargo hil traffic trigger-hil <monitor-interface> \\\n\
           --transmitter <ap-bssid> --aid <association-id> [options]\n\
         \n\
         Trigger options are identical to `cargo hil traffic trigger`, with HIL-safe\n\
         defaults of 10,000 frames at a 1-ms interval. Additional options:\n\
           --serial <port>             device UART (default: /dev/ttyACM0)\n\
           --baseline-seconds <10..60> capture a pre-injection counter (default: 11)\n\
           --post-seconds <10..60>     capture a post-injection counter (default: 12)\n\
         \n\
         PASS requires wrapping-positive growth in both the hardware Trigger\n\
         RX and HE-TB transmission counters, plus a latched TB user matching\n\
         the requested AID, RU allocation and MCS and a matching fail-closed\n\
         software Trigger plan. UART evidence and a report are saved under\n\
         target/hil/esp32s31/qualification/open-radio-trigger-hil."
    );
}

fn parse_options(args: &[String], hil: bool) -> Result<Options> {
    let interface = args.first().ok_or("missing monitor interface")?.clone();
    validate_interface_name(&interface)?;
    let mut options = Options {
        interface,
        transmitter: MacAddress([0; 6]),
        receiver: MacAddress([0xff; 6]),
        association_id: 0,
        uplink_length: 100,
        ru_allocation: 61,
        mcs: 0,
        gi_ltf: TriggerGiLtf::TwoLtf1600Ns,
        ldpc: false,
        dcm: false,
        ap_tx_power_encoding: 42,
        target_rssi_encoding: 0x7f,
        count: if hil { 10_000 } else { 10 },
        interval: Duration::from_millis(if hil { 1 } else { 100 }),
        dry_run: false,
        serial: "/dev/ttyACM0".into(),
        baseline: Duration::from_secs(11),
        post_injection: Duration::from_secs(12),
    };
    let mut transmitter_set = false;
    let mut aid_set = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--ldpc" => {
                options.ldpc = true;
                index += 1;
            }
            "--dcm" => {
                options.dcm = true;
                index += 1;
            }
            "--dry-run" => {
                options.dry_run = true;
                index += 1;
            }
            option => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("{option} requires a value"))?;
                match option {
                    "--transmitter" => {
                        options.transmitter = parse_mac(value)?;
                        transmitter_set = true;
                    }
                    "--receiver" => options.receiver = parse_mac(value)?,
                    "--aid" => {
                        options.association_id = parse_bounded(value, 1_u16, 0x0fff, "--aid")?;
                        aid_set = true;
                    }
                    "--uplink-length" => {
                        options.uplink_length =
                            parse_bounded(value, 1_u16, 0x0fff, "--uplink-length")?;
                    }
                    "--ru" => {
                        options.ru_allocation = parse_bounded(value, 0_u8, 0x7f, "--ru")?;
                    }
                    "--mcs" => options.mcs = parse_bounded(value, 0_u8, 9, "--mcs")?,
                    "--gi" => {
                        options.gi_ltf = match value.as_str() {
                            "1x1.6" => TriggerGiLtf::OneLtf1600Ns,
                            "1.6" | "2x1.6" => TriggerGiLtf::TwoLtf1600Ns,
                            "3.2" | "4x3.2" => TriggerGiLtf::FourLtf3200Ns,
                            _ => {
                                return Err("--gi must be 1x1.6, 2x1.6/1.6 or 4x3.2/3.2; \
                                     HE Trigger has no 0.8-us GI"
                                    .into());
                            }
                        };
                    }
                    "--ap-tx-power-dbm" => {
                        let dbm = parse_bounded(value, -20_i16, 43, "--ap-tx-power-dbm")?;
                        options.ap_tx_power_encoding = (dbm + 20) as u8;
                    }
                    "--target-rssi-dbm" => {
                        let dbm = parse_bounded(value, -110_i16, 16, "--target-rssi-dbm")?;
                        options.target_rssi_encoding = (dbm + 110) as u8;
                    }
                    "--count" => {
                        options.count = parse_bounded(value, 1_u32, 10_000, "--count")?;
                    }
                    "--interval-ms" => {
                        let millis = parse_bounded(value, 1_u64, 1_000, "--interval-ms")?;
                        options.interval = Duration::from_millis(millis);
                    }
                    "--serial" if hil => options.serial.clone_from(value),
                    "--baseline-seconds" if hil => {
                        options.baseline = Duration::from_secs(parse_bounded(
                            value,
                            10_u64,
                            60,
                            "--baseline-seconds",
                        )?);
                    }
                    "--post-seconds" if hil => {
                        options.post_injection = Duration::from_secs(parse_bounded(
                            value,
                            10_u64,
                            60,
                            "--post-seconds",
                        )?);
                    }
                    _ => return Err(format!("unknown open-radio trigger option `{option}`").into()),
                }
                index += 2;
            }
        }
    }
    if !transmitter_set {
        return Err("missing --transmitter <ap-bssid>".into());
    }
    if !aid_set {
        return Err("missing --aid <association-id>".into());
    }
    Ok(options)
}

fn qualify_hil(options: &Options, uart_log: &str) -> Result<HilEvidence> {
    let samples = parse_he_tb_samples(uart_log);
    if samples.len() < 2 {
        return Err(format!(
            "expected pre/post he-tb-statistics snapshots, observed {}",
            samples.len()
        )
        .into());
    }
    let before = samples[0].statistics;
    let final_sample = samples.last().expect("length checked above");
    let after = final_sample.statistics;
    let rx_delta = after.rx_trigger_count.wrapping_sub(before.rx_trigger_count);
    if rx_delta == 0 {
        return Err("hardware RX Trigger counter did not advance".into());
    }
    let transmission_delta = after
        .transmission_count
        .wrapping_sub(before.transmission_count);
    if transmission_delta == 0 {
        return Err(format!(
            "hardware accepted {rx_delta} Trigger frame(s), but HE-TB transmission counter did not advance"
        )
        .into());
    }
    let user = final_sample
        .user
        .filter(|user| {
            user.association_id == options.association_id
                && user.ru_allocation == options.ru_allocation
                && user.mcs == options.mcs
        })
        .ok_or_else(|| {
            format!(
                "post-injection HE-TB user did not match aid={} ru={} mcs={}",
                options.association_id, options.ru_allocation, options.mcs
            )
        })?;
    if final_sample.software_schedule_aid != Some(options.association_id)
        || !final_sample.software_schedule_valid
    {
        return Err(format!(
            "post-injection software Trigger plan did not admit AID {}",
            options.association_id
        )
        .into());
    }
    Ok(HilEvidence {
        before,
        after,
        user,
    })
}

fn parse_he_tb_samples(log: &str) -> Vec<HeTbSample> {
    let mut samples = Vec::new();
    for line in log.lines() {
        if line.contains("stage=he-tb-statistics") {
            let Some(statistics) = (|| {
                Some(HeTbStatistics {
                    rx_trigger_count: parse_decimal_after(line, "rx_trigger_count:")?,
                    transmission_count: parse_decimal_after(line, "transmission_count:")?,
                    qos_null_count: parse_decimal_after(line, "qos_null_count:")?,
                })
            })() else {
                continue;
            };
            samples.push(HeTbSample {
                statistics,
                user: None,
                software_schedule_aid: None,
                software_schedule_valid: false,
            });
        } else if line.contains("stage=he-tb-user") {
            let Some(user) = (|| {
                Some(HeTbUser {
                    association_id: parse_decimal_after(line, "aid=")?,
                    ru_allocation: parse_decimal_after(line, "ru=")?,
                    mcs: parse_decimal_after(line, "mcs=")?,
                })
            })() else {
                continue;
            };
            if let Some(sample) = samples.last_mut() {
                sample.user = Some(user);
            }
        } else if line.contains("stage=he-trigger-frame") {
            if let Some(sample) = samples.last_mut() {
                sample.software_schedule_aid = parse_decimal_after(line, "schedule_aid=");
                sample.software_schedule_valid = line.contains("schedule=Some(Ok(");
            }
        }
    }
    samples
}

fn parse_decimal_after<T>(line: &str, marker: &str) -> Option<T>
where
    T: std::str::FromStr,
{
    let tail = line.split_once(marker)?.1.trim_start();
    let digits = tail
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    (digits != 0).then(|| tail[..digits].parse().ok()).flatten()
}

fn write_hil_report(
    directory: &Path,
    options: &Options,
    evidence: Option<HilEvidence>,
    failure: &str,
) -> Result<()> {
    let (result, counters, user) = match evidence {
        Some(evidence) => (
            "PASS",
            format!(
                "- RX Trigger counter: `{}` → `{}` (delta `{}`)\n\
                 - HE-TB transmission counter: `{}` → `{}` (delta `{}`)\n\
                 - QoS Null counter: `{}` → `{}` (delta `{}`)\n",
                evidence.before.rx_trigger_count,
                evidence.after.rx_trigger_count,
                evidence
                    .after
                    .rx_trigger_count
                    .wrapping_sub(evidence.before.rx_trigger_count),
                evidence.before.transmission_count,
                evidence.after.transmission_count,
                evidence
                    .after
                    .transmission_count
                    .wrapping_sub(evidence.before.transmission_count),
                evidence.before.qos_null_count,
                evidence.after.qos_null_count,
                evidence
                    .after
                    .qos_null_count
                    .wrapping_sub(evidence.before.qos_null_count),
            ),
            format!(
                "- Latched TB user: AID `{}`, RU `{}`, MCS `{}`\n",
                evidence.user.association_id, evidence.user.ru_allocation, evidence.user.mcs
            ),
        ),
        None => ("FAIL", format!("- Failure: `{failure}`\n"), String::new()),
    };
    let markdown = format!(
        "# Open-radio HE Trigger/TB HIL\n\n\
         - Result: `{result}`\n\
         - Monitor interface: `{}`\n\
         - Trigger transmitter: `{:02x?}`\n\
         - Requested AID/RU/MCS: `{}/{}/{}`\n\
         - Coding: `{}`; DCM: `{}`\n\
         - Frames sent: `{}` at `{}` ms intervals\n\
         {counters}{user}\n\
         Complete UART evidence is in [`uart.log`](uart.log).\n",
        options.interface,
        options.transmitter.0,
        options.association_id,
        options.ru_allocation,
        options.mcs,
        if options.ldpc { "LDPC" } else { "BCC" },
        options.dcm,
        options.count,
        options.interval.as_millis(),
    );
    fs::write(directory.join("report.md"), markdown)?;
    Ok(())
}

fn parse_bounded<T>(value: &str, minimum: T, maximum: T, option: &str) -> Result<T>
where
    T: std::str::FromStr + PartialOrd + Copy,
    T::Err: std::error::Error + 'static,
{
    let parsed = value.parse::<T>()?;
    if parsed < minimum || parsed > maximum {
        return Err(format!("{option} is out of range").into());
    }
    Ok(parsed)
}

fn parse_mac(value: &str) -> Result<MacAddress> {
    let mut address = [0_u8; 6];
    let mut fields = value.split(':');
    for byte in &mut address {
        let field = fields.next().ok_or("MAC address requires six octets")?;
        if field.len() != 2 {
            return Err("MAC address octets must contain two hex digits".into());
        }
        *byte = u8::from_str_radix(field, 16)?;
    }
    if fields.next().is_some() {
        return Err("MAC address contains more than six octets".into());
    }
    Ok(MacAddress(address))
}

fn validate_interface_name(interface: &str) -> Result<()> {
    if interface.is_empty()
        || interface.len() >= libc::IFNAMSIZ
        || !interface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err("invalid network interface name".into());
    }
    Ok(())
}

fn encode_packet(options: &Options) -> Result<Vec<u8>> {
    let encoding = BasicTriggerFrameEncoding {
        duration: 0,
        receiver_address: options.receiver.0,
        transmitter_address: options.transmitter.0,
        common: TriggerCommonEncoding {
            uplink_length: options.uplink_length,
            more_trigger_frames: false,
            carrier_sense_required: false,
            uplink_bandwidth_encoding: 0,
            gi_ltf: options.gi_ltf,
            mu_mimo_ltf_mode: false,
            he_ltf_symbols_and_midamble_periodicity: 0,
            uplink_stbc: false,
            ldpc_extra_symbol_segment: false,
            ap_tx_power_encoding: options.ap_tx_power_encoding,
            pre_fec_padding_factor_encoding: 1,
            packet_extension_disambiguity: false,
            uplink_spatial_reuse: 0,
            doppler: false,
            // This HIL policy uses the all-ones reserved field image. It is
            // deliberately not inferred by the blob inverse encoder.
            uplink_he_sig_a2_reserved: 0x01ff,
            trailing_reserved: false,
        },
        user: TriggerScheduledUserEncoding {
            association_id: options.association_id,
            ru_allocation_region: false,
            ru_allocation: options.ru_allocation,
            coding_type: options.ldpc,
            mcs: options.mcs,
            dcm: options.dcm,
            starting_spatial_stream_encoding: 0,
            spatial_stream_count_encoding: 0,
            target_rssi_encoding: options.target_rssi_encoding,
            reserved: false,
        },
        dependent: TriggerBasicDependentInfo {
            mpdu_mu_spacing_factor: 0,
            tid_aggregation_limit: 0,
            reserved: false,
            preferred_access_category: 0,
        },
    };
    let mut packet = vec![0_u8; RADIOTAP_HEADER.len() + TRIGGER_BASIC_FRAME_LEN];
    packet[..RADIOTAP_HEADER.len()].copy_from_slice(&RADIOTAP_HEADER);
    let length = encoding
        .encode(&mut packet[RADIOTAP_HEADER.len()..])
        .map_err(|error| format!("cannot encode Basic Trigger: {error:?}"))?;
    debug_assert_eq!(length, TRIGGER_BASIC_FRAME_LEN);
    Ok(packet)
}

fn ensure_monitor_interface(interface: &str) -> Result<()> {
    let kind = fs::read_to_string(Path::new("/sys/class/net").join(interface).join("type"))
        .map_err(|error| format!("cannot inspect interface `{interface}`: {error}"))?;
    let kind = kind.trim().parse::<u16>()?;
    if kind != ARPHRD_IEEE80211_RADIOTAP {
        return Err(format!(
            "interface `{interface}` has ARPHRD type {kind}, expected monitor/radiotap type \
             {ARPHRD_IEEE80211_RADIOTAP}"
        )
        .into());
    }
    Ok(())
}

struct PacketSocket {
    descriptor: OwnedFd,
    address: libc::sockaddr_ll,
}

impl PacketSocket {
    fn bind(interface: &str) -> Result<Self> {
        let interface = CString::new(interface)?;
        // SAFETY: `interface` is a live NUL-terminated C string for the
        // duration of the call.
        let interface_index = unsafe { libc::if_nametoindex(interface.as_ptr()) };
        if interface_index == 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: `socket` has no pointer arguments. A nonnegative return
        // value transfers one owned descriptor to this scope.
        let raw_descriptor = unsafe {
            libc::socket(
                libc::AF_PACKET,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                i32::from(ETH_P_ALL.to_be()),
            )
        };
        if raw_descriptor < 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: the successful `socket` call returned a fresh descriptor
        // that has not been wrapped or closed elsewhere.
        let descriptor = unsafe { OwnedFd::from_raw_fd(raw_descriptor) };
        let address = libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as u16,
            sll_protocol: ETH_P_ALL.to_be(),
            sll_ifindex: interface_index as i32,
            sll_hatype: 0,
            sll_pkttype: 0,
            sll_halen: 0,
            sll_addr: [0; 8],
        };
        Ok(Self {
            descriptor,
            address,
        })
    }

    fn send(&self, packet: &[u8]) -> Result<()> {
        use std::os::fd::AsRawFd;

        // SAFETY: both pointers refer to live immutable objects for the call;
        // their byte lengths are exact, and the owned descriptor remains open.
        let sent = unsafe {
            libc::sendto(
                self.descriptor.as_raw_fd(),
                packet.as_ptr().cast(),
                packet.len(),
                0,
                (&raw const self.address).cast(),
                size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if sent < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if sent as usize != packet.len() {
            return Err(format!("short monitor injection: {sent}/{}", packet.len()).into());
        }
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_ieee80211::trigger::{
        TriggerType, parse_trigger_basic_dependent, parse_trigger_frame,
        parse_trigger_user_spatial_stream,
    };

    #[test]
    fn dry_run_packet_is_a_bounded_basic_trigger() {
        let options = parse_options(
            &[
                "mon0".into(),
                "--transmitter".into(),
                "70:15:fb:a8:48:f0".into(),
                "--aid".into(),
                "42".into(),
                "--mcs".into(),
                "9".into(),
                "--ldpc".into(),
            ],
            false,
        )
        .unwrap();
        let packet = encode_packet(&options).unwrap();
        assert_eq!(&packet[..8], &RADIOTAP_HEADER);
        let trigger = parse_trigger_frame(&packet[8..]).unwrap();
        assert_eq!(trigger.common.trigger_type, TriggerType::Basic);
        assert_eq!(trigger.common.uplink_length, 100);
        assert_eq!(trigger.common.uplink_bandwidth_encoding, 0);
        assert_eq!(trigger.transmitter_address, options.transmitter.0);
        let user = trigger.users().next().unwrap().unwrap();
        let user_info = parse_trigger_user_spatial_stream(user.user_info).unwrap();
        assert_eq!(user_info.aid12, 42);
        assert_eq!(user_info.ru_allocation, 61);
        assert_eq!(user_info.mcs, 9);
        assert!(user_info.coding_type);
        assert_eq!(
            parse_trigger_basic_dependent(user.dependent_info)
                .unwrap()
                .tid_aggregation_limit,
            0
        );
    }

    #[test]
    fn parser_rejects_missing_identity_and_unbounded_values() {
        assert!(parse_options(&["mon0".into()], false).is_err());
        assert!(
            parse_options(
                &[
                    "mon0".into(),
                    "--transmitter".into(),
                    "70:15:fb:a8:48:f0".into(),
                    "--aid".into(),
                    "4096".into(),
                ],
                false
            )
            .is_err()
        );
        assert!(
            parse_options(
                &[
                    "../wlan0".into(),
                    "--transmitter".into(),
                    "70:15:fb:a8:48:f0".into(),
                    "--aid".into(),
                    "1".into(),
                ],
                false
            )
            .is_err()
        );
    }

    #[test]
    fn hil_parser_owns_serial_and_sustained_injection_defaults() {
        let options = parse_options(
            &[
                "mon0".into(),
                "--transmitter".into(),
                "70:15:fb:a8:48:f0".into(),
                "--aid".into(),
                "1".into(),
                "--serial".into(),
                "/dev/ttyACM1".into(),
            ],
            true,
        )
        .unwrap();
        assert_eq!(options.count, 10_000);
        assert_eq!(options.interval, Duration::from_millis(1));
        assert_eq!(options.serial, "/dev/ttyACM1");
        assert!(
            parse_options(
                &[
                    "mon0".into(),
                    "--transmitter".into(),
                    "70:15:fb:a8:48:f0".into(),
                    "--aid".into(),
                    "1".into(),
                    "--serial".into(),
                    "/dev/ttyACM0".into(),
                ],
                false
            )
            .is_err()
        );
    }

    #[test]
    fn hil_qualification_requires_counter_growth_and_matching_user() {
        let options = parse_options(
            &[
                "mon0".into(),
                "--transmitter".into(),
                "70:15:fb:a8:48:f0".into(),
                "--aid".into(),
                "1".into(),
            ],
            true,
        )
        .unwrap();
        let log = "\
OPEN_RADIO_PHY_HIL stage=he-tb-statistics value=MacHeTbStatistics { rx_trigger_count: 4, transmission_count: 2, qos_null_count: 1 }\n\
OPEN_RADIO_PHY_HIL stage=he-tb-user aid=0 ru=0 mcs=0 preferred_ac=0 spacing=0 packet_extension=0\n\
OPEN_RADIO_PHY_HIL stage=he-trigger-frame count=0 schedule_aid=1 schedule=None\n\
OPEN_RADIO_PHY_HIL stage=he-tb-statistics value=MacHeTbStatistics { rx_trigger_count: 9, transmission_count: 5, qos_null_count: 2 }\n\
OPEN_RADIO_PHY_HIL stage=he-tb-user aid=1 ru=61 mcs=0 preferred_ac=0 spacing=0 packet_extension=0\n\
OPEN_RADIO_PHY_HIL stage=he-trigger-frame count=5 schedule_aid=1 schedule=Some(Ok(HeTriggerScheduledRate))\n";
        let evidence = qualify_hil(&options, log).unwrap();
        assert_eq!(evidence.before.rx_trigger_count, 4);
        assert_eq!(evidence.after.transmission_count, 5);
        assert_eq!(evidence.user.association_id, 1);

        let no_transmission = log.replace("transmission_count: 5", "transmission_count: 2");
        assert!(qualify_hil(&options, &no_transmission).is_err());
        let wrong_user = log.replace("aid=1 ru=61", "aid=2 ru=61");
        assert!(qualify_hil(&options, &wrong_user).is_err());
        let rejected_schedule = log.replace("schedule=Some(Ok(", "schedule=Some(Err(");
        assert!(qualify_hil(&options, &rejected_schedule).is_err());
        let stale_matching_user = log
            .replace("aid=1 ru=61", "aid=2 ru=61")
            .replace("aid=0 ru=0", "aid=1 ru=61");
        assert!(qualify_hil(&options, &stale_matching_user).is_err());
    }
}
