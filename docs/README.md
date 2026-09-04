# Documentation

- [`../driver/README.md`](../driver/README.md): canonical driver boundaries,
  ownership and lifecycle.
- [`SOURCE_POLICY.md`](SOURCE_POLICY.md): permitted source and vendor evidence.
- [`VERIFICATION_AND_QUALIFICATION.md`](VERIFICATION_AND_QUALIFICATION.md):
  evidence classes, the production-trace gate and ledger workflow.
- [`WIFI_FAIRNESS_REQUIREMENTS.md`](WIFI_FAIRNESS_REQUIREMENTS.md): normative,
  intentionally evolvable behavior and resource requirements for multi-client
  AP, same-channel STA+AP and HE20 station operation.
- [`WIFI_EGRESS_ARCHITECTURE.md`](WIFI_EGRESS_ARCHITECTURE.md): target packet
  ownership, driver boundary, queue/fairness topology and design rationale.
- [`WIFI_EGRESS_STATUS.md`](WIFI_EGRESS_STATUS.md): audited current state of
  open-radio, the two Xarxa lineages, Embassy and the retained HIL evidence.
- [`WIFI_EGRESS_CUTOVER_PLAN.md`](WIFI_EGRESS_CUTOVER_PLAN.md): ordered
  ownership-first migration, deletion ledger and acceptance gates.
- [`../tools/blobray/README.md`](../tools/blobray/README.md):
  Blobray workflow and canonical documentation.
- [`BLUETOOTH_CODE_ARCHITECTURE_AUDIT.md`](BLUETOOTH_CODE_ARCHITECTURE_AUDIT.md):
  Bluetooth layer boundaries, monolith findings and refactor order.
- [`../qualification/`](../qualification/README.md): machine-readable claims
  and dated hardware evidence.
- [`../verification/`](../verification/README.md): structured vendor evidence,
  reviewed function packs and register models.

Historical migration narratives are intentionally not retained in the current
tree. Git history and HIL artifacts are the archive; current contracts live in
code, tests and the four purpose-specific Wi-Fi documents above.
