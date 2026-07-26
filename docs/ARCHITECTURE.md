# Architecture

Dependency direction is fixed:

```text
application
  -> open-esp-radio
       -> open-esp-radio-phy-esp32s31
            -> open-esp-radio-hal-esp32s31
                 -> open-esp-radio-pac-esp32s31
```

Future MAC, IEEE 802.11 and WPA crates sit above PHY/HAL. Cryptography,
timers, channels and executor integration are injected at those higher
boundaries. PAC and HAL must not depend on Embassy or another executor.

An Embassy adapter is planned as a separate crate. It may provide static
tasks, interrupt futures, timers and channels, but the core remains usable
with a custom executor.

## Ownership

One Rust value owns the live radio state. Cold-init transitions move their
state into child transitions and recover it at terminal states. Hardware
operations are represented by non-cloneable identity bindings. There is no
implicit C callback table or C-owned parameter block in the source-only
profile.

## Waiting

Finite local arithmetic runs immediately. Every real delay or readiness edge
is awaited through a Rust async port. Future polling by an executor is normal;
busy loops over hardware registers and unconditional self-wakes are not.
Where no interrupt source is evidenced, a bounded one-shot register sample
may be scheduled by an async timer.

## Transitional debt

`open-esp-radio-phy-esp32s31` temporarily contains `radio_hal.rs`. These
finite MMIO leaves have no ROM/blob dependency, but their final home is the
HAL/PAC layers. Moving them is structural work and must not change the
calibration state-machine behaviour.
