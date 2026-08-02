# Concrete coverage for leaves whose logical queue argument selects the
# reversed physical CONTROL[3-queue] PAC bank.

profile libpp-txq-enabled
vendor-source libpp
vendor-symbol hal_mac_is_txq_enabled
rust-symbol open_libpp_tx_trace_hal_mac_is_txq_enabled
compare-return true
arg-range 0 0 3

case queue-0-set
arg 0
mmio 0x20104d40=0
mmio 0x20104d50=0
mmio 0x20104d60=0
mmio 0x20104d70=0x80000000

case queue-1-clear
arg 1
mmio 0x20104d40=0x80000000
mmio 0x20104d50=0x80000000
mmio 0x20104d60=0
mmio 0x20104d70=0x80000000

case queue-2-set
arg 2
mmio 0x20104d40=0
mmio 0x20104d50=0x80000000
mmio 0x20104d60=0
mmio 0x20104d70=0

case queue-3-clear
arg 3
mmio 0x20104d40=0
mmio 0x20104d50=0x80000000
mmio 0x20104d60=0x80000000
mmio 0x20104d70=0x80000000

profile libpp-txq-valid
vendor-source libpp
vendor-symbol hal_mac_is_txq_valid
rust-symbol open_libpp_tx_trace_hal_mac_is_txq_valid
compare-return true
arg-range 0 0 3

case queue-0-clear
arg 0
mmio 0x20104d40=0x40000000
mmio 0x20104d50=0x40000000
mmio 0x20104d60=0x40000000
mmio 0x20104d70=0

case queue-1-set
arg 1
mmio 0x20104d40=0
mmio 0x20104d50=0
mmio 0x20104d60=0x40000000
mmio 0x20104d70=0

case queue-2-clear
arg 2
mmio 0x20104d40=0x40000000
mmio 0x20104d50=0
mmio 0x20104d60=0x40000000
mmio 0x20104d70=0x40000000

case queue-3-set
arg 3
mmio 0x20104d40=0x40000000
mmio 0x20104d50=0
mmio 0x20104d60=0
mmio 0x20104d70=0

profile libpp-set-txq-invalid
vendor-source libpp
vendor-symbol hal_mac_set_txq_invalid
rust-symbol open_libpp_tx_trace_hal_mac_set_txq_invalid
compare-return false
arg-range 0 0 3

case queue-0
arg 0
mmio 0x20104d70=0xffffffff

case queue-1
arg 1
mmio 0x20104d60=0xd2345678

case queue-2
arg 2
mmio 0x20104d50=0x41234567

case queue-3
arg 3
mmio 0x20104d40=0x87654321

profile libpp-disable-txq
vendor-source libpp
vendor-symbol hal_mac_txq_disable
rust-symbol open_libpp_tx_trace_hal_mac_txq_disable
compare-return false
arg-range 0 0 3

case queue-0
arg 0
mmio 0x20104d70=0xffffffff

case queue-1
arg 1
mmio 0x20104d60=0xd2345678

case queue-2
arg 2
mmio 0x20104d50=0x41234567

case queue-3
arg 3
mmio 0x20104d40=0x87654321
