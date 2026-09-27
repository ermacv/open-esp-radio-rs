/*
 * Cold vendor PHY calibration for the hardware calibration cross-check.
 *
 * Registers the PHY once with full calibration (calibration storage is
 * disabled, so no retained data applies), then reports every generated
 * vendor object as one line per object:
 *
 *   oer-vendor-calibration <object> <hex bytes>
 *
 * followed by `oer-vendor-calibration-end`, and stays idle. The host resets
 * the board for each repetition.
 */
#include <stdio.h>

#include "esp_phy_init.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "reported.h"

#define REPORT_PREFIX "oer-vendor-calibration"

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
    for (;;) {
        vTaskDelay(portMAX_DELAY);
    }
}
