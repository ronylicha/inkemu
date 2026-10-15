/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * A reference firmware of inkemu, written for its tests: the peripherals of the ESP32-C3 through
 * which the Xteink X4 reaches its chips, driven by the ESP-IDF drivers, on the pins of the X4
 * (FreeInk's board header, docs/research/esp32c3-soc-peripherals-sp2b.md). Its test attaches
 * chips to the emulated pins (ADR 0012) and checks what they saw and what the firmware prints.
 *
 * 1. GPIO: the reset line of the panel (GPIO5) is an output, held low then released; its BUSY
 *    line (GPIO6) is an input with an interrupt on both edges, whose handler counts the edges.
 *    The test's panel answers the release with five edges of BUSY, 1 ms apart.
 * 2. GP-SPI2 in CPU mode (no DMA) on the bus of the X4 (SCLK GPIO8, MOSI GPIO10, MISO GPIO7):
 *    the panel, its chip select GPIO21 driven by the controller and its data/command line GPIO4
 *    by software, receives a command byte, then two data bytes in full duplex; the SD card, its
 *    chip select GPIO12 driven by software at 400 kHz, receives the six bytes of CMD0. The test's
 *    chips answer each byte with a function of it, which the firmware prints.
 * 3. The SAR ADC: a one-shot conversion of GPIO1 (ADC1 channel 1) at 12 dB, converted to
 *    millivolts by the curve-fitting calibration of the chip's eFuse.
 * 4. I2C0 (SDA GPIO2, SCL GPIO3, internal pull-ups): a probe of address 0x68, which no chip
 *    acknowledges, as on an X4: ESP_ERR_NOT_FOUND.
 */
#include <inttypes.h>
#include <stdio.h>
#include <string.h>
#include "driver/gpio.h"
#include "driver/i2c_master.h"
#include "driver/spi_master.h"
#include "esp_adc/adc_cali.h"
#include "esp_adc/adc_cali_scheme.h"
#include "esp_adc/adc_oneshot.h"
#include "esp_attr.h"
#include "esp_err.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#define PIN_SCLK 8
#define PIN_MOSI 10
#define PIN_MISO 7
#define PIN_PANEL_CS 21
#define PIN_PANEL_DC 4
#define PIN_PANEL_RST 5
#define PIN_PANEL_BUSY 6
#define PIN_CARD_CS 12
#define PIN_I2C_SDA 2
#define PIN_I2C_SCL 3

/* The edges of BUSY seen by the interrupt handler. */
static volatile uint32_t busy_edges;

static void IRAM_ATTR on_busy_edge(void *arg)
{
    (void)arg;
    busy_edges++;
}

static void print_bytes(const char *what, const uint8_t *bytes, size_t len)
{
    printf("soc_signals: %s", what);
    for (size_t i = 0; i < len; i++) {
        printf(" %02x", bytes[i]);
    }
    printf("\n");
}

static void gpio_part(void)
{
    /* The levels first, then the outputs: no glitch on the lines. */
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_RST, 0));
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_DC, 1));
    ESP_ERROR_CHECK(gpio_set_level(PIN_CARD_CS, 1));
    const gpio_config_t outputs = {
        .pin_bit_mask = BIT64(PIN_PANEL_RST) | BIT64(PIN_PANEL_DC) | BIT64(PIN_CARD_CS),
        .mode = GPIO_MODE_OUTPUT,
    };
    ESP_ERROR_CHECK(gpio_config(&outputs));
    const gpio_config_t busy = {
        .pin_bit_mask = BIT64(PIN_PANEL_BUSY),
        .mode = GPIO_MODE_INPUT,
        .intr_type = GPIO_INTR_ANYEDGE,
    };
    ESP_ERROR_CHECK(gpio_config(&busy));
    ESP_ERROR_CHECK(gpio_install_isr_service(0));
    ESP_ERROR_CHECK(gpio_isr_handler_add(PIN_PANEL_BUSY, on_busy_edge, NULL));
    printf("soc_signals: busy level %d before the reset is released\n",
           gpio_get_level(PIN_PANEL_BUSY));
    vTaskDelay(pdMS_TO_TICKS(10));
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_RST, 1));
    vTaskDelay(pdMS_TO_TICKS(20));
    printf("soc_signals: busy edges %" PRIu32 ", busy level %d\n", busy_edges,
           gpio_get_level(PIN_PANEL_BUSY));
}

static void spi_part(void)
{
    const spi_bus_config_t bus = {
        .mosi_io_num = PIN_MOSI,
        .miso_io_num = PIN_MISO,
        .sclk_io_num = PIN_SCLK,
        .quadwp_io_num = -1,
        .quadhd_io_num = -1,
        .max_transfer_sz = 64,
    };
    ESP_ERROR_CHECK(spi_bus_initialize(SPI2_HOST, &bus, SPI_DMA_DISABLED));
    const spi_device_interface_config_t panel_config = {
        .clock_speed_hz = 10 * 1000 * 1000,
        .mode = 0,
        .spics_io_num = PIN_PANEL_CS,
        .queue_size = 1,
    };
    spi_device_handle_t panel;
    ESP_ERROR_CHECK(spi_bus_add_device(SPI2_HOST, &panel_config, &panel));
    const spi_device_interface_config_t card_config = {
        .clock_speed_hz = 400 * 1000,
        .mode = 0,
        .spics_io_num = -1,
        .queue_size = 1,
    };
    spi_device_handle_t card;
    ESP_ERROR_CHECK(spi_bus_add_device(SPI2_HOST, &card_config, &card));

    /* The panel: a command byte with DC low, then two data bytes with DC high. */
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_DC, 0));
    spi_transaction_t command = {
        .length = 8,
        .flags = SPI_TRANS_USE_TXDATA,
        .tx_data = {0x12},
    };
    ESP_ERROR_CHECK(spi_device_polling_transmit(panel, &command));
    ESP_ERROR_CHECK(gpio_set_level(PIN_PANEL_DC, 1));
    spi_transaction_t data = {
        .length = 16,
        .flags = SPI_TRANS_USE_TXDATA | SPI_TRANS_USE_RXDATA,
        .tx_data = {0x01, 0x80},
    };
    ESP_ERROR_CHECK(spi_device_polling_transmit(panel, &data));
    print_bytes("panel answered", data.rx_data, 2);

    /* The card: CMD0 with its chip select held low by software. */
    static const uint8_t cmd0[6] = {0x40, 0x00, 0x00, 0x00, 0x00, 0x95};
    uint8_t answer[6];
    memset(answer, 0, sizeof answer);
    spi_transaction_t frame = {
        .length = 8 * sizeof cmd0,
        .tx_buffer = cmd0,
        .rx_buffer = answer,
    };
    ESP_ERROR_CHECK(gpio_set_level(PIN_CARD_CS, 0));
    ESP_ERROR_CHECK(spi_device_polling_transmit(card, &frame));
    ESP_ERROR_CHECK(gpio_set_level(PIN_CARD_CS, 1));
    print_bytes("card answered", answer, sizeof answer);
}

static void adc_part(void)
{
    adc_oneshot_unit_handle_t unit;
    const adc_oneshot_unit_init_cfg_t unit_config = {.unit_id = ADC_UNIT_1};
    ESP_ERROR_CHECK(adc_oneshot_new_unit(&unit_config, &unit));
    const adc_oneshot_chan_cfg_t channel = {
        .atten = ADC_ATTEN_DB_12,
        .bitwidth = ADC_BITWIDTH_DEFAULT,
    };
    ESP_ERROR_CHECK(adc_oneshot_config_channel(unit, ADC_CHANNEL_1, &channel));
    adc_cali_handle_t calibration;
    const adc_cali_curve_fitting_config_t calibration_config = {
        .unit_id = ADC_UNIT_1,
        .chan = ADC_CHANNEL_1,
        .atten = ADC_ATTEN_DB_12,
        .bitwidth = ADC_BITWIDTH_DEFAULT,
    };
    ESP_ERROR_CHECK(adc_cali_create_scheme_curve_fitting(&calibration_config, &calibration));
    int raw = 0;
    int millivolts = 0;
    ESP_ERROR_CHECK(adc_oneshot_read(unit, ADC_CHANNEL_1, &raw));
    ESP_ERROR_CHECK(adc_cali_raw_to_voltage(calibration, raw, &millivolts));
    printf("soc_signals: adc raw %d, %d mV\n", raw, millivolts);
}

static void i2c_part(void)
{
    const i2c_master_bus_config_t bus_config = {
        .i2c_port = I2C_NUM_0,
        .sda_io_num = PIN_I2C_SDA,
        .scl_io_num = PIN_I2C_SCL,
        .clk_source = I2C_CLK_SRC_DEFAULT,
        .glitch_ignore_cnt = 7,
        .flags.enable_internal_pullup = true,
    };
    i2c_master_bus_handle_t bus;
    ESP_ERROR_CHECK(i2c_new_master_bus(&bus_config, &bus));
    esp_err_t probed = i2c_master_probe(bus, 0x68, 100);
    printf("soc_signals: i2c probe of 0x68: %s\n", esp_err_to_name(probed));
}

void app_main(void)
{
    printf("soc_signals: start\n");
    gpio_part();
    spi_part();
    adc_part();
    i2c_part();
    printf("soc_signals: done\n");
}
