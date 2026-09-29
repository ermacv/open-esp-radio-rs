"""On-air check between two boards running the peer line protocol."""
import sys, time, serial

S31 = "/dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_30:ED:A0:F3:F6:D0-if00"
C5 = "/dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_38:44:BE:AA:25:64-if00"

def open_reset(path):
    port = serial.Serial(path, 115200, timeout=0.05)
    port.dtr = False
    port.rts = True
    time.sleep(0.1)
    port.reset_input_buffer()
    port.rts = False
    return port

class Board:
    def __init__(self, name, path):
        self.name, self.port, self.buf = name, open_reset(path), b""
    def lines(self, seconds):
        end, out = time.time() + seconds, []
        while time.time() < end:
            self.buf += self.port.read(256)
            while b"\n" in self.buf:
                line, self.buf = self.buf.split(b"\n", 1)
                text = line.decode(errors="replace").strip()
                if text.startswith("@"):
                    print(f"{self.name} < {text}", flush=True)
                    out.append(text)
        return out
    def wait(self, prefix, seconds=6):
        end = time.time() + seconds
        while time.time() < end:
            for line in self.lines(0.2):
                if line.startswith(prefix):
                    return line
        raise SystemExit(f"{self.name}: no {prefix}")
    def cmd(self, text, expect=None):
        print(f"{self.name} > {text}", flush=True)
        self.port.write((text + "\n").encode())
        return self.wait(expect or "@OK", 3)

def frame(seq, dst, src, ack):
    control = 0x8841 | (0x20 if ack else 0)
    return (control.to_bytes(2, "little") + bytes([seq]) + (0x4f45).to_bytes(2, "little")
            + dst.to_bytes(2, "little") + src.to_bytes(2, "little") + b"OER-154-" + bytes([seq])).hex()

s31, c5 = Board("S31", S31), Board("C5", C5)
s31.wait("@READY"); c5.wait("@READY")
s31.cmd("CFG 15 4f45 0001 015431533332654f 0 0")
c5.cmd("CFG 15 4f45 0002 025431433570654f 0 0")
results = {"s31_to_c5": 0, "c5_to_s31": 0}
c5.cmd("RX")
for seq in range(1, 4):
    s31.cmd(f"TX 0 {frame(seq, 0x0002, 0x0001, True)}")
    got = s31.lines(0.5) + c5.lines(0.5)
    results["s31_to_c5"] += sum(1 for line in got if line.startswith("@RX"))
s31.cmd("RX")
for seq in range(0x81, 0x84):
    c5.cmd(f"TX 0 {frame(seq, 0x0001, 0x0002, True)}")
    got = c5.lines(0.5) + s31.lines(0.5)
    results["c5_to_s31"] += sum(1 for line in got if line.startswith("@RX"))
print("RESULT", results, flush=True)
