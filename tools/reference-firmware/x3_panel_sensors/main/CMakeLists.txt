# SPDX-License-Identifier: Apache-2.0
idf_component_register(SRCS "x3_panel_sensors_main.c"
                       PRIV_REQUIRES esp_driver_gpio esp_driver_spi esp_driver_i2c esp_timer)
