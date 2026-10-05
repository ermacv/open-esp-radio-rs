//! Every chip's command line builds with its own pins and lists its
//! scenarios and the reviewer commands; an unknown chip is refused.

use std::process::Command;

fn run(arguments: &[&str]) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_vendor-scenarios"))
        .args(arguments)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr),
    )
}

#[test]
fn each_chip_lists_its_scenarios_and_the_reviewer_commands() {
    for (chip, scenario) in [("esp32s31", "gain"), ("esp32c5", "phy-i2c")] {
        let (success, help) = run(&[chip, "--help"]);
        assert!(success, "{chip}: {help}");
        assert!(help.contains(&format!("vendor-scenarios {chip}")), "{help}");
        assert!(help.contains(scenario), "{chip}: {help}");
        assert!(help.contains("xref"), "{chip}: {help}");
    }
    let (success, usage) = run(&["esp32x9"]);
    assert!(!success);
    assert!(usage.contains("<esp32s31|esp32c5>"), "{usage}");
}
