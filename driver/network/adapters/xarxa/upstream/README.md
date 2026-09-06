# Original Xarxa driver

This crate implements the original `xarxa-driver::Driver` at the revision
selected by the station example's upstream Embassy dependency. It depends on
neither Embassy networking nor chip-specific radio execution.

`Resources::split` creates a unique network device and a radio endpoint for
one logical interface. The radio controls link epochs and narrows its endpoint
to RX publication or TX consumption. Queues hold upstream `PacketBuf` owners;
they do not contain additional frame-sized arrays. The upstream global packet
pool owns all packet storage. Multiple logical interfaces share that pool.

TX rejection returns the same packet. Accepted packets keep their association
epoch, including when link state changes between `can_transmit` and `transmit`.
The ESP32-S31 bridge releases the software packet only after construction of
the final physical frame. The radio's physical owner then survives retries and
completion independently.

The pinned original Embassy UDP API wakes its runner after a pending send;
Xarxa wakes the sending socket again when polling its TX-starved state. With
no free packet or TX queue slot, those tasks can repeatedly wake each other
before the radio makes progress. The driver does not control that socket/stack
scheduling policy. Its release notifications provide actual capacity changes,
but do not eliminate these upstream retries.

RX publication checks link state, frame length, queue capacity and allocation.
`poll_ready` waits for a queue slot, not global pool capacity. Upstream provides
no pool-release notification, so allocation failure is returned as an explicit
`PoolExhausted` drop, counted by `Endpoint::rx_pool_drops`. The radio bridge
declares terminal pool exhaustion, so AP/paired-role batch cursors discard
that record instead of waiting or repeatedly polling for it.
Queue consumption wakes the radio; packet publication,
TX queue-credit return and link changes wake the network runner. Dropping a
selected TX owner also wakes it after releasing the packet's global pool slot:
the queue-credit wake can run on the other core before that slot is available.
An RX drain
consumes at most the queue depth before returning `None` and waking a pending
continuation, preventing an indefinitely refilled queue from starving sockets.

The driver advertises software checksum processing. It exposes no checksum
disable switches, physical addresses, BA sessions or packet-pool replacements.
The product's `upstream-network` feature composes these endpoints with the
same physical scheduler as the other integrations. Applications own original
Embassy stack storage, interfaces and sockets; see the
[station example](../../../../../examples/esp32s31-station/README.md) and
[dependency contract](../../../../../docs/wifi-egress.md).
