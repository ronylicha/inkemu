/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * A reference firmware of inkemu, written for its tests: the wait for the SAR ADC that stopped
 * CrossPoint 1.6.5 and the stock firmware of the Xteink X4 under inkemu until the ADC was modelled,
 * and the panic of the interrupt watchdog, with code this repository can build, run and publish.
 * It reads a register of I2C0 and starts a one-shot conversion of ADC1 by direct register access,
 * as CrossPoint does (channel 1, attenuation 3), then waits for the end of the conversion
 * (`APB_SARADC.INT_RAW` bit 31) with interrupts masked, as `adc_oneshot_read` does
 * (`portENTER_CRITICAL` around `adc_oneshot_hal_convert`, ESP-IDF v5.5.2
 * components/esp_adc/adc_oneshot.c), and prints the reading. Then it clears ADC2's done bit and
 * waits the same way for the end of a conversion of ADC2 (bit 30), which nothing started: on a
 * chip as under inkemu, the interrupt watchdog ends that wait with a panic; the panic handler
 * writes a core dump to the `coredump` partition and restarts the CPU. The next life finds the
 * core dump and says so, and does nothing more.
 */
#include <inttypes.h>
#include <stdio.h>
#include "esp_core_dump.h"
#include "esp_err.h"
#include "freertos/FreeRTOS.h"
#include "soc/apb_saradc_reg.h"
#include "soc/i2c_reg.h"
#include "soc/soc.h"

/* The pad CrossPoint reads its first button ladder on, GPIO1, is channel 1 of ADC1. */
#define LADDER_CHANNEL 1u
/* Attenuation 3, the 0 to 2.5 V range CrossPoint converts with. */
#define LADDER_ATTENUATION 3u

static portMUX_TYPE adc_lock = portMUX_INITIALIZER_UNLOCKED;

void app_main(void)
{
    esp_err_t found = esp_core_dump_image_check();
    size_t address = 0;
    size_t size = 0;
    if (found == ESP_OK && esp_core_dump_image_get(&address, &size) == ESP_OK) {
        printf("adc_wait_panic: a core dump is in flash at 0x%x, %u bytes\n", (unsigned)address,
               (unsigned)size);
        return;
    }
    printf("adc_wait_panic: no core dump in flash (%s)\n", esp_err_to_name(found));

    printf("adc_wait_panic: I2C0 SR 0x%08" PRIx32 "\n", REG_READ(I2C_SR_REG(0)));

    REG_WRITE(APB_SARADC_INT_CLR_REG, APB_SARADC_ADC1_DONE_INT_CLR);
    uint32_t sample = APB_SARADC1_ONETIME_SAMPLE
                      | (LADDER_CHANNEL << APB_SARADC_ONETIME_CHANNEL_S)
                      | (LADDER_ATTENUATION << APB_SARADC_ONETIME_ATTEN_S);
    printf("adc_wait_panic: waiting for the SAR ADC, interrupts masked\n");
    portENTER_CRITICAL(&adc_lock);
    REG_WRITE(APB_SARADC_ONETIME_SAMPLE_REG, sample);
    REG_WRITE(APB_SARADC_ONETIME_SAMPLE_REG, sample | APB_SARADC_ONETIME_START);
    while ((REG_READ(APB_SARADC_INT_RAW_REG) & APB_SARADC_ADC1_DONE_INT_RAW) == 0) {
    }
    portEXIT_CRITICAL(&adc_lock);
    printf("adc_wait_panic: the SAR ADC answered 0x%03" PRIx32 "\n",
           REG_READ(APB_SARADC_1_DATA_STATUS_REG) & 0xFFFu);

    /* A done bit of ADC2 left over from before (the timed scan of the bootloader samples ADC2 on
     * a chip, bootloader_random_esp32c3.c) must not end the wait: it is cleared first. */
    REG_WRITE(APB_SARADC_INT_CLR_REG, APB_SARADC_ADC2_DONE_INT_CLR);
    printf("adc_wait_panic: waiting for ADC2, which converts nothing, interrupts masked\n");
    portENTER_CRITICAL(&adc_lock);
    while ((REG_READ(APB_SARADC_INT_RAW_REG) & APB_SARADC_ADC2_DONE_INT_RAW) == 0) {
    }
    portEXIT_CRITICAL(&adc_lock);
    printf("adc_wait_panic: ADC2 answered, which no chip does\n");
}
