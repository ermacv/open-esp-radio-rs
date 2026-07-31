use std::{env, path::PathBuf};

const BIN: &str = "open-esp-radio-vendor-oracle-hil-esp32s31";

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo provides CARGO_MANIFEST_DIR"),
    );
    let linker_dir = manifest_dir
        .ancestors()
        .nth(3)
        .expect("oracle runtime must live below hil/vendor-oracle/esp32s31")
        .join("esp32s31/linker");

    for file in [
        "../../../esp32s31/linker/rom/esp32s31-eco0.x",
        "../../../esp32s31/linker/bootstrap/link.x",
        "../../../esp32s31/linker/bootstrap/memory.x",
        "../../../esp32s31/linker/bootstrap/sections.x",
        "../../../esp32s31/linker/bootstrap/flash-sections.x",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }
    println!("cargo:rustc-link-search={}", linker_dir.display());
    for argument in ["-Trom/esp32s31-eco0.x", "-Tbootstrap/link.x", "--nmagic"] {
        println!("cargo:rustc-link-arg-bin={BIN}={argument}");
    }

    for symbol in [
        "phy_txdc_cal_init",
        "phy_rf_init",
        "phy_set_chan_freq_hw_init",
        "phy_xtal_duty_cal_init",
        "phy_fe_reg_update",
        "phy_i2cmst_reg_init",
        "phy_pwdet_reg_init",
        "phy_fe_reg_init",
        "phy_tx_pwctrl_bg_init",
        "phy_rc_cal_init",
        "phy_filter_dcap_set",
        "phy_i2c_init1",
        "phy_rfpll_chgp_cal",
        "phy_i2c_master_cmd_mem_init",
        "phy_bias_reg_set",
        "phy_open_i2c_xpd_new",
        "phy_tsens_read_init",
    ] {
        println!("cargo:rustc-link-arg-bin={BIN}=--wrap={symbol}");
    }

    println!("cargo:rerun-if-env-changed=ESP32S31_LIBGCC_DIR");
    if let Some(directory) = env::var_os("ESP32S31_LIBGCC_DIR") {
        println!(
            "cargo:rustc-link-search=native={}",
            PathBuf::from(directory).display()
        );
        println!("cargo:rustc-link-arg-bin={BIN}=-lgcc");
    }
}
