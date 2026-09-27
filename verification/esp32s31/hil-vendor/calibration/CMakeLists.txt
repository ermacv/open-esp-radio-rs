# Vendor calibration firmware for the hardware calibration cross-check.
#
# Built by `cargo xtask vendor-firmware esp32s31` against the pinned ESP-IDF
# tree whose PHY, coexistence and Wi-Fi library submodules are replaced by
# the pinned archives of verification/esp32s31/artifacts.toml.
cmake_minimum_required(VERSION 3.22)
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
idf_build_set_property(MINIMAL_BUILD ON)
project(oer_vendor_calibration)
