use super::*;

const EXAMPLE: &str = include_str!("../../stand.example.toml");

fn chips() -> Vec<Profile> {
    Profile::all(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap()
}

fn example() -> StandFile {
    StandFile::parse(EXAMPLE).unwrap()
}

/// The example with `from` replaced by `to`, validated.
fn edited(from: &str, to: &str) -> Result<StandFile> {
    assert!(EXAMPLE.contains(from), "{from}");
    let file = StandFile::parse(&EXAMPLE.replacen(from, to, 1))?;
    file.validate()?;
    file.validate_chips(&chips()).map(|()| file)
}

fn rejected(from: &str, to: &str, reason: &str) {
    let message = edited(from, to).err().map(|error| error.to_string());
    assert!(
        message
            .as_deref()
            .is_some_and(|message| message.contains(reason)),
        "{from} → {to}: {message:?}"
    );
}

#[test]
fn the_example_is_a_valid_stand_file() {
    let file = example();
    file.validate().unwrap();
    file.validate_chips(&chips()).unwrap();
    assert_eq!(file.stand.air, AirPolicy::Exclusive);
    assert_eq!(file.hub("rsh-mid").unwrap().button(3), Some(6));
    let board = file.board_by_serial("30:ed:a0:00:00:01").unwrap();
    assert_eq!(board.id, "s31-a");
    assert_eq!(
        board.reset,
        [ResetStep::UsbJtagRts, ResetStep::Jtag, ResetStep::Power]
    );
    for section in FIXTURE_SECTIONS
        .into_iter()
        .filter(|section| *section != "air_observer")
    {
        assert!(file.fixture(section).is_some(), "{section}");
    }
}

#[test]
fn identities_are_unique() {
    rejected(
        "id = \"c5-a\"",
        "id = \"s31-a\"",
        "board `s31-a` is described twice",
    );
    rejected(
        "usb-serial = \"38:44:BE:00:00:01\"",
        "usb-serial = \"30:ed:a0:00:00:01\"",
        "another board's",
    );
    rejected(
        "id = \"rsh-bottom\"",
        "id = \"rsh-mid\"",
        "hub `rsh-mid` is described twice",
    );
    rejected("usb2 = \"3-8.3.4\"", "usb2 = \"3-8.3\"", "another hub's");
    rejected("4 = 10 }", "4 = 9 }", "button 9 is zero or another port's");
    rejected(
        "id = \"s31-a\"",
        "id = \"S31\"",
        "must start with a lowercase letter",
    );
}

#[test]
fn a_board_hangs_on_a_switchable_board_port_of_a_described_hub() {
    rejected(
        "port = { hub = \"rsh-mid\", port = 3 }",
        "port = { hub = \"rsh-mid\", port = 4 }",
        "is not a board port",
    );
    rejected(
        "port = { hub = \"rsh-mid\", port = 3 }",
        "port = { hub = \"rsh-top\", port = 2 }",
        "resets by power, but port 2 of hub `rsh-top` is not switchable",
    );
    rejected(
        "port = { hub = \"rsh-mid\", port = 3 }",
        "port = { hub = \"rsh-mid\", port = 2 }",
        "holds another board",
    );
    rejected(
        "port = { hub = \"rsh-mid\", port = 3 }",
        "port = { hub = \"rsh-side\", port = 3 }",
        "hub `rsh-side` is not described",
    );
    rejected(
        "protected = [4]",
        "protected = [3]",
        "port 3 is both protected and switchable",
    );
    // Without the power step, an unswitchable port is enough.
    edited(
        "reset = [\"usb-jtag-rts\", \"jtag\", \"power\"]\n",
        "reset = [\"usb-jtag-rts\", \"jtag\"]\n",
    )
    .unwrap();
}

#[test]
fn a_board_carries_only_its_chips_radios() {
    rejected(
        "radios = [\"wifi-2g4\", \"ble\", \"ieee802154\"]",
        "radios = [\"wifi-2g4\", \"wifi-5g\"]",
        "an esp32s31 has no wifi-5g radio",
    );
    rejected(
        "chip = \"esp32s31\"",
        "chip = \"esp32c6\"",
        "chip `esp32c6` has no profile",
    );
    rejected(
        "roles = [\"dut\", \"peer\"]",
        "roles = [\"dut\", \"dut\"]",
        "roles repeats a value",
    );
    rejected(
        "roles = [\"dut\", \"peer\"]",
        "roles = []",
        "roles is empty",
    );
}

#[test]
fn the_file_has_only_its_sections_and_its_schema() {
    rejected(
        "schema = 1",
        "schema = 2",
        "stand file schema 2; this build reads schema 1",
    );
    rejected("[legacy_bss]", "[duts.esp32s31]", "unknown section [duts]");
    assert!(
        StandFile::parse(&EXAMPLE.replacen("air = \"exclusive\"", "air = \"ranges\"", 1)).is_err(),
        "ranges is a later decision"
    );
}

#[test]
fn a_run_names_its_board_only_when_the_pool_has_several() {
    let file = example();
    assert_eq!(
        file.select_board("esp32s31", BoardRole::Dut, None, None)
            .unwrap()
            .id,
        "s31-a"
    );
    let second = EXAMPLE.replacen(
        "[[board]]\nid = \"c5-a\"",
        "[[board]]\nid = \"s31-b\"\nusb-serial = \"30:ED:A0:00:00:02\"\nchip = \"esp32s31\"\n\
         radios = [\"wifi-2g4\"]\nroles = [\"dut\"]\nport = { hub = \"rsh-bottom\", port = 1 }\n\
         reset = [\"power\"]\n\n[[board]]\nid = \"c5-a\"",
        1,
    );
    let file = StandFile::parse(&second).unwrap();
    file.validate().unwrap();
    let several = file
        .select_board("esp32s31", BoardRole::Dut, None, None)
        .unwrap_err();
    assert!(several.to_string().contains("s31-a, s31-b"), "{several}");
    assert_eq!(
        file.select_board("esp32s31", BoardRole::Dut, Some("s31-b"), None)
            .unwrap()
            .id,
        "s31-b"
    );
    // The peer is another board than the device under test.
    assert_eq!(
        file.select_board("esp32s31", BoardRole::Peer, None, Some("s31-b"))
            .unwrap()
            .id,
        "s31-a"
    );
    assert!(
        file.select_board("esp32s31", BoardRole::Peer, None, Some("s31-a"))
            .unwrap_err()
            .to_string()
            .contains("no esp32s31 board for peer besides `s31-a`")
    );
    assert!(
        file.select_board("esp32c5", BoardRole::Dut, Some("s31-b"), None)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn the_stand_file_is_private_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stand.toml");
    fs::write(&path, EXAMPLE).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(
        StandFile::load(&path)
            .unwrap_err()
            .to_string()
            .contains("chmod 600")
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    StandFile::load(&path).unwrap();
}

#[test]
fn one_resolver_names_a_board_by_id_by_its_only_chip_or_by_mac() {
    let file = example();
    let s31 = file.resolve("s31-a").unwrap().reference().unwrap();
    assert_eq!(
        (s31.id.as_str(), s31.mac.as_str(), s31.chip.as_str()),
        ("s31-a", "30:ED:A0:00:00:01", "esp32s31")
    );
    assert_eq!(s31.label(), "s31-a (esp32s31)");
    // A chip names its only board.
    assert_eq!(file.resolve("esp32c5").unwrap().id, "c5-a");
    // A MAC in any form names its board.
    assert_eq!(file.resolve("38:44:be:00:00:01").unwrap().id, "c5-a");
    assert_eq!(file.resolve("3844BE000001").unwrap().id, "c5-a");
    // A MAC outside the stand file, or a word that is none of them, is no board.
    let unknown = file.resolve("AA:BB:CC:DD:EE:FF").unwrap_err().to_string();
    assert!(unknown.contains("no board of the stand file"), "{unknown}");
    assert!(file.resolve("s3").is_err());
    assert_eq!(file.label("38:44:BE:00:00:01"), "c5-a (esp32c5)");
    assert_eq!(file.label("AA:BB:CC:DD:EE:FF"), "AA:BB:CC:DD:EE:FF");
}

#[test]
fn several_boards_of_a_chip_need_a_name() {
    let mut two = example();
    let mut second = two.board[0].clone();
    second.id = "s31-b".into();
    second.usb_serial = "30:ED:A0:00:00:02".into();
    two.board.push(second);
    let error = two.resolve("esp32s31").unwrap_err().to_string();
    assert!(error.contains("s31-a, s31-b"), "{error}");
}

#[test]
fn a_board_that_resets_by_power_has_its_hub_port() {
    let file = example();
    let c5 = file.board("c5-a").unwrap();
    let port = file.hub_port(c5).unwrap();
    let hub_port = c5.port.as_ref().unwrap();
    assert_eq!(port.location, file.hub(&hub_port.hub).unwrap().usb2);
    assert_eq!(port.port, hub_port.port);
    let mut no_power = c5.clone();
    no_power.reset = vec![ResetStep::Jtag];
    assert_eq!(file.hub_port(&no_power), None);
}

#[test]
fn a_usb_serial_must_be_a_mac() {
    rejected(
        "usb-serial = \"38:44:BE:00:00:01\"",
        "usb-serial = \"not-a-mac\"",
        "is not a six-byte MAC address",
    );
}
