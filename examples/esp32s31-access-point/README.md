# ESP32-S31 access point

The example starts a WPA2-Personal 20 MHz access point at `192.168.4.1/24`.
Clients receive addresses from `192.168.4.100..=114`; UDP and TCP echo use
port 7.

```sh
ESP32S31_AP_SSID=open-radio \
ESP32S31_AP_PASSPHRASE=replace-this-password \
cargo run --release
```

The driver owns radio, DMA, interrupts, associations and WPA2 keys. Static IP,
DHCP and echo services are application responsibilities implemented only in
this example. `AP_CLIENT_LIMIT` may be set from 1 through 15. The current AP
data path is legacy ERP; this example does not claim HT or A-MPDU support.
The 15-client value is the checked resource ceiling. Host tests cover all 15
AID/key slots; current physical HIL qualification uses two concurrent WPA2
clients under sustained RX, TX and bidirectional load.
