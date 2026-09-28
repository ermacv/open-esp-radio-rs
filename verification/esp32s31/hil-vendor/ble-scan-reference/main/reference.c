/*
 * Vendor reference for the open Bluetooth LE scanner of the ESP32-S31.
 *
 * The pinned vendor BLE Controller runs a passive legacy scan over VHCI with
 * the scan parameters the open scanner's HIL workload uses. A linker wrap of
 * the memory-manager call the scanner's item-end callback makes on a
 * completed scheduler item snapshots that item, the scan state machine and
 * its link state, which the host compares with the open Controller's memory.
 * Every protocol line starts with '@'; the host ignores any other output.
 *
 * Symbols of the pinned libble_app 10c5077:
 * - sym_ble_GrcxGSe23Od1VdL3UBbX: global pointer; its +0x10 is the legacy
 *   scan state machine, whose +0x1c is the link state and +0x20 the item in
 *   flight;
 * - r_sym_ble_6dUmBPDoEG5h9BPr5ciK: the scanner item-end callback, stored at
 *   item +0x58;
 * - r_sym_memMgmt_232y4Maqyh8DfDo3KxfV(item): the first call of that
 *   callback after it reads item +0x38.
 */

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "driver/usb_serial_jtag.h"
#include "driver/usb_serial_jtag_vfs.h"
#include "esp_bt.h"
#include "esp_err.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/semphr.h"
#include "freertos/task.h"
#include "nvs_flash.h"

#define PROTOCOL_VERSION 1
#define LINE_CAPACITY 64
#define COMMAND_TIMEOUT_MS 2000

#define HCI_COMMAND 0x01
#define HCI_EVENT 0x04
#define EVENT_COMMAND_COMPLETE 0x0e
#define EVENT_COMMAND_STATUS 0x0f
#define EVENT_LE_META 0x3e
#define LE_ADVERTISING_REPORT 0x02

#define OP_RESET 0x0c03
#define OP_SET_EVENT_MASK 0x0c01
#define OP_LE_SET_SCAN_PARAMETERS 0x200b
#define OP_LE_SET_SCAN_ENABLE 0x200c

#define LINK_STATE_WORDS (0x84 / 4)
#define ITEM_WORDS (0x60 / 4)
#define STATE_WORDS (0x80 / 4)
#define SNAPSHOTS 4

extern void *sym_ble_GrcxGSe23Od1VdL3UBbX;
extern void r_sym_ble_6dUmBPDoEG5h9BPr5ciK(void);
extern uint32_t __real_r_sym_memMgmt_232y4Maqyh8DfDo3KxfV(void *a0, void *a1, void *a2);

typedef struct {
    uint32_t item[ITEM_WORDS];
    uint32_t link[LINK_STATE_WORDS];
    uint32_t state[STATE_WORDS];
    /* The SCAN_REQ TX descriptor at link +0x6c and the buffer it names. */
    uint32_t tx_descriptor[4];
    /* The scheduler context the item's +0x04 names (state +0x64). */
    uint32_t context[8];
    uint32_t tx_buffer[12];
    uint32_t item_address;
} snapshot_t;

static snapshot_t s_snapshots[SNAPSHOTS];

/* BLE MAC register windows read once at the first completed scan item. */
typedef struct {
    uint32_t base;
    uint32_t words;
} window_t;

static const window_t s_windows[] = {
    { 0x20101000, (0x201014c8 - 0x20101000) / 4 },
    { 0x2010180c, 6 },
    { 0x20101970, 4 },
    { 0x20109880, 16 },
};
#define REGISTER_WORDS (306 + 6 + 4 + 16)
static uint32_t s_registers[REGISTER_WORDS];
static volatile bool s_registers_taken;
static volatile uint32_t s_snapshot_count;
static volatile uint32_t s_completed_items;
static volatile uint32_t s_reports;
static volatile uint32_t s_scan_responses;

static void copy_words(uint32_t *out, const volatile uint32_t *in, size_t count)
{
    for (size_t index = 0; index < count; index++) {
        out[index] = in[index];
    }
}

/* Runs in the Controller's context on every memory-manager release. */
uint32_t __wrap_r_sym_memMgmt_232y4Maqyh8DfDo3KxfV(void *a0, void *a1, void *a2)
{
    const volatile uint32_t *item = a0;
    if (item != NULL && item[0x58 / 4] == (uint32_t)(uintptr_t)&r_sym_ble_6dUmBPDoEG5h9BPr5ciK) {
        s_completed_items++;
        if (!s_registers_taken) {
            size_t out = 0;
            for (size_t window = 0; window < sizeof(s_windows) / sizeof(s_windows[0]); window++) {
                const volatile uint32_t *base =
                    (const volatile uint32_t *)(uintptr_t)s_windows[window].base;
                for (uint32_t word = 0; word < s_windows[window].words && out < REGISTER_WORDS;
                     word++) {
                    s_registers[out++] = base[word];
                }
            }
            s_registers_taken = true;
        }
        uint32_t index = s_snapshot_count;
        if (index < SNAPSHOTS) {
            snapshot_t *snapshot = &s_snapshots[index];
            snapshot->item_address = (uint32_t)(uintptr_t)item;
            copy_words(snapshot->item, item, ITEM_WORDS);
            const volatile uint8_t *root = sym_ble_GrcxGSe23Od1VdL3UBbX;
            const volatile uint32_t *state =
                root != NULL ? *(uint32_t *const volatile *)(root + 0x10) : NULL;
            if (state != NULL) {
                copy_words(snapshot->state, state, STATE_WORDS);
                const volatile uint32_t *context =
                    (const volatile uint32_t *)(uintptr_t)state[0x64 / 4];
                if (context != NULL) {
                    copy_words(snapshot->context, context, 8);
                }
                const volatile uint32_t *link = (const volatile uint32_t *)(uintptr_t)state[0x1c / 4];
                if (link != NULL) {
                    copy_words(snapshot->link, link, LINK_STATE_WORDS);
                    const volatile uint32_t *descriptor =
                        (const volatile uint32_t *)(uintptr_t)link[0x6c / 4];
                    if (descriptor != NULL) {
                        copy_words(snapshot->tx_descriptor, descriptor, 4);
                        const volatile uint32_t *buffer = (const volatile uint32_t *)(uintptr_t)(
                            0x2f000000u | ((descriptor[1] & 0xfffffu) << 2));
                        copy_words(snapshot->tx_buffer, buffer, 12);
                    }
                }
            }
            s_snapshot_count = index + 1;
        }
    }
    return __real_r_sym_memMgmt_232y4Maqyh8DfDo3KxfV(a0, a1, a2);
}

typedef struct {
    uint16_t opcode;
    uint8_t status;
} completion_t;

static QueueHandle_t s_completions;
static SemaphoreHandle_t s_send_available;

static void notify_host_send_available(void)
{
    xSemaphoreGive(s_send_available);
}

/* Runs in the Controller's context: count reports, pass completions on. */
static int notify_host_recv(uint8_t *data, uint16_t length)
{
    if (length < 4 || data[0] != HCI_EVENT) {
        return 0;
    }
    if (data[1] == EVENT_LE_META && data[3] == LE_ADVERTISING_REPORT) {
        s_reports++;
        /* Code, length, subevent, report count, then the first report's
         * event type: 4 is SCAN_RSP. */
        if (length > 5 && data[5] == 0x04) {
            s_scan_responses++;
        }
        return 0;
    }
    completion_t completion = { 0 };
    if (data[1] == EVENT_COMMAND_COMPLETE && length >= 7) {
        completion.opcode = (uint16_t)(data[4] | data[5] << 8);
        completion.status = data[6];
    } else if (data[1] == EVENT_COMMAND_STATUS && length >= 7) {
        completion.status = data[3];
        completion.opcode = (uint16_t)(data[5] | data[6] << 8);
    } else {
        return 0;
    }
    xQueueSend(s_completions, &completion, 0);
    return 0;
}

static const esp_vhci_host_callback_t s_vhci_callbacks = {
    .notify_host_send_available = notify_host_send_available,
    .notify_host_recv = notify_host_recv,
};

/* Send one command and wait for its successful completion. */
static bool hci_command(uint16_t opcode, const uint8_t *parameters, uint8_t length)
{
    uint8_t packet[4 + 16];
    if (length > sizeof(packet) - 4) {
        return false;
    }
    packet[0] = HCI_COMMAND;
    packet[1] = (uint8_t)opcode;
    packet[2] = (uint8_t)(opcode >> 8);
    packet[3] = length;
    memcpy(&packet[4], parameters, length);
    TickType_t deadline = xTaskGetTickCount() + pdMS_TO_TICKS(COMMAND_TIMEOUT_MS);
    while (!esp_vhci_host_check_send_available()) {
        TickType_t now = xTaskGetTickCount();
        if (now >= deadline || xSemaphoreTake(s_send_available, deadline - now) != pdTRUE) {
            return false;
        }
    }
    xQueueReset(s_completions);
    esp_vhci_host_send_packet(packet, (uint16_t)(4 + length));
    completion_t completion;
    for (;;) {
        TickType_t now = xTaskGetTickCount();
        if (now >= deadline || xQueueReceive(s_completions, &completion, deadline - now) != pdTRUE) {
            return false;
        }
        if (completion.opcode == opcode) {
            return completion.status == 0;
        }
    }
}

static void print_words(const char *tag, uint32_t index, const uint32_t *words, size_t count)
{
    printf("@%s %lu", tag, (unsigned long)index);
    for (size_t word = 0; word < count; word++) {
        printf(" %08lx", (unsigned long)words[word]);
    }
    printf("\n");
}

/* Scan for `milliseconds` with the open workload's parameters: a 60 ms
 * interval and a 30 ms window on all primary channels, passive or active. */
static void scan(uint32_t milliseconds, bool active)
{
    static const uint8_t event_mask[8] = { 0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0x00, 0x20 };
    const uint8_t parameters[7] = { active ? 0x01 : 0x00, 0x60, 0x00, 0x30, 0x00, 0x00, 0x00 };
    static const uint8_t enable[2] = { 0x01, 0x00 };
    static const uint8_t disable[2] = { 0x00, 0x00 };
    s_snapshot_count = 0;
    s_registers_taken = false;
    s_completed_items = 0;
    s_reports = 0;
    s_scan_responses = 0;
    if (!hci_command(OP_RESET, NULL, 0) || !hci_command(OP_SET_EVENT_MASK, event_mask, 8)
        || !hci_command(OP_LE_SET_SCAN_PARAMETERS, parameters, 7)
        || !hci_command(OP_LE_SET_SCAN_ENABLE, enable, 2)) {
        printf("@ERR SCAN hci\n");
        return;
    }
    vTaskDelay(pdMS_TO_TICKS(milliseconds));
    if (!hci_command(OP_LE_SET_SCAN_ENABLE, disable, 2)) {
        printf("@ERR SCAN disable\n");
        return;
    }
    uint32_t count = s_snapshot_count;
    printf("@SCAN reports=%lu scan_responses=%lu completed=%lu snapshots=%lu\n",
           (unsigned long)s_reports, (unsigned long)s_scan_responses,
           (unsigned long)s_completed_items, (unsigned long)count);
    for (uint32_t index = 0; index < count; index++) {
        const snapshot_t *snapshot = &s_snapshots[index];
        printf("@ITEMAT %lu %08lx\n", (unsigned long)index, (unsigned long)snapshot->item_address);
        print_words("ITEM", index, snapshot->item, ITEM_WORDS);
        print_words("LINK", index, snapshot->link, LINK_STATE_WORDS);
        print_words("STATE", index, snapshot->state, STATE_WORDS);
        print_words("TXD", index, snapshot->tx_descriptor, 4);
        print_words("CTX", index, snapshot->context, 8);
        print_words("TXB", index, snapshot->tx_buffer, 12);
    }
    if (s_registers_taken) {
        size_t out = 0;
        for (size_t window = 0; window < sizeof(s_windows) / sizeof(s_windows[0]); window++) {
            for (uint32_t word = 0; word < s_windows[window].words; word += 8) {
                printf("@REG %08lx", (unsigned long)(s_windows[window].base + word * 4));
                for (uint32_t index = word; index < word + 8 && index < s_windows[window].words;
                     index++) {
                    printf(" %08lx", (unsigned long)s_registers[out++]);
                }
                printf("\n");
            }
        }
    }
    printf("@OK SCAN\n");
}

static void console_init(void)
{
    usb_serial_jtag_driver_config_t config = USB_SERIAL_JTAG_DRIVER_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(usb_serial_jtag_driver_install(&config));
    usb_serial_jtag_vfs_use_driver();
    setvbuf(stdin, NULL, _IONBF, 0);
}

static void controller_init(void)
{
    esp_err_t error = nvs_flash_init();
    if (error == ESP_ERR_NVS_NO_FREE_PAGES || error == ESP_ERR_NVS_NEW_VERSION_FOUND) {
        ESP_ERROR_CHECK(nvs_flash_erase());
        error = nvs_flash_init();
    }
    ESP_ERROR_CHECK(error);
    esp_bt_controller_config_t config = BT_CONTROLLER_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_bt_controller_init(&config));
    ESP_ERROR_CHECK(esp_bt_controller_enable(ESP_BT_MODE_BLE));
    ESP_ERROR_CHECK(esp_vhci_host_register_callback(&s_vhci_callbacks));
}

static void dispatch(char *line)
{
    char *command = strtok(line, " ");
    if (command == NULL) {
        return;
    }
    if (strcmp(command, "SCAN") == 0) {
        char *argument = strtok(NULL, " ");
        long milliseconds = argument != NULL ? strtol(argument, NULL, 10) : 0;
        if (milliseconds <= 0 || milliseconds > 10000) {
            printf("@ERR SCAN invalid\n");
        } else {
            char *mode = strtok(NULL, " ");
            scan((uint32_t)milliseconds, mode != NULL && strcmp(mode, "ACTIVE") == 0);
        }
    } else if (strcmp(command, "SYNC") == 0) {
        printf("@READY protocol=%d target=%s\n", PROTOCOL_VERSION, CONFIG_IDF_TARGET);
        printf("@OK SYNC\n");
    } else {
        printf("@ERR %s unknown\n", command);
    }
    fflush(stdout);
}

void app_main(void)
{
    s_completions = xQueueCreate(4, sizeof(completion_t));
    s_send_available = xSemaphoreCreateBinary();
    configASSERT(s_completions != NULL && s_send_available != NULL);
    console_init();
    controller_init();
    printf("@READY protocol=%d target=%s\n", PROTOCOL_VERSION, CONFIG_IDF_TARGET);
    fflush(stdout);
    static char line[LINE_CAPACITY];
    size_t length = 0;
    for (;;) {
        int character = getchar();
        if (character == EOF) {
            vTaskDelay(1);
            continue;
        }
        if (character == '\r') {
            continue;
        }
        if (character == '\n') {
            line[length] = '\0';
            dispatch(line);
            length = 0;
            continue;
        }
        if (length + 1 < sizeof(line)) {
            line[length++] = (char)character;
        } else {
            length = 0;
        }
    }
}
