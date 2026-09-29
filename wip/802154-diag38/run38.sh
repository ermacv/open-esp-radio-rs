#!/bin/zsh
# #38 runbook: run after the stand is released. Logs in $T.
T=/home/ermacv/.claude/jobs/6dcee4f0/tmp
PY=/home/ermacv/.cache/open-esp-radio/esp-idf/idf-tools/python_env/idf6.2_py3.14_env/bin/python
W=/home/ermacv/dev/oer-38-verify
AC=hil/targets/esp32s31/runtime/src/product_hil/ieee802154/air_check.rs
RADIO=hil/targets/esp32s31/runtime/src/product_hil/ieee802154/session/radio.rs
cd $W || exit 1
[ -z "$(git status --short)" ] || { echo "dirty worktree"; exit 1; }
echo "## 1 vendor xtal duty"; ( cd /home/ermacv/dev/open-esp-radio-rs-wifi && cargo hil --owner 802154 lease --board esp32s31 --air shared -- sh -c "cargo hil firmware flash ieee802154-vendor-reference --board esp32s31 && $PY $T/vendor_xtal.py" > $T/r38_vendor_xtal.log 2>&1 ); grep "VENDOR" $T/r38_vendor_xtal.log
hil_run() { name=$1; scen=$2; env -u OER_DIAG38_AFTER -u OER_DIAG38_ARM -u OER_DIAG38_VENDOR systemd-run --user --scope -q -p MemoryMax=12G cargo hil --owner 802154 run $scen > $T/r38_$name.log 2>&1; echo "== $name RC=$? $(date +%T)"; grep -o '"run_id":"[^"]*"' $T/r38_$name.log; grep -E "^error" -A12 $T/r38_$name.log | head -30; R=$(grep -o '"run_directory":"[^"]*"' $T/r38_$name.log | cut -d'"' -f4); [ -n "$R" ] && { grep -rhao "rssi=-[0-9]*" $R | sort | uniq -c; for f in $R/scenarios/*/repetition-001/boot-*/uart.bin; do grep -a "OER38\|diag38" "$f" | tr -cd '\11\12\40-\176'; done; }; }
ac12() { sed -i 's/^\(        \)transmit_power_dbm: None,/\1transmit_power_dbm: Some(12),/; s/^\(            \)transmit_power_dbm: None,/\1transmit_power_dbm: Some(12),/' $AC; }
echo "## 2 air-check ch15 timeline + xtal write"; git checkout -q -- hil; python3 $T/patch_sdm.py $AC && python3 $T/patch_freq.py $AC && python3 $T/patch_xtal_aircheck.py $AC && ac12 && hil_run ac15_xtal ieee802154-air-check
echo "## 3 air-check ch11 timeline"; git checkout -q -- hil; python3 $T/patch_sdm.py $AC && python3 $T/patch_freq.py $AC && ac12 && sed -i "s/^channel = 15$/channel = 11/" hil/scenarios/ieee802154/ieee802154-air-check.toml && hil_run ac11 ieee802154-air-check
git checkout -q -- hil
vend() { ( cd /home/ermacv/dev/open-esp-radio-rs-wifi && cargo hil --owner 802154 lease --board esp32s31 --board esp32c5 --air shared -- sh -c "cargo hil firmware flash ieee802154-peer --board esp32c5 --if-changed && cargo hil devices reset esp32c5 && cargo hil firmware flash ieee802154-vendor-reference --board esp32s31 && $PY $T/vendor_rssi2.py 12" > $T/r38_vendor_$1.log 2>&1 ); echo "== vendor $1 $(date +%T)"; grep -o "rssi=-[0-9]*" $T/r38_vendor_$1.log | sort | uniq -c; }
echo "## 4 bracket"; vend 1
grep -q "Some(12)" $RADIO || sed -i 's/^            transmit_power_dbm: None,/            transmit_power_dbm: Some(12),/' $RADIO
python3 $T/patch_freq_session.py && hil_run pe_default ieee802154-peer-exchange
echo "## 5 vendor seed right after our image"; ( cd /home/ermacv/dev/open-esp-radio-rs-wifi && cargo hil --owner 802154 lease --board esp32s31 --air shared -- sh -c "cargo hil firmware flash ieee802154-vendor-reference --board esp32s31 && $PY $T/vendor_seed.py" > $T/r38_vendor_seed.log 2>&1 ); grep "VENDOR\|SYMS" $T/r38_vendor_seed.log
git checkout -q -- hil; grep -q "Some(12)" $RADIO || sed -i 's/^            transmit_power_dbm: None,/            transmit_power_dbm: Some(12),/' $RADIO
python3 $T/patch_freq_session.py && python3 $T/patch_xtal_session.py && hil_run pe_xtal ieee802154-peer-exchange
git checkout -q -- hil
vend 2
git status --short

echo "## 6 EN vs RTS reset proxy (ROM download mode, no app)"
OCDB=/home/ermacv/.cache/open-esp-radio/esp-idf/idf-tools/tools/openocd-esp32/v0.12.0-esp32-20260831/openocd-esp32
for via in rts en; do ( cd /home/ermacv/dev/open-esp-radio-rs-wifi && cargo hil --owner 802154 lease --board esp32s31 --air none -- sh -c "cargo hil board reset esp32s31 --via $via --download && $OCDB/bin/openocd -s $OCDB/share/openocd/scripts -f interface/esp_usb_jtag.cfg -c 'adapter serial 30:ED:A0:F3:F6:D0' -f target/esp32s31.cfg -f $T/dump_rom61.tcl" > $T/r38_rom61_$via.log 2>&1 ); echo "== via $via"; grep "ROM61\|rst:" $T/r38_rom61_$via.log; done
cd /home/ermacv/dev/open-esp-radio-rs-wifi && cargo hil --owner 802154 devices reset esp32s31 > /dev/null 2>&1
