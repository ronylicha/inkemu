/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * A reference firmware of inkemu, written for its tests (no ESP-IDF example reads stdin over the
 * USB-Serial-JTAG console with a small footprint): USB-Serial-JTAG is the primary console, stdin
 * and stdout go through its driver, and every line read is written back after "echo: ". The
 * console only works while a USB host reads the port: without the start-of-frame interrupts of a
 * host the output is discarded and stdin reads end, which is what its tests check.
 */
#include <stdio.h>
#include "esp_err.h"
#include "driver/usb_serial_jtag.h"
#include "driver/usb_serial_jtag_vfs.h"

void app_main(void)
{
    usb_serial_jtag_driver_config_t config = USB_SERIAL_JTAG_DRIVER_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(usb_serial_jtag_driver_install(&config));
    usb_serial_jtag_vfs_use_driver();
    setvbuf(stdin, NULL, _IONBF, 0);
    printf("ready\n");
    char line[64];
    while (fgets(line, sizeof line, stdin)) {
        printf("echo: %s", line);
    }
    printf("stdin closed\n");
}
