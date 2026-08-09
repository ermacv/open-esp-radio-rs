# ESP32-S31 Wi-Fi role round trip

Qualification ID: `HIL_ESP32S31_WIFI_ROLE_ROUNDTRIP_2026_08_09`

The production runner completed this owner sequence against an external AP:

```text
Station(1) -> Idle -> Scan(2) -> Idle -> Monitor(3) -> Idle -> Station(4)
```

The finite scan found the configured BSS on channel 1. The one-second monitor
epoch captured 20 frames / 5,120 bytes with zero generation or explicit
channel mismatches. Per-frame hardware channel metadata was zero for all 20
frames and is normalized as unavailable;
the fixed channel is therefore proven by the exclusive monitor configuration,
not invented from RX metadata. The final station completed scan,
authentication, association, WPA2 and connected entry. Each role transition
used protocol v18 typed commands and completion evidence; UART text was
diagnostic only.

Command: `cargo hil wifi roundtrip --external-ap --channel 1 --monitor-seconds 1`.
