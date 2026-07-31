# Hardware qualification records

Files in this directory are immutable records of a specific hardware cell.
They preserve the tested code paths, commands, memory profile, peer, result
and artifact hashes as they existed on the recorded date. Old API paths in a
record are therefore evidence, not current usage instructions.

Current records:

- [driver-repository HE20 bidirectional](2026-07-31-driver-repo-he20-bidirectional.md);
- [connected HE20 BCC DCM](2026-07-31-he20-dcm-connected.md);
- [connected HE20 LDPC DCM](2026-07-31-he20-dcm-ldpc-connected.md);
- [vendor-oracle transfer](2026-07-31-vendor-oracle-transfer.md).

The canonical result matrix and repeat conditions live in the
[Wi-Fi feature ledger](../ESP32S31_WIFI_FEATURE_STATUS.md). Add a new dated
record when a materially different cell is qualified. Do not revise an old
record to claim a result from a newer implementation.

Superseded frontier narratives belong in [`../archive/hil/`](../archive/hil/),
not beside current qualification records.
