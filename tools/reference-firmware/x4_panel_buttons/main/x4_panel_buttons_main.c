/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * A reference firmware of inkemu, written for its tests: the board of the Xteink X4 as a firmware
 * uses it, through the ESP-IDF drivers, on the pins of the X4 (crates/boards/profiles/x4.toml,
 * `[wiring]`). Its tests run it on the wired X4 (ADR 0013) and check what it prints, what the
 * panel shows and how the run ends.
 *
 * 1. Power: it holds the latch of its supply (GPIO13) high at once, reads the cable (GPIO20) and the
 *    battery through its divider (GPIO0, ADC1 channel 0), and prints both.
 * 2. The panel, an SSD1677 on GP-SPI2 (SCLK GPIO8, MOSI GPIO10, CS GPIO21 by the controller, D/C
 *    GPIO4, RST GPIO5, BUSY GPIO6 active high), driven as the X4's drivers drive it: hardware and
 *    software reset, the internal temperature sensor, 480 gates scanned with SM = 1, the top row of
 *    the glass written to the last RAM row (data entry 01h), then a full refresh (22h F7h) of a
 *    white image with a black frame and a black square in its top-left corner. Both planes hold the
 *    image after it: the red plane is the previous frame of the differential refresh. It waits for
 *    BUSY by the interrupt of its falling edge, a semaphore given by the handler, as FreeInk does
 *    (2d-D5, 2d-D21): the fall the panel announces reaches the CPU as an interrupt.
 * 3. The buttons: the two resistor ladders (GPIO1, GPIO2, ADC1 channels 1 and 2, 12 dB, curve
 *    fitting) read every 20 ms; a button seen pressed on two readings in a row is a press, which it
 *    prints, then draws a black square in the row of squares at the place of that button and shows
 *    it by a differential refresh (22h FCh).
 * 4. The power key (GPIO3, low while pressed): held at power-on by the gesture of the board, it is
 *    armed once seen released. Pressed then on battery, the firmware lets go of its latch, and the
 *    X4 goes off when the key is released. Pressed with the cable in, which keeps the X4 powered
 *    whatever the latch, the firmware waits for the release and goes into deep sleep woken by the
 *    power key (GPIO3 low), as CrossPoint does on a cable; woken, its next life says so and its
 *    full refresh draws a second black square, in the bottom-right corner.
 *
 * Every line it prints starts with "x4_panel_buttons: ".
 */
#include <assert.h>
#include <inttypes.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>
#include "driver/gpio.h"
#include "driver/spi_master.h"
#include "esp_adc/adc_cali.h"
#include "esp_adc/adc_cali_scheme.h"
#include "esp_adc/adc_oneshot.h"
#include "esp_attr.h"
#include "esp_err.h"
#include "esp_sleep.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/semphr.h"
#include "freertos/task.h"

#define PIN_BATTERY 0 /* ADC1 channel 0 */
#define PIN_LADDER_1 1 /* ADC1 channel 1 */
#define PIN_LADDER_2 2 /* ADC1 channel 2 */
#define PIN_POWER_KEY 3
#define PIN_PANEL_DC 4
#define PIN_PANEL_RST 5
#define PIN_PANEL_BUSY 6
#define PIN_SCLK 8
#define PIN_MOSI 10
#define PIN_LATCH 13
#define PIN_USB_DETECT 20
#define PIN_PANEL_CS 21

/* The glass, as the controller scans it. */
#define WIDTH 800
#define HEIGHT 480
#define ROW_BYTES (WIDTH / 8)
#define FRAME_BYTES (ROW_BYTES * HEIGHT)

/* What one SPI transaction carries without DMA. */
#define CHUNK 64

/* A ladder reads idle above this; the X4's rail is 3.3 V, the ADC tops out near 3.1 V. */
#define IDLE_ABOVE_MV 2900
/* How far from its level a reading may be and still name a button. */
#define TOLERANCE_MV 250

#define POLL_MS 20

struct button {
    const char *id;
    int millivolts;
};

/* The levels of crates/boards/profiles/x4.toml, the order of its keys. */
static const struct button LADDER_1[] = {
    {"back", 2448}, {"confirm", 1896}, {"left", 1061}, {"right", 4},
};
static const struct button LADDER_2[] = {
    {"up", 1584}, {"down", 4},
};
#define LADDER_1_COUNT (sizeof LADDER_1 / sizeof LADDER_1[0])
#define LADDER_2_COUNT (sizeof LADDER_2 / sizeof LADDER_2[0])

static uint8_t frame[FRAME_BYTES];
static spi_device_handle_t panel;
static adc_oneshot_unit_handle_t adc;

static void say(const char *text)
{
    printf("x4_panel_buttons: %s\n", text);
}

/* Paints black the rectangle [x0, x1) × [y0, y1) of the frame (a 0 bit is black). */
static void paint_black(int x0, int y0, int x1, int y1)
{
    for (int y = y0; y < y1; y++) {
        for (int x = x0; x < x1; x++) {
            frame[y * ROW_BYTES + x / 8] &= (uint8_t)~(0x80u >> (x % 8));
        }
    }
}

/* Given by the interrupt of BUSY's falling edge; the falls it saw. */
static SemaphoreHandle_t busy_fell;
static volatile uint32_t busy_falls;

static void IRAM_ATTR on_busy_fall(void *arg)
{
    (void)arg;
    busy_falls++;
    BaseType_t woken = pdFALSE;
    xSemaphoreGiveFromISR(busy_fell, &woken);
    if (woken == pdTRUE) {
        portYIELD_FROM_ISR();
    }
}

/*
 * Waits for BUSY to fall, by its interrupt as FreeInk does, never by polling: the task sleeps on
 * the semaphore with no timeout, so a fall the interrupt does not report hangs the firmware and
 * fails its tests. A give left from an earlier fall only makes the loop read the level again.
 */
static void wait_while_busy(void)
{
    while (gpio_get_level(PIN_PANEL_BUSY) == 1) {
        xSemaphoreTake(busy_fell, portMAX_DELAY);
    }
}

static void send(const uint8_t *bytes, size_t len)
{
    for (size_t at = 0; at < len; at += CHUNK) {
        size_t part = len - at < CHUNK ? len - at : CHUNK;
        spi_transaction_t transaction = {
            .length = 8 * part,
            .tx_buffer = bytes + at,
        };
        ESP_ERROR_CHECK(spi_device_polling_transmit(panel, &transaction));
    }
}

static void command(uint8_t code, const uint8_t *data, size_t len)
{
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_DC, 0));
    send(&code, 1);
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_DC, 1));
    if (len > 0) {
        send(data, len);
    }
}

/* The whole glass as the window, the counter at its top row (the last RAM row). */
static void whole_window(void)
{
    static const uint8_t entry[] = {0x01};
    static const uint8_t x_window[] = {0x00, 0x00, (WIDTH - 1) % 256, (WIDTH - 1) / 256};
    static const uint8_t y_window[] = {(HEIGHT - 1) % 256, (HEIGHT - 1) / 256, 0x00, 0x00};
    static const uint8_t x_counter[] = {0x00, 0x00};
    static const uint8_t y_counter[] = {(HEIGHT - 1) % 256, (HEIGHT - 1) / 256};
    command(0x11, entry, sizeof entry);
    command(0x44, x_window, sizeof x_window);
    command(0x45, y_window, sizeof y_window);
    command(0x4E, x_counter, sizeof x_counter);
    command(0x4F, y_counter, sizeof y_counter);
}

static void write_plane(uint8_t plane)
{
    whole_window();
    command(plane, frame, sizeof frame);
}

/* Runs the stages `control` enables and waits for BUSY; returns the milliseconds it stayed high. */
static int64_t refresh(uint8_t control)
{
    command(0x22, &control, 1);
    int64_t started = esp_timer_get_time();
    command(0x20, NULL, 0);
    wait_while_busy();
    return (esp_timer_get_time() - started) / 1000;
}

/* Whether this life began with a wake by the power key from deep sleep. */
static bool woken_by_the_power_key;

static void panel_part(void)
{
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_RST, 0));
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_DC, 1));
    const gpio_config_t outputs = {
        .pin_bit_mask = BIT64(PIN_PANEL_RST) | BIT64(PIN_PANEL_DC),
        .mode = GPIO_MODE_OUTPUT,
    };
    ESP_ERROR_CHECK(gpio_config(&outputs));
    busy_fell = xSemaphoreCreateBinary();
    assert(busy_fell != NULL);
    const gpio_config_t busy = {
        .pin_bit_mask = BIT64(PIN_PANEL_BUSY),
        .mode = GPIO_MODE_INPUT,
        .intr_type = GPIO_INTR_NEGEDGE,
    };
    ESP_ERROR_CHECK(gpio_config(&busy));
    ESP_ERROR_CHECK(gpio_install_isr_service(0));
    ESP_ERROR_CHECK(gpio_isr_handler_add(PIN_PANEL_BUSY, on_busy_fall, NULL));
    const spi_bus_config_t bus = {
        .mosi_io_num = PIN_MOSI,
        .miso_io_num = -1,
        .sclk_io_num = PIN_SCLK,
        .quadwp_io_num = -1,
        .quadhd_io_num = -1,
        .max_transfer_sz = CHUNK,
    };
    ESP_ERROR_CHECK(spi_bus_initialize(SPI2_HOST, &bus, SPI_DMA_DISABLED));
    const spi_device_interface_config_t device = {
        .clock_speed_hz = 10 * 1000 * 1000,
        .mode = 0,
        .spics_io_num = PIN_PANEL_CS,
        .queue_size = 1,
    };
    ESP_ERROR_CHECK(spi_bus_add_device(SPI2_HOST, &device, &panel));

    vTaskDelay(pdMS_TO_TICKS(10));
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_RST, 1));
    vTaskDelay(pdMS_TO_TICKS(10));
    command(0x12, NULL, 0);
    wait_while_busy();
    static const uint8_t internal_sensor[] = {0x80};
    command(0x18, internal_sensor, sizeof internal_sensor);
    /* 480 gates (MUX 1DFh), SM = 1. */
    static const uint8_t driver_output[] = {(HEIGHT - 1) % 256, (HEIGHT - 1) / 256, 0x02};
    command(0x01, driver_output, sizeof driver_output);

    memset(frame, 0xFF, sizeof frame);
    paint_black(0, 0, WIDTH, 4);
    paint_black(0, HEIGHT - 4, WIDTH, HEIGHT);
    paint_black(0, 0, 4, HEIGHT);
    paint_black(WIDTH - 4, 0, WIDTH, HEIGHT);
    paint_black(16, 16, 80, 80);
    if (woken_by_the_power_key) {
        paint_black(WIDTH - 80, HEIGHT - 80, WIDTH - 16, HEIGHT - 16);
    }
    write_plane(0x24);
    write_plane(0x26);
    int64_t ms = refresh(0xF7);
    printf("x4_panel_buttons: full refresh, busy %" PRId64 " ms, %" PRIu32
           " falls of busy by interrupt\n",
           ms, busy_falls);
}

static adc_cali_handle_t calibrated(adc_channel_t channel)
{
    const adc_oneshot_chan_cfg_t config = {
        .atten = ADC_ATTEN_DB_12,
        .bitwidth = ADC_BITWIDTH_DEFAULT,
    };
    ESP_ERROR_CHECK(adc_oneshot_config_channel(adc, channel, &config));
    const adc_cali_curve_fitting_config_t calibration = {
        .unit_id = ADC_UNIT_1,
        .chan = channel,
        .atten = ADC_ATTEN_DB_12,
        .bitwidth = ADC_BITWIDTH_DEFAULT,
    };
    adc_cali_handle_t handle;
    ESP_ERROR_CHECK(adc_cali_create_scheme_curve_fitting(&calibration, &handle));
    return handle;
}

static int millivolts(adc_channel_t channel, adc_cali_handle_t calibration)
{
    int raw = 0;
    int mv = 0;
    ESP_ERROR_CHECK(adc_oneshot_read(adc, channel, &raw));
    ESP_ERROR_CHECK(adc_cali_raw_to_voltage(calibration, raw, &mv));
    return mv;
}

/* The index of the button of `ladder` whose level `mv` is nearest, -1 when idle or none is near. */
static int button_of(const struct button *ladder, size_t count, int mv)
{
    if (mv > IDLE_ABOVE_MV) {
        return -1;
    }
    int best = -1;
    int best_distance = TOLERANCE_MV + 1;
    for (size_t i = 0; i < count; i++) {
        int distance = mv > ladder[i].millivolts ? mv - ladder[i].millivolts
                                                 : ladder[i].millivolts - mv;
        if (distance < best_distance) {
            best = (int)i;
            best_distance = distance;
        }
    }
    return best;
}

/* What a ladder showed: the button seen on the last reading, and the one taken as pressed. */
struct ladder_state {
    int seen;
    int pressed;
};

static int presses;

/* Shows a press of the button at `place` (0 to 5, the order of the profile's keys). */
static void show_press(const char *id, int place)
{
    presses++;
    printf("x4_panel_buttons: pressed %s\n", id);
    int x = 120 + 96 * place;
    paint_black(x, 216, x + 48, 264);
    write_plane(0x24);
    int64_t ms = refresh(0xFC);
    write_plane(0x26);
    printf("x4_panel_buttons: partial refresh %d, busy %" PRId64 " ms\n", presses, ms);
}

static void watch_ladder(struct ladder_state *state, const struct button *ladder, size_t count,
                         int mv, int first_place)
{
    int now = button_of(ladder, count, mv);
    if (now == state->seen && now != state->pressed) {
        state->pressed = now;
        if (now >= 0) {
            show_press(ladder[now].id, first_place + now);
        }
    }
    state->seen = now;
}

/* With the cable in: deep sleep once the key is let go, woken when it is pressed again. */
static void sleep_until_the_power_key(void)
{
    say("power key pressed, cable in: deep sleep until the power key");
    while (gpio_get_level(PIN_POWER_KEY) == 0) {
        vTaskDelay(pdMS_TO_TICKS(POLL_MS));
    }
    say("power key released, sleeping");
    fflush(stdout);
    vTaskDelay(pdMS_TO_TICKS(POLL_MS));
    ESP_ERROR_CHECK(
        esp_deep_sleep_enable_gpio_wakeup(BIT64(PIN_POWER_KEY), ESP_GPIO_WAKEUP_GPIO_LOW));
    esp_deep_sleep_start();
}

/* On battery: the latch let go; the key holds the supply until it is released. */
static void turn_off(void)
{
    say("power key pressed, latch released");
    fflush(stdout);
    vTaskDelay(pdMS_TO_TICKS(POLL_MS));
    ESP_ERROR_CHECK(gpio_set_level(PIN_LATCH, 0));
    for (;;) {
        vTaskDelay(pdMS_TO_TICKS(1000));
    }
}

void app_main(void)
{
    woken_by_the_power_key = esp_sleep_get_wakeup_cause() == ESP_SLEEP_WAKEUP_GPIO;
    /* The latch first: the supply holds whatever the power key does from now on. */
    ESP_ERROR_CHECK(gpio_set_level(PIN_LATCH, 1));
    const gpio_config_t latch = {
        .pin_bit_mask = BIT64(PIN_LATCH),
        .mode = GPIO_MODE_OUTPUT,
    };
    ESP_ERROR_CHECK(gpio_config(&latch));
    const gpio_config_t inputs = {
        .pin_bit_mask = BIT64(PIN_POWER_KEY) | BIT64(PIN_USB_DETECT),
        .mode = GPIO_MODE_INPUT,
        .pull_up_en = GPIO_PULLUP_DISABLE,
    };
    ESP_ERROR_CHECK(gpio_config(&inputs));
    ESP_ERROR_CHECK(gpio_pullup_en(PIN_POWER_KEY));
    say(woken_by_the_power_key ? "start, woken by the power key, latch held"
                               : "start, latch held");
    printf("x4_panel_buttons: usb cable %s\n",
           gpio_get_level(PIN_USB_DETECT) == 1 ? "plugged" : "unplugged");

    const adc_oneshot_unit_init_cfg_t unit = {.unit_id = ADC_UNIT_1};
    ESP_ERROR_CHECK(adc_oneshot_new_unit(&unit, &adc));
    adc_cali_handle_t battery = calibrated(ADC_CHANNEL_0);
    adc_cali_handle_t ladder_1 = calibrated(ADC_CHANNEL_1);
    adc_cali_handle_t ladder_2 = calibrated(ADC_CHANNEL_2);
    /* Two equal resistors: the pin reads half the cell. */
    printf("x4_panel_buttons: battery %d mV\n", 2 * millivolts(ADC_CHANNEL_0, battery));

    panel_part();
    say("ready");

    struct ladder_state first = {-1, -1};
    struct ladder_state second = {-1, -1};
    bool armed = false;
    for (;;) {
        watch_ladder(&first, LADDER_1, LADDER_1_COUNT, millivolts(ADC_CHANNEL_1, ladder_1), 0);
        watch_ladder(&second, LADDER_2, LADDER_2_COUNT, millivolts(ADC_CHANNEL_2, ladder_2),
                     LADDER_1_COUNT);
        bool power_pressed = gpio_get_level(PIN_POWER_KEY) == 0;
        if (!armed && !power_pressed) {
            armed = true;
            say("power key released, armed");
        } else if (armed && power_pressed) {
            if (gpio_get_level(PIN_USB_DETECT) == 1) {
                sleep_until_the_power_key();
            }
            turn_off();
        }
        vTaskDelay(pdMS_TO_TICKS(POLL_MS));
    }
}
