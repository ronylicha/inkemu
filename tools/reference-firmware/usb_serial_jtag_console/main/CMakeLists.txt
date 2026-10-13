# SPDX-License-Identifier: Apache-2.0
idf_component_register(SRCS "usb_serial_jtag_console_main.c"
                       PRIV_REQUIRES esp_driver_usb_serial_jtag vfs)
