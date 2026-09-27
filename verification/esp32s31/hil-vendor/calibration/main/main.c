/*
 * Cold vendor PHY calibration for the hardware calibration cross-check.
 *
 * Registers the PHY once with full calibration (calibration storage is
 * disabled, so no retained data applies), then reports every generated
 * vendor object as one line per object:
 *
 *   oer-vendor-calibration <object> <hex bytes>
 *
 * followed by `oer-vendor-calibration-end`. It then answers register reads
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

#include "esp_phy_init.h"
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
