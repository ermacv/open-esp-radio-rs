# The peer application with the radio enable deferred and split into steps.
idf_component_register(
    SRCS "../../esp32c5-ieee802154/main/peer.c"
    PRIV_REQUIRES ieee802154 esp_timer esp_phy esp_hw_support esp_driver_uart esp_driver_usb_serial_jtag vfs
)
target_compile_definitions(${COMPONENT_LIB} PRIVATE OER_PEER_ENABLE_BISECT)
