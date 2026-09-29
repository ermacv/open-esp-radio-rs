import subprocess, sys, time, serial
exec(open("/home/ermacv/.claude/jobs/6dcee4f0/tmp/onair_ab.py").read().split("s31, c5 = Board")[0])
OCD = "/home/ermacv/.cache/open-esp-radio/esp-idf/idf-tools/tools/openocd-esp32/v0.12.0-esp32-20260831/openocd-esp32"
NM = "/home/ermacv/.cache/open-esp-radio/esp-idf/idf-tools/tools/riscv32-esp-elf/esp-16.1.0_20260609/riscv32-esp-elf/bin/riscv32-esp-elf-nm"
ELF = "/home/ermacv/dev/open-esp-radio-rs-wifi/target/vendor-firmware/esp32s31/ieee802154-reference/build/oer_ieee802154_vendor_reference.elf"
syms = {}
for l in subprocess.run([NM, ELF], capture_output=True, text=True).stdout.splitlines():
    f = l.split()
    if len(f) == 3 and f[2] in ("phy_xtal_duty_cal", "register_chipv7_phy"): syms[f[2]] = f[0]
print("SYMS", syms, flush=True)
bp = syms.get("phy_xtal_duty_cal") or syms.get("register_chipv7_phy")
out = subprocess.run([OCD + "/bin/openocd", "-s", OCD + "/share/openocd/scripts", "-f", "interface/esp_usb_jtag.cfg",
                      "-c", "adapter serial 30:ED:A0:F3:F6:D0", "-f", "target/esp32s31.cfg",
                      "-c", f"set BP 0x{bp}", "-f", "/home/ermacv/.claude/jobs/6dcee4f0/tmp/dump_seed.tcl"],
                     capture_output=True, text=True, timeout=180)
for l in (out.stdout + out.stderr).splitlines():
    if l.startswith("SEED") or "rror" in l: print("VENDOR", l, flush=True)
class PlainBoard(Board):
    def __init__(self, name, path):
        self.name, self.buf = name, b""
        self.port = serial.Serial(); self.port.port, self.port.baudrate, self.port.timeout = path, 115200, 0.05
        self.port.dtr = False; self.port.rts = False; self.port.open()
time.sleep(2)
s31 = PlainBoard("S31", S31)
s31.port.write(b"\n")
for attempt in range(5):
    try: s31.cmd("SYNC"); break
    except SystemExit: time.sleep(1)
s31.cmd("CFG 15 4f45 0001 015431533332654f 0 12")
s31.cmd("TX 0 " + frame(1, 0x0002, 0x0001, False)); s31.lines(0.3)
out = subprocess.run([OCD + "/bin/openocd", "-s", OCD + "/share/openocd/scripts", "-f", "interface/esp_usb_jtag.cfg",
                      "-c", "adapter serial 30:ED:A0:F3:F6:D0", "-f", "target/esp32s31.cfg",
                      "-f", "/home/ermacv/.claude/jobs/6dcee4f0/tmp/dump_xtal.tcl"], capture_output=True, text=True, timeout=120)
print("VENDOR after-TX", [l for l in (out.stdout + out.stderr).splitlines() if l.startswith("I2C")], flush=True)
