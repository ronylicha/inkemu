# SPDX-License-Identifier: Apache-2.0
idf_component_register(SRCS "x4_panel_buttons_main.c"
                       PRIV_REQUIRES esp_driver_gpio esp_driver_spi esp_adc esp_timer)
