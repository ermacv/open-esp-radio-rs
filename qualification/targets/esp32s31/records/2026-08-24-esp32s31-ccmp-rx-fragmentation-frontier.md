# ESP32-S31 CCMP RX fragmentation source frontier

Date: 2026-08-24

This is a source-frontier record, not a hardware qualification claim. It does
not change a qualification ledger and contains no vendor binary, private
oracle output, extracted table or disassembly. The hardware premise remains
the repository's existing ESP32-S31 CCMP receive contract: plaintext is
exposed only after the reviewed RX status reports successful decrypt and MIC
verification.

## Maintained production boundary

- The MAC has distinct ordinary and fragmented CCMP views backed by the same
  descriptor, trailer, ExtIV/Key-ID and hardware-crypto validator. Neither
  view can be substituted for the other by changing only Sequence Control or
  More Fragments.
- STA and AP accept only individually addressed, three-address Data or QoS
  Data fragments. Ordered/HT-Control, A-MSDU, group-destination, foreign-role,
  empty and oversized inputs fail closed.
- Two fixed contexts retain at most one 1,500-byte Ethernet payload plus its
  LLC/SNAP header. Runtime timeout, deterministic reuse, STA epoch teardown,
  AP peer close, association/AID reuse and PTK-generation change revoke the
  retained plaintext and Retry fingerprints.
- Every new fragment has an independent CCMP PN. Replay is prepared only
  after hardware authentication, identity parsing, live peer/controlled-port
  authorization and a side-effect-free reassembly preflight. The fragment is
  then durably admitted to its bounded context before that PN is committed.
  A final PN is committed inside the completion edge before the sole Ethernet
  publication.
- A retransmission is suppressed only when Retry, protection/Key ID, key or
  association epoch, transmitter, full address identity, TID, sequence,
  fragment number, PN, More Fragments shape and authenticated plaintext bytes
  all match the retained fragment. A changed PN or byte is an explicit
  rejection and never completes an MSDU.
- AP replay admission is two phase and revalidates the controlled port,
  association epoch and exact installed PTK generation at authorize, prepare
  and commit. STA shared replay candidates retain their generation fence; a
  prepared group candidate also retains the group-rotation publication gate.
- A replay-invalid new fragment cannot evict an existing train: eviction and
  byte mutation occur only after replay preparation succeeds. An ingest or
  commit failure discards the exact affected train.

## Deliberate limits

This slice covers WPA2-Personal CCMP RX only. It does not implement or claim
TX fragmentation, group-address fragmentation, GCMP/TKIP/WAPI/WEP, PMF/BIP,
WPA3, Enterprise authentication, four-address data, protected A-MSDU
fragmentation, or an on-air interoperability/HIL result. Open-network RX
reassembly remains supported by the same bounded owner. TX fragmentation and
hardware qualification remain separate future work.

The source regressions exercise STA and AP production dispatch, exact Retry
fingerprints, per-fragment PN advancement, final commit-before-publication,
replay rejection without context eviction, association/key fencing and
ordinary-MPDU collision fences. Synthetic descriptor state validates source
ownership and ordering only; it is not a substitute for dated on-air evidence.
