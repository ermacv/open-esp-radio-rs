/*
 * Cold vendor PHY calibration for the hardware calibration cross-check.
 *
 * Registers the PHY once with full calibration (calibration storage is
 * disabled, so no retained data applies), then reports every generated
 * vendor object as one line per object:
 *
 *   oer-vendor-calibration <object> <hex bytes>
 *
 * then starts the Wi-Fi client without a connection and prints
 * `oer-vendor-calibration-end`. It then answers register reads
 * the host requests from the published register model:
 *
 *   r <hex address>   ->   oer-vendor-calibration-register <address> <value>
 *
 * The firmware holds no register list of its own. The host resets the board
 * for each repetition.
 */
#include <stdio.h>
#include <stdlib.h>

#include "driver/usb_serial_jtag.h"

#include "esp_event.h"
#include "esp_phy_init.h"
#include "esp_wifi.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "reported.h"

#define REPORT_PREFIX "oer-vendor-calibration"
/* Longest request line: "r " and eight hex digits. */
#define REQUEST_BYTES 16

/* Answer one `r <hex address>` request with the word read there. */
static void answer(const char *request)
{
    if (request[0] != 'r' || request[1] != ' ') {
        return;
    }
    uint32_t address = strtoul(request + 2, NULL, 16);
    uint32_t value = *(volatile const uint32_t *)address;
    char reply[64];
    int length = snprintf(reply, sizeof(reply), REPORT_PREFIX "-register %08lx %08lx\n",
                          (unsigned long)address, (unsigned long)value);
    usb_serial_jtag_write_bytes(reply, length, portMAX_DELAY);
}

void app_main(void)
{
    esp_phy_enable(PHY_MODEM_WIFI);
    for (size_t i = 0; i < sizeof(REPORTED_OBJECTS) / sizeof(REPORTED_OBJECTS[0]); i++) {
        const reported_object_t *object = &REPORTED_OBJECTS[i];
        printf(REPORT_PREFIX " %s ", object->name);
        for (size_t j = 0; j < object->length; j++) {
            printf("%02x", object->bytes[j]);
        }
        printf("\n");
    }
    /* Bring the Wi-Fi client up without a connection, as production's
     * role-neutral initialization does, so the register state is read at
     * the same lifecycle point: RX enabled on the default home channel. */
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    wifi_init_config_t wifi = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&wifi));
    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_STA));
    ESP_ERROR_CHECK(esp_wifi_start());
    printf(REPORT_PREFIX "-end\n");
    fflush(stdout);
    usb_serial_jtag_driver_config_t config = USB_SERIAL_JTAG_DRIVER_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(usb_serial_jtag_driver_install(&config));
    char request[REQUEST_BYTES];
    size_t length = 0;
    for (;;) {
        char byte;
        if (usb_serial_jtag_read_bytes(&byte, 1, portMAX_DELAY) != 1) {
            continue;
        }
        if (byte == '\n') {
            request[length] = '\0';
            answer(request);
            length = 0;
        } else if (length + 1 < sizeof(request)) {
            request[length++] = byte;
        }
    }
}
