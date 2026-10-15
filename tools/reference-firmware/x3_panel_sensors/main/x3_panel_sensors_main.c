/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * A reference firmware of inkemu, written for its tests: the board of the Xteink X3 as a firmware
 * uses it, through the ESP-IDF drivers, on the pins of the X3 (crates/boards/profiles/x3.toml,
 * `[wiring]`). Its tests run it on the wired X3 (ADR 0015) and check what it prints, what the
 * panel shows, what it left in the chips of the I2C bus and how the run ends.
 *
 * 1. The card's supply: GPIO13 driven high, as CrossPoint does first.
 * 2. The I2C bus (SDA GPIO20, SCL GPIO0, 400 kHz, the pads' pull-ups) through the `i2c_master`
 *    driver, each read a register pointer, a repeated START and the bytes, as CrossPoint's `Wire`
 *    makes it:
 *    - the BQ27220 at 0x55: voltage (0x08), state of charge (0x2C), current (0x0C), design
 *      capacity (0x3C), and the subcommand DEVICE_NUMBER written to Control(), answered in
 *      MACData();
 *    - the DS3231 at 0x68: its status (0x0F), its control set to 0x04 and read back, its time; then
 *      the time set to 2026-10-15 12:34:56, a Thursday, and read again 1.5 s later;
 *    - the QMI8658 at 0x6B: WHO_AM_I and REVISION_ID, then set up as CrossPoint sets it (address
 *      auto-increment, ±2 g, ±512 dps, both sensors on), and 20 ms later its data-ready bits, its
 *      temperature, its acceleration and its rate;
 *    - a probe of 0x6A, where nothing answers.
 * 3. The panel, a UC8253 on GP-SPI2 (SCLK GPIO8, MOSI GPIO10, CS GPIO21 by the controller, D/C
 *    GPIO4, RST GPIO5, BUSY GPIO6, low while busy), driven as FreeInk's X3 driver drives it: the
 *    reset, KW mode with its waveform in the LUT registers (these are this firmware's own: 20
 *    frames of VDL toward white, of VDH toward black), 792 × 600, every plane written last row
 *    first. A full refresh draws a white image with a black frame, a black square in its top-left
 *    corner and a bar as long as four pixels for each percent of the battery; a refresh in partial
 *    mode then draws a second square, inside its window only. It waits for BUSY by the interrupt
 *    of its rising edge, a semaphore given by the handler. Then the supplies go off and the
 *    controller into deep sleep.
 * 4. The power key (GPIO3, low while pressed), held at power-on by the gesture of the board: once
 *    it is released, the firmware goes into deep sleep woken by it, the X3's way of powering off.
 *
 * Every line it prints starts with "x3_panel_sensors: ".
 */
#include <assert.h>
#include <inttypes.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>
#include "driver/gpio.h"
#include "driver/i2c_master.h"
#include "driver/spi_master.h"
#include "esp_attr.h"
#include "esp_err.h"
#include "esp_sleep.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/semphr.h"
#include "freertos/task.h"

#define PIN_I2C_SCL 0
#define PIN_POWER_KEY 3
#define PIN_PANEL_DC 4
#define PIN_PANEL_RST 5
#define PIN_PANEL_BUSY 6
#define PIN_SCLK 8
#define PIN_MOSI 10
#define PIN_SD_POWER 13
#define PIN_I2C_SDA 20
#define PIN_PANEL_CS 21

#define GAUGE 0x55
#define CLOCK 0x68
#define MOTION 0x6B
#define NOBODY 0x6A
#define I2C_HZ 400000
#define I2C_TIMEOUT_MS 50

/* The glass, as FreeInk holds its frame: 792 × 528, a 1 bit white. */
#define WIDTH 792
#define HEIGHT 528
#define ROW_BYTES (WIDTH / 8)
#define FRAME_BYTES (ROW_BYTES * HEIGHT)

/* The second square, drawn by the refresh in partial mode: its window, 8-pixel aligned. */
#define SQUARE_X 400
#define SQUARE_Y 300
#define SQUARE_SIDE 64

/* What one SPI transaction carries without DMA. */
#define CHUNK 64

#define POLL_MS 20

static uint8_t frame[FRAME_BYTES];
static spi_device_handle_t panel;
static i2c_master_bus_handle_t bus;

static void say(const char *text)
{
    printf("x3_panel_sensors: %s\n", text);
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

/* --- I2C ------------------------------------------------------------------------------------ */

static i2c_master_dev_handle_t device_at(uint16_t address)
{
    const i2c_device_config_t config = {
        .dev_addr_length = I2C_ADDR_BIT_LEN_7,
        .device_address = address,
        .scl_speed_hz = I2C_HZ,
    };
    i2c_master_dev_handle_t device;
    ESP_ERROR_CHECK(i2c_master_bus_add_device(bus, &config, &device));
    return device;
}

/* `count` bytes from register `reg`: the pointer, a repeated START, the bytes, a STOP. */
static void read_registers(i2c_master_dev_handle_t device, uint8_t reg, uint8_t *bytes,
                           size_t count)
{
    ESP_ERROR_CHECK(
        i2c_master_transmit_receive(device, &reg, 1, bytes, count, I2C_TIMEOUT_MS));
}

static void write_registers(i2c_master_dev_handle_t device, const uint8_t *bytes, size_t count)
{
    ESP_ERROR_CHECK(i2c_master_transmit(device, bytes, count, I2C_TIMEOUT_MS));
}

/* A 16-bit little-endian standard command of the gauge. */
static uint16_t gauge_word(i2c_master_dev_handle_t gauge, uint8_t command)
{
    uint8_t bytes[2];
    read_registers(gauge, command, bytes, sizeof bytes);
    return (uint16_t)(bytes[0] | bytes[1] << 8);
}

static int state_of_charge;

static void gauge_part(void)
{
    i2c_master_dev_handle_t gauge = device_at(GAUGE);
    unsigned voltage = gauge_word(gauge, 0x08);
    state_of_charge = gauge_word(gauge, 0x2C);
    int current = (int16_t)gauge_word(gauge, 0x0C);
    unsigned design = gauge_word(gauge, 0x3C);
    printf("x3_panel_sensors: gauge %u mV, %d %%, %d mA, design %u mAh\n", voltage,
           state_of_charge, current, design);
    /* DEVICE_NUMBER (0x0001) written to Control() in one transaction, answered in MACData(). */
    static const uint8_t device_number[] = {0x00, 0x01, 0x00};
    write_registers(gauge, device_number, sizeof device_number);
    printf("x3_panel_sensors: gauge device number 0x%04x\n", gauge_word(gauge, 0x40));
}

static unsigned from_bcd(uint8_t value)
{
    return (value >> 4) * 10u + (value & 0x0Fu);
}

static uint8_t to_bcd(unsigned value)
{
    return (uint8_t)(value / 10 << 4 | value % 10);
}

/* The seven time registers as `YYYY-MM-DDTHH:MM:SS, day D` (24-hour, years 20xx). */
static void say_time(const char *what, i2c_master_dev_handle_t clock)
{
    uint8_t t[7];
    read_registers(clock, 0x00, t, sizeof t);
    printf("x3_panel_sensors: clock %s 20%02u-%02u-%02uT%02u:%02u:%02u, day %u\n", what,
           from_bcd(t[6]), from_bcd(t[5] & 0x1F), from_bcd(t[4] & 0x3F), from_bcd(t[2] & 0x3F),
           from_bcd(t[1] & 0x7F), from_bcd(t[0] & 0x7F), t[3] & 0x07);
}

static void clock_part(void)
{
    i2c_master_dev_handle_t clock = device_at(CLOCK);
    uint8_t status = 0;
    read_registers(clock, 0x0F, &status, 1);
    static const uint8_t control[] = {0x0E, 0x04};
    write_registers(clock, control, sizeof control);
    uint8_t read_back = 0;
    read_registers(clock, 0x0E, &read_back, 1);
    printf("x3_panel_sensors: clock status 0x%02x, control 0x%02x\n", status, read_back);
    say_time("at start", clock);
    const uint8_t set[] = {0x00,       to_bcd(56), to_bcd(34), to_bcd(12),
                           4,          to_bcd(15), to_bcd(10), to_bcd(26)};
    write_registers(clock, set, sizeof set);
    vTaskDelay(pdMS_TO_TICKS(1500));
    say_time("1.5 s after it was set", clock);
}

static void motion_part(void)
{
    i2c_master_dev_handle_t motion = device_at(MOTION);
    uint8_t id[2];
    read_registers(motion, 0x00, id, 1);
    read_registers(motion, 0x01, id + 1, 1);
    printf("x3_panel_sensors: motion who am i 0x%02x, revision 0x%02x\n", id[0], id[1]);
    /* CTRL7 off, CTRL1 auto-increment, CTRL2 ±2 g, CTRL3 ±512 dps, CTRL7 both sensors on. */
    static const uint8_t setup[][2] = {
        {0x08, 0x00}, {0x02, 0x40}, {0x03, 0x06}, {0x04, 0x56}, {0x08, 0x03},
    };
    for (size_t i = 0; i < sizeof setup / sizeof setup[0]; i++) {
        write_registers(motion, setup[i], 2);
    }
    vTaskDelay(pdMS_TO_TICKS(20));
    uint8_t status = 0;
    read_registers(motion, 0x2E, &status, 1);
    uint8_t data[14];
    read_registers(motion, 0x33, data, sizeof data);
    int16_t values[6];
    for (int axis = 0; axis < 6; axis++) {
        values[axis] = (int16_t)(data[2 + 2 * axis] | data[3 + 2 * axis] << 8);
    }
    printf("x3_panel_sensors: motion status 0x%02x, temperature %d + %d/256 C\n", status,
           (int8_t)data[1], data[0]);
    printf("x3_panel_sensors: motion acceleration %d %d %d, rate %d %d %d\n", values[0],
           values[1], values[2], values[3], values[4], values[5]);
}

static void i2c_part(void)
{
    const i2c_master_bus_config_t config = {
        .i2c_port = I2C_NUM_0,
        .sda_io_num = PIN_I2C_SDA,
        .scl_io_num = PIN_I2C_SCL,
        .clk_source = I2C_CLK_SRC_DEFAULT,
        .glitch_ignore_cnt = 7,
        .flags.enable_internal_pullup = true,
    };
    ESP_ERROR_CHECK(i2c_new_master_bus(&config, &bus));
    gauge_part();
    clock_part();
    motion_part();
    esp_err_t probed = i2c_master_probe(bus, NOBODY, I2C_TIMEOUT_MS);
    printf("x3_panel_sensors: probe of 0x%02x: %s\n", NOBODY, esp_err_to_name(probed));
}

/* --- The panel ------------------------------------------------------------------------------ */

/* Given by the interrupt of BUSY's rising edge. */
static SemaphoreHandle_t busy_rose;

static void IRAM_ATTR on_busy_rise(void *arg)
{
    (void)arg;
    BaseType_t woken = pdFALSE;
    xSemaphoreGiveFromISR(busy_rose, &woken);
    if (woken == pdTRUE) {
        portYIELD_FROM_ISR();
    }
}

/* Waits while BUSY is low, by its interrupt, never by polling: a rise the interrupt does not
 * report hangs the firmware and fails its tests. */
static void wait_while_busy(void)
{
    while (gpio_get_level(PIN_PANEL_BUSY) == 0) {
        xSemaphoreTake(busy_rose, portMAX_DELAY);
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

/* Sends `code` and waits for BUSY; returns the milliseconds it stayed low. */
static int64_t busy_command(uint8_t code)
{
    int64_t started = esp_timer_get_time();
    command(code, NULL, 0);
    wait_while_busy();
    return (esp_timer_get_time() - started) / 1000;
}

/* Rows `first` to `last` of the frame, last first, as FreeInk's `sendPlaneFlipped` sends them. */
static void send_rows_flipped(int first, int last, int x0, int x1)
{
    for (int y = last; y >= first; y--) {
        send(frame + y * ROW_BYTES + x0 / 8, (size_t)(x1 - x0) / 8);
    }
}

static void plane(uint8_t code)
{
    command(code, NULL, 0);
    send_rows_flipped(0, HEIGHT - 1, 0, WIDTH);
    command(0x11, NULL, 0);
}

/* A LUT of one group: `levels` in phase A for 20 frames, run once. */
static void lut(uint8_t code, uint8_t levels)
{
    uint8_t bytes[42] = {levels, 20, 0, 0, 0, 1};
    command(code, bytes, sizeof bytes);
}

static void panel_part(void)
{
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_RST, 0));
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_DC, 1));
    const gpio_config_t outputs = {
        .pin_bit_mask = BIT64(PIN_PANEL_RST) | BIT64(PIN_PANEL_DC),
        .mode = GPIO_MODE_OUTPUT,
    };
    ESP_ERROR_CHECK(gpio_config(&outputs));
    busy_rose = xSemaphoreCreateBinary();
    assert(busy_rose != NULL);
    const gpio_config_t busy = {
        .pin_bit_mask = BIT64(PIN_PANEL_BUSY),
        .mode = GPIO_MODE_INPUT,
        .intr_type = GPIO_INTR_POSEDGE,
    };
    ESP_ERROR_CHECK(gpio_config(&busy));
    ESP_ERROR_CHECK(gpio_install_isr_service(0));
    ESP_ERROR_CHECK(gpio_isr_handler_add(PIN_PANEL_BUSY, on_busy_rise, NULL));
    const spi_bus_config_t spi = {
        .mosi_io_num = PIN_MOSI,
        .miso_io_num = -1,
        .sclk_io_num = PIN_SCLK,
        .quadwp_io_num = -1,
        .quadhd_io_num = -1,
        .max_transfer_sz = CHUNK,
    };
    ESP_ERROR_CHECK(spi_bus_initialize(SPI2_HOST, &spi, SPI_DMA_DISABLED));
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
    wait_while_busy();
    /* KW mode, LUTs from the registers, gate scan up, source shift right, booster on. */
    static const uint8_t panel_setting[] = {0x3F, 0x0A};
    command(0x00, panel_setting, sizeof panel_setting);
    static const uint8_t resolution[] = {0x03, 0x18, 0x02, 0x58};
    command(0x61, resolution, sizeof resolution);
    static const uint8_t power_setting[] = {0x07, 0x17, 0x3F, 0x3F, 0x17};
    command(0x01, power_setting, sizeof power_setting);
    static const uint8_t frame_rate[] = {0x09};
    command(0x30, frame_rate, sizeof frame_rate);
    /* Copy NEW to OLD after each refresh, a 1 bit white. */
    static const uint8_t interface[] = {0x29, 0x07};
    command(0x50, interface, sizeof interface);
    lut(0x20, 0x00);
    lut(0x21, 0x80);
    lut(0x22, 0x80);
    lut(0x23, 0x40);
    lut(0x24, 0x40);

    memset(frame, 0xFF, sizeof frame);
    plane(0x10);
    paint_black(0, 0, WIDTH, 4);
    paint_black(0, HEIGHT - 4, WIDTH, HEIGHT);
    paint_black(0, 0, 4, HEIGHT);
    paint_black(WIDTH - 4, 0, WIDTH, HEIGHT);
    paint_black(16, 16, 80, 80);
    paint_black(16, 100, 16 + 4 * state_of_charge, 124);
    plane(0x13);
    printf("x3_panel_sensors: power on, busy %" PRId64 " ms\n", busy_command(0x04));
    printf("x3_panel_sensors: full refresh, busy %" PRId64 " ms\n", busy_command(0x12));

    /* The second square, in partial mode: its window in RAM, where the frame's rows lie flipped. */
    paint_black(SQUARE_X, SQUARE_Y, SQUARE_X + SQUARE_SIDE, SQUARE_Y + SQUARE_SIDE);
    const int ram_first = HEIGHT - (SQUARE_Y + SQUARE_SIDE);
    const int ram_last = HEIGHT - 1 - SQUARE_Y;
    const int x_end = SQUARE_X + SQUARE_SIDE - 1;
    const uint8_t window[] = {
        SQUARE_X >> 8, SQUARE_X & 0xF8, x_end >> 8, x_end & 0xF8,
        ram_first >> 8, ram_first & 0xFF, ram_last >> 8, ram_last & 0xFF, 0x01,
    };
    command(0x91, NULL, 0);
    command(0x90, window, sizeof window);
    command(0x13, NULL, 0);
    send_rows_flipped(SQUARE_Y, SQUARE_Y + SQUARE_SIDE - 1, SQUARE_X, SQUARE_X + SQUARE_SIDE);
    command(0x11, NULL, 0);
    printf("x3_panel_sensors: partial refresh, busy %" PRId64 " ms\n", busy_command(0x12));
    command(0x92, NULL, 0);

    printf("x3_panel_sensors: power off, busy %" PRId64 " ms\n", busy_command(0x02));
    static const uint8_t check[] = {0xA5};
    command(0x07, check, sizeof check);
    say("panel asleep");
}

/* --- The power key -------------------------------------------------------------------------- */

static void sleep_until_the_power_key(void)
{
    const gpio_config_t key = {
        .pin_bit_mask = BIT64(PIN_POWER_KEY),
        .mode = GPIO_MODE_INPUT,
        .pull_up_en = GPIO_PULLUP_ENABLE,
    };
    ESP_ERROR_CHECK(gpio_config(&key));
    while (gpio_get_level(PIN_POWER_KEY) == 0) {
        vTaskDelay(pdMS_TO_TICKS(POLL_MS));
    }
    say("power key released, deep sleep until it is pressed");
    fflush(stdout);
    vTaskDelay(pdMS_TO_TICKS(POLL_MS));
    ESP_ERROR_CHECK(
        esp_deep_sleep_enable_gpio_wakeup(BIT64(PIN_POWER_KEY), ESP_GPIO_WAKEUP_GPIO_LOW));
    esp_deep_sleep_start();
}

void app_main(void)
{
    ESP_ERROR_CHECK(gpio_set_level(PIN_SD_POWER, 1));
    const gpio_config_t sd_power = {
        .pin_bit_mask = BIT64(PIN_SD_POWER),
        .mode = GPIO_MODE_OUTPUT,
    };
    ESP_ERROR_CHECK(gpio_config(&sd_power));
    say(esp_sleep_get_wakeup_cause() == ESP_SLEEP_WAKEUP_GPIO
            ? "start, woken by the power key, card powered"
            : "start, card powered");
    i2c_part();
    panel_part();
    sleep_until_the_power_key();
}
