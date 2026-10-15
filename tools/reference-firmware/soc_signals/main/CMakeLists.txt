# SPDX-License-Identifier: Apache-2.0
idf_component_register(SRCS "soc_signals_main.c"
                       PRIV_REQUIRES esp_driver_gpio esp_driver_spi esp_driver_i2c esp_adc)
