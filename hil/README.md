# Hardware-in-the-loop infrastructure

HIL executes the production driver on real hardware and records typed
qualification evidence. It must not become an alternative implementation of
the radio driver.

```text
hil/
├── protocol/          host/target command and telemetry wire protocol
├── host/
│   ├── runner/        build, flash and scenario orchestration
│   └── linux-net/     privileged Linux AP/monitor fixture
└── targets/
    └── esp32s31/      current embedded target workspace
```

Target firmware lives under `hil/targets/<chip>`. Dated results live under
`qualification/targets/esp32s31/records`; their contents retain the paths and
commands of the revision they measured.

The isolated vendor-linked oracle firmware is a verification input under
`verification/vendor/targets/esp32s31/oracle-firmware`; HIL only provides its
explicit build/flash orchestration.

Run the host interface through the workspace alias:

```console
cargo hil doctor
```

The Linux helper is installed separately because its narrowly scoped AP,
monitor and USB-reset operations require root privileges:

```console
cargo build -p open-esp-radio-hil-runner
sudo hil/host/linux-net/install.sh
```
