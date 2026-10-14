/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * A reference firmware of inkemu, written for its tests: the deep sleep of the ESP32-C3 and its
 * wake by the RTC timer, with code this repository can build, run and publish. Each life counts
 * itself in RTC memory (RTC_DATA_ATTR: the bootloader does not load the RTC segments again after
 * a wake from deep sleep, ESP-IDF v5.5.2 components/bootloader_support/src/esp_image_format.c),
 * and prints its reset reason, its wake cause and, after a wake, the milliseconds it slept by the
 * clock ESP-IDF keeps across the sleep (the RTC timer and its calibration). Lives 1 to 3 sleep
 * 2 s on the RTC timer; life 4 sleeps with no wake source, which is how a firmware turns a device
 * off. No other wake source is enabled: the deep_sleep example of ESP-IDF arms a GPIO wake by
 * default on this chip.
 *
 * ESP-IDF enters the sleep through rtc_deep_sleep_start (CONFIG_ESP_SYSTEM_ALLOW_RTC_FAST_MEM_AS_HEAP
 * keeps its default, y), which takes the CRC of the RTC fast memory with the unit of SYSTEM; the
 * ROM checks it at each wake before it calls the wake stub. When the CRC does not match, the ROM
 * skips the stub without a word and boots from flash all the same, and the stub ESP-IDF provides
 * does nothing on this chip; so the app brings its own stub, which counts its runs in RTC memory,
 * and each life prints the count. Life N sees N - 1 only if the CRC matched at each wake and the
 * stub ran from RTC fast memory. The stub follows the rules of ESP-IDF v5.5.2
 * (docs/en/api-guides/deep-sleep-stub.rst): it lives in RTC fast memory (RTC_IRAM_ATTR), calls
 * the default stub first, and touches only RTC memory, since flash is not mapped yet.
 */
#include <inttypes.h>
#include <stdio.h>
#include <sys/time.h>
#include "esp_attr.h"
#include "esp_err.h"
#include "esp_sleep.h"
#include "esp_system.h"

/* The time each of the first three lives sleeps, in microseconds. */
#define SLEEP_US 2000000ULL
/* The lives that sleep on the RTC timer; the next one sleeps with no wake source. */
#define TIMED_LIVES 3u

static RTC_DATA_ATTR uint32_t lives;
/* The time of day, in microseconds, at which the last life went to sleep. */
static RTC_DATA_ATTR int64_t slept_at_us;
/* The times the wake stub below ran: once at each wake from deep sleep. */
static RTC_DATA_ATTR uint32_t stub_runs;

/* Overrides the weak stub of ESP-IDF, which the ROM calls at a wake once the CRC matched. */
void RTC_IRAM_ATTR esp_wake_deep_sleep(void)
{
    esp_default_wake_deep_sleep();
    stub_runs++;
}

static int64_t now_us(void)
{
    struct timeval now;
    gettimeofday(&now, NULL);
    return (int64_t)now.tv_sec * 1000000 + now.tv_usec;
}

static const char *reset_name(esp_reset_reason_t reason)
{
    switch (reason) {
    case ESP_RST_POWERON:
        return "power-on";
    case ESP_RST_DEEPSLEEP:
        return "deep sleep";
    default:
        return "another reason";
    }
}

static const char *wake_name(esp_sleep_wakeup_cause_t cause)
{
    switch (cause) {
    case ESP_SLEEP_WAKEUP_UNDEFINED:
        return "nothing";
    case ESP_SLEEP_WAKEUP_TIMER:
        return "the RTC timer";
    default:
        return "another source";
    }
}

void app_main(void)
{
    int64_t woke_at_us = now_us();
    lives++;
    esp_reset_reason_t reason = esp_reset_reason();
    esp_sleep_wakeup_cause_t cause = esp_sleep_get_wakeup_cause();
    printf("deep_sleep_timer: life %" PRIu32 ", reset by %s (%d), woken by %s (%d)\n", lives,
           reset_name(reason), (int)reason, wake_name(cause), (int)cause);
    printf("deep_sleep_timer: wake stub ran %" PRIu32 " times\n", stub_runs);
    if (cause == ESP_SLEEP_WAKEUP_TIMER) {
        printf("deep_sleep_timer: slept %" PRId64 " ms\n", (woke_at_us - slept_at_us) / 1000);
    }
    if (lives <= TIMED_LIVES) {
        printf("deep_sleep_timer: sleeping %u ms on the RTC timer\n",
               (unsigned)(SLEEP_US / 1000));
        ESP_ERROR_CHECK(esp_sleep_enable_timer_wakeup(SLEEP_US));
    } else {
        printf("deep_sleep_timer: sleeping with no wake source\n");
    }
    slept_at_us = now_us();
    /* Waits for the UARTs to send what was printed, then sleeps; it does not return. */
    esp_deep_sleep_start();
}
