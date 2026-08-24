# ESP32-S31 hardware qualification records

Files in this directory are immutable records of a specific hardware cell.
They preserve the tested code paths, commands, memory profile, peer, result
and artifact hashes as they existed on the recorded date. Old API paths in a
record are therefore evidence, not current usage instructions.

Current records:

- [ESP32-S31 FTM requester source frontier](2026-08-24-esp32s31-ftm-requester-frontier.md);
- [ESP32-S31 hardware beacon-monitor source frontier](2026-08-24-esp32s31-hardware-beacon-monitor-frontier.md);
- [ESP32-S31 HT Duplicate MCS32 source frontier](2026-08-24-esp32s31-ht-duplicate-mcs32-frontier.md);
- [ESP32-S31 AP TX BA16 datapath cutover](2026-08-23-esp32s31-ap-tx-ba16-cutover.md);
- [ESP32-S31 AP TX ceiling analysis](2026-08-23-esp32s31-ap-tx-ceiling-analysis.md);
- [ESP32-S31 AP PSRAM-stack load characterization](2026-08-17-esp32s31-ap-psram-stack-load.md);
- [ESP32-S31 HE RX A-MPDU containment](2026-08-04-esp32s31-he-rx-ampdu-containment.md);
- [ESP32-S31 direct HT RX aggregation metadata](2026-08-04-esp32s31-ht-rx-aggregation-metadata.md);
- [ESP32-S31 RX S-MPDU metadata](2026-08-04-esp32s31-rx-s-mpdu-metadata.md);
- [ESP32-S31 recoverable connected RX fault frontier](2026-08-06-esp32s31-station-rx-fault.md);
- [ESP32-S31 UDP, TCP and ICMP network regression](2026-08-06-esp32s31-network-regression.md);
- [ESP32-S31 stack and HE20 regression](2026-08-10-stack-and-he20-regression.md);
- [ESP32-S31 RX delivery frontier](2026-08-10-esp32s31-rx-delivery-frontier.md);
- [ESP32-S31 connected TX reset frontier](2026-08-04-esp32s31-station-tx-fault.md);
- [ESP32-S31 prolonged AP absence and retry exhaustion](2026-08-04-esp32s31-station-ap-absence.md);
- [ESP32-S31 controlled AP-loss recovery](2026-08-04-esp32s31-station-ap-loss.md);
- [driver-repository HE20 bidirectional](2026-07-31-driver-repo-he20-bidirectional.md);
- [connected HE20 BCC DCM](2026-07-31-he20-dcm-connected.md);
- [connected HE20 LDPC DCM](2026-07-31-he20-dcm-ldpc-connected.md);
- [vendor-oracle transfer](2026-07-31-vendor-oracle-transfer.md).

The canonical result graph and open gaps live in the
[machine-checked qualification ledger](../wifi-sta.ledger). Add a new dated
record when a materially different cell is qualified. Do not revise an old
record to claim a result from a newer implementation. Git history is the
archive for superseded narratives.
