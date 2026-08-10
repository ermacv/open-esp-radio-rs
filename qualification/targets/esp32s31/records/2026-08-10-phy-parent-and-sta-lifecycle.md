# ESP32-S31 cold PHY and STA lifecycle

Evidence ID: `HIL_ESP32S31_PHY_PARENT_STA_LIFECYCLE_2026_08_10`

- release HIL image passed SRAM/PSRAM placement and source-only graph audits;
- full PHY calibration returned a 284-byte host-owned artifact as `Created`
  after 2,046,434 us, then completed one typed station epoch cycle;
- a supplied structurally valid artifact was deliberately reported as
  `Replaced`: the driver does not yet own complete cold hardware replay, so it
  performed full calibration again and replaced the caller-owned artifact;
- the replacement path associated and passed exact 20 Mbit/s UDP RX with no
  driver queue drops; the target-acknowledged UDP readiness edge kept the
  measured sequence contiguous from zero;
- a separate cold boot completed two consecutive typed station epoch cycles
  without retryable scan or reconnect failures.

This qualifies cold registration, RF/baseband initialization, channel use and
healthy Station -> Idle -> Station owner return. It does not qualify retained
calibration replay, peak traffic performance, injected timeout cleanup or a
terminal fault frontier.
