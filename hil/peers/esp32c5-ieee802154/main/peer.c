/*
 * IEEE 802.15.4 reference peer for the open-esp-radio HIL.
 *
 * The vendor driver (`esp_ieee802154_*`) owns the radio; this application
 * only drives it through a line protocol on the console. Every protocol line
 * starts with '@'; the host ignores any other output. See README.md.
 */

#include <ctype.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "esp_attr.h"
#include "esp_err.h"
#include "esp_ieee802154.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/task.h"
#include "sdkconfig.h"

#if CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG
#include "driver/usb_serial_jtag.h"
#include "driver/usb_serial_jtag_vfs.h"
#else
#include "driver/uart.h"
#include "driver/uart_vfs.h"
#endif

#define PROTOCOL_VERSION 2
/* PHR length field: MAC bytes plus the two-byte FCS. */
#define MAX_PSDU 127
#define FCS_LEN 2
#define LINE_CAPACITY 320
#define EVENT_QUEUE_DEPTH 16

typedef enum {
    EVENT_RX,
    EVENT_TX_DONE,
    EVENT_TX_FAILED,
    EVENT_ED_DONE,
} event_kind_t;

typedef struct {
    event_kind_t kind;
    /* MAC bytes without PHR and FCS. */
    uint8_t length;
    uint8_t bytes[MAX_PSDU];
    /* RX: the frame; TX_DONE: the received acknowledgement, if any. */
    bool has_ack;
    bool pending;
    uint8_t channel;
    int8_t rssi;
    uint8_t lqi;
    int32_t code;
} peer_event_t;

static QueueHandle_t s_events;
static uint8_t s_tx_frame[1 + MAX_PSDU];

/* A numbered stream: the template carries the stream magic, and each frame
 * gets the next MAC sequence number and the next little-endian counter right
 * after the magic. Per-frame reports are counted instead of printed. */
static const uint8_t STREAM_MAGIC[4] = { 'O', 'E', 'R', 'S' };
static TaskHandle_t s_stream_task;
static esp_timer_handle_t s_stream_timer;
static volatile bool s_streaming;
static volatile uint32_t s_stream_done;
static volatile uint32_t s_stream_failed;
static uint32_t s_stream_count;
static size_t s_stream_counter_at;

static void copy_frame(peer_event_t *event, const uint8_t *frame)
{
    uint8_t psdu = frame[0];
    uint8_t length = psdu >= FCS_LEN ? psdu - FCS_LEN : 0;
    if (length > MAX_PSDU) {
        length = MAX_PSDU;
    }
    event->length = length;
    memcpy(event->bytes, &frame[1], length);
}

static void post(const peer_event_t *event)
{
    BaseType_t woken = pdFALSE;
    xQueueSendFromISR(s_events, event, &woken);
    portYIELD_FROM_ISR(woken);
}

/* Driver callbacks run in interrupt context: copy and defer. */

void IRAM_ATTR esp_ieee802154_receive_done(uint8_t *frame, esp_ieee802154_frame_info_t *frame_info)
{
    peer_event_t event = {
        .kind = EVENT_RX,
        .pending = frame_info->pending,
        .channel = frame_info->channel,
        .rssi = frame_info->rssi,
        .lqi = frame_info->lqi,
    };
    copy_frame(&event, frame);
    esp_ieee802154_receive_handle_done(frame);
    post(&event);
}

void IRAM_ATTR esp_ieee802154_transmit_done(const uint8_t *frame, const uint8_t *ack,
                                            esp_ieee802154_frame_info_t *ack_frame_info)
{
    if (s_streaming) {
        if (ack != NULL) {
            esp_ieee802154_receive_handle_done(ack);
        }
        s_stream_done++;
        return;
    }
    peer_event_t event = { .kind = EVENT_TX_DONE };
    if (ack != NULL) {
        event.has_ack = true;
        event.pending = ack_frame_info->pending;
        event.rssi = ack_frame_info->rssi;
        event.lqi = ack_frame_info->lqi;
        copy_frame(&event, ack);
        esp_ieee802154_receive_handle_done(ack);
    }
    post(&event);
}

void IRAM_ATTR esp_ieee802154_transmit_failed(const uint8_t *frame, esp_ieee802154_tx_error_t error)
{
    if (s_streaming) {
        s_stream_failed++;
        return;
    }
    peer_event_t event = { .kind = EVENT_TX_FAILED, .code = error };
    post(&event);
}

void IRAM_ATTR esp_ieee802154_energy_detect_done(int8_t power)
{
    peer_event_t event = { .kind = EVENT_ED_DONE, .rssi = power };
    post(&event);
}

static void print_hex(const uint8_t *bytes, uint8_t length)
{
    for (uint8_t index = 0; index < length; index++) {
        printf("%02x", bytes[index]);
    }
}

static void event_task(void *arg)
{
    peer_event_t event;
    for (;;) {
        if (xQueueReceive(s_events, &event, portMAX_DELAY) != pdTRUE) {
            continue;
        }
        switch (event.kind) {
        case EVENT_RX:
            printf("@RX ");
            print_hex(event.bytes, event.length);
            printf(" rssi=%d lqi=%u pending=%d ch=%u\n", event.rssi, event.lqi, event.pending,
                   event.channel);
            break;
        case EVENT_TX_DONE:
            printf("@TXDONE ack=");
            if (event.has_ack) {
                print_hex(event.bytes, event.length);
                printf(" pending=%d rssi=%d lqi=%u\n", event.pending, event.rssi, event.lqi);
            } else {
                printf("-\n");
            }
            break;
        case EVENT_TX_FAILED:
            printf("@TXFAIL %ld\n", (long)event.code);
            break;
        case EVENT_ED_DONE:
            printf("@ED %d\n", event.rssi);
            break;
        }
        fflush(stdout);
    }
}

static int hex_value(char digit)
{
    if (digit >= '0' && digit <= '9') {
        return digit - '0';
    }
    digit = (char)tolower((unsigned char)digit);
    if (digit >= 'a' && digit <= 'f') {
        return digit - 'a' + 10;
    }
    return -1;
}

/* Decode exactly `length` bytes; the text must hold exactly 2 * length digits. */
static bool decode_hex(const char *text, uint8_t *out, size_t length)
{
    if (strlen(text) != 2 * length) {
        return false;
    }
    for (size_t index = 0; index < length; index++) {
        int high = hex_value(text[2 * index]);
        int low = hex_value(text[2 * index + 1]);
        if (high < 0 || low < 0) {
            return false;
        }
        out[index] = (uint8_t)(high << 4 | low);
    }
    return true;
}

static bool parse_u32(const char *text, uint32_t *out, uint32_t maximum)
{
    char *end = NULL;
    unsigned long value = strtoul(text, &end, 10);
    if (text[0] == '\0' || *end != '\0' || value > maximum) {
        return false;
    }
    *out = (uint32_t)value;
    return true;
}

static bool parse_i8(const char *text, int8_t *out)
{
    char *end = NULL;
    long value = strtol(text, &end, 10);
    if (text[0] == '\0' || *end != '\0' || value < INT8_MIN || value > INT8_MAX) {
        return false;
    }
    *out = (int8_t)value;
    return true;
}

static void reply(esp_err_t error, const char *what)
{
    if (error == ESP_OK) {
        printf("@OK %s\n", what);
    } else {
        printf("@ERR %s %s\n", what, esp_err_to_name(error));
    }
    fflush(stdout);
}

/* CFG <channel> <panid:4hex> <short:4hex> <ext:16hex> <promiscuous> <power_dbm>
 *
 * PAN ID and short address are big-endian numbers; the extended address is
 * in over-the-air byte order, which is also the driver's storage order. The
 * driver always generates automatic acknowledgements. */
static void command_cfg(char **argv, int argc)
{
    uint32_t channel, promiscuous;
    uint8_t panid[2], short_address[2], extended[8];
    int8_t power;
    if (argc != 7 || !parse_u32(argv[1], &channel, 26) || channel < 11
        || !decode_hex(argv[2], panid, 2) || !decode_hex(argv[3], short_address, 2)
        || !decode_hex(argv[4], extended, 8) || !parse_u32(argv[5], &promiscuous, 1)
        || !parse_i8(argv[6], &power)) {
        reply(ESP_ERR_INVALID_ARG, "CFG");
        return;
    }
    esp_err_t error = esp_ieee802154_set_channel((uint8_t)channel);
    if (error == ESP_OK) {
        error = esp_ieee802154_set_panid((uint16_t)(panid[0] << 8 | panid[1]));
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_set_short_address(
            (uint16_t)(short_address[0] << 8 | short_address[1]));
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_set_extended_address(extended);
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_set_promiscuous(promiscuous != 0);
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_set_rx_when_idle(true);
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_set_txpower(power);
    }
    reply(error, "CFG");
}

/* TX <cca 0|1> <MAC bytes as hex, without FCS> */
static void command_tx(char **argv, int argc)
{
    uint32_t cca;
    size_t digits = argc == 3 ? strlen(argv[2]) : 0;
    size_t length = digits / 2;
    if (argc != 3 || !parse_u32(argv[1], &cca, 1) || digits % 2 != 0 || length == 0
        || length + FCS_LEN > MAX_PSDU || !decode_hex(argv[2], &s_tx_frame[1], length)) {
        reply(ESP_ERR_INVALID_ARG, "TX");
        return;
    }
    s_tx_frame[0] = (uint8_t)(length + FCS_LEN);
    reply(esp_ieee802154_transmit(s_tx_frame, cca != 0), "TX");
}

/* PENDING <mode 0..3> | PENDING ADD <short:4hex> | PENDING CLEAR */
static void command_pending(char **argv, int argc)
{
    uint32_t mode;
    uint8_t short_address[2];
    esp_err_t error = ESP_ERR_INVALID_ARG;
    if (argc == 2 && strcmp(argv[1], "CLEAR") == 0) {
        error = esp_ieee802154_reset_pending_table(true);
    } else if (argc == 3 && strcmp(argv[1], "ADD") == 0 && decode_hex(argv[2], short_address, 2)) {
        /* The driver stores short addresses little-endian. */
        uint8_t stored[2] = { short_address[1], short_address[0] };
        error = esp_ieee802154_add_pending_addr(stored, true);
    } else if (argc == 2 && parse_u32(argv[1], &mode, 3)) {
        error = esp_ieee802154_set_pending_mode((esp_ieee802154_pending_mode_t)mode);
    }
    reply(error, "PENDING");
}

/* ED <duration in 16 us symbols> */
static void command_ed(char **argv, int argc)
{
    uint32_t duration;
    if (argc != 2 || !parse_u32(argv[1], &duration, UINT16_MAX) || duration == 0) {
        reply(ESP_ERR_INVALID_ARG, "ED");
        return;
    }
    reply(esp_ieee802154_energy_detect(duration), "ED");
}

static void stream_tick(void *arg)
{
    xTaskNotifyGive(s_stream_task);
}

/* Send the stream frame by frame, one per timer tick, then report. */
static void stream_task(void *arg)
{
    uint8_t sequence = s_tx_frame[1 + 2];
    for (uint32_t counter = 0; counter < s_stream_count; counter++) {
        ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
        s_tx_frame[1 + 2] = sequence++;
        s_tx_frame[1 + s_stream_counter_at] = (uint8_t)counter;
        s_tx_frame[1 + s_stream_counter_at + 1] = (uint8_t)(counter >> 8);
        if (esp_ieee802154_transmit(s_tx_frame, false) != ESP_OK) {
            s_stream_failed++;
        }
    }
    esp_timer_stop(s_stream_timer);
    /* Let the last transmission report before the counts are read. */
    vTaskDelay(pdMS_TO_TICKS(20));
    s_streaming = false;
    printf("@STREAMDONE sent=%lu done=%lu failed=%lu\n", (unsigned long)s_stream_count,
           (unsigned long)s_stream_done, (unsigned long)s_stream_failed);
    fflush(stdout);
    s_stream_task = NULL;
    vTaskDelete(NULL);
}

/* STREAM <count 1..4096> <interval_ms 1..1000> <template MAC bytes as hex>
 *
 * The template must contain the stream magic followed by two counter bytes. */
static void command_stream(char **argv, int argc)
{
    uint32_t count, interval;
    size_t digits = argc == 4 ? strlen(argv[3]) : 0;
    size_t length = digits / 2;
    if (argc != 4 || s_stream_task != NULL || !parse_u32(argv[1], &count, 4096) || count == 0
        || !parse_u32(argv[2], &interval, 1000) || interval == 0 || digits % 2 != 0
        || length < 3 || length + FCS_LEN > MAX_PSDU
        || !decode_hex(argv[3], &s_tx_frame[1], length)) {
        reply(ESP_ERR_INVALID_ARG, "STREAM");
        return;
    }
    size_t at = 0;
    while (at + sizeof(STREAM_MAGIC) + 2 <= length
           && memcmp(&s_tx_frame[1 + at], STREAM_MAGIC, sizeof(STREAM_MAGIC)) != 0) {
        at++;
    }
    if (at + sizeof(STREAM_MAGIC) + 2 > length) {
        reply(ESP_ERR_INVALID_ARG, "STREAM");
        return;
    }
    s_tx_frame[0] = (uint8_t)(length + FCS_LEN);
    s_stream_counter_at = at + sizeof(STREAM_MAGIC);
    s_stream_count = count;
    s_stream_done = 0;
    s_stream_failed = 0;
    s_streaming = true;
    if (s_stream_timer == NULL) {
        const esp_timer_create_args_t args = { .callback = stream_tick, .name = "stream" };
        ESP_ERROR_CHECK(esp_timer_create(&args, &s_stream_timer));
    }
    xTaskCreate(stream_task, "peer_stream", 3072, NULL, 6, &s_stream_task);
    reply(esp_timer_start_periodic(s_stream_timer, (uint64_t)interval * 1000), "STREAM");
}

static void dispatch(char *line)
{
    char *argv[8];
    int argc = 0;
    for (char *token = strtok(line, " "); token != NULL && argc < 8; token = strtok(NULL, " ")) {
        argv[argc++] = token;
    }
    if (argc == 0) {
        return;
    }
    if (strcmp(argv[0], "CFG") == 0) {
        command_cfg(argv, argc);
    } else if (strcmp(argv[0], "RX") == 0 && argc == 1) {
        reply(esp_ieee802154_receive(), "RX");
    } else if (strcmp(argv[0], "SLEEP") == 0 && argc == 1) {
        reply(esp_ieee802154_sleep(), "SLEEP");
    } else if (strcmp(argv[0], "TX") == 0) {
        command_tx(argv, argc);
    } else if (strcmp(argv[0], "PENDING") == 0) {
        command_pending(argv, argc);
    } else if (strcmp(argv[0], "ED") == 0) {
        command_ed(argv, argc);
    } else if (strcmp(argv[0], "STREAM") == 0) {
        command_stream(argv, argc);
    } else {
        printf("@ERR %s unknown\n", argv[0]);
        fflush(stdout);
    }
}

static void console_init(void)
{
#if CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG
    usb_serial_jtag_driver_config_t config = USB_SERIAL_JTAG_DRIVER_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(usb_serial_jtag_driver_install(&config));
    usb_serial_jtag_vfs_use_driver();
#else
    ESP_ERROR_CHECK(uart_driver_install(CONFIG_ESP_CONSOLE_UART_NUM, 2 * LINE_CAPACITY, 0, 0, NULL, 0));
    uart_vfs_dev_use_driver(CONFIG_ESP_CONSOLE_UART_NUM);
#endif
    setvbuf(stdin, NULL, _IONBF, 0);
}

void app_main(void)
{
    s_events = xQueueCreate(EVENT_QUEUE_DEPTH, sizeof(peer_event_t));
    configASSERT(s_events != NULL);
    console_init();
    ESP_ERROR_CHECK(esp_ieee802154_enable());
    xTaskCreate(event_task, "peer_events", 4096, NULL, 5, NULL);
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
            printf("@ERR LINE overflow\n");
            fflush(stdout);
        }
    }
}
