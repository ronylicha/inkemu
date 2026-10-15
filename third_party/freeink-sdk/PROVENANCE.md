# Provenance

| File | Source | Commit | SHA-256 |
|---|---|---|---|
| `LICENSE` | https://github.com/Free-Ink/freeink-sdk/blob/111fdcc7f0176c3ee38391a160ee296bf492dbd8/LICENSE | `111fdcc7f0176c3ee38391a160ee296bf492dbd8` | `17dea5079d9905fbbc8323cde7bfffd0f24185949c43a5daad5177022b796588` |
| `NOTICE` | https://github.com/Free-Ink/freeink-sdk/blob/111fdcc7f0176c3ee38391a160ee296bf492dbd8/NOTICE | `111fdcc7f0176c3ee38391a160ee296bf492dbd8` | `1a45b9f057b415f84085d808bd12ec5df1296808ad14bd9583f2a585c84290ff` |

Both files are unmodified copies. `111fdcc7` is the commit the submodule `freeink-sdk` of
CrossPoint Reader points to at its tag `1.6.5` (`crosspoint-reader/crosspoint-reader` at
`93e98bb78702e29868a16a13b80c40e6b36ccdff`). The two files are byte for byte the same at
`7fcd3b34c108f28c086ade3b433924022dbc0998` (the submodule of CrossPoint's `main` at
`bb855ef935826e86c65574b5abbd4eab974d53e0`) and at `32a265fa86498c014e13a5bc16133bb021cb620c`.

`LICENSE` is FreeInk's MIT licence (Copyright (c) 2026 FreeInk). `NOTICE` reproduces the MIT
licence of the OpenX4 E-Paper Community SDK (Copyright (c) 2025 Open X4 E-Paper Contributors), from
which FreeInk says its SSD1677 initialisation sequences and waveform LUTs derive.

## What this repository copies from FreeInk

One table of data, in test code only:

| Copy | Source | Size and revisions where the source is identical |
|---|---|---|
| `FACTORY_GREY` in `crates/chips/src/ssd1677/tests.rs` | `lut_factory_quality` in `libs/display/FreeInkDisplay/src/lut/Ssd1677Luts.h` | 110 bytes, the same at `111fdcc7`, `7fcd3b34` and `32a265fa` |
| `FACTORY_GREY` in `crates/chips/tests/x4_driver.rs` | the same table | the same |

The tests send it as the X4's firmware does (`0x32`, then `0x03`, `0x04`, `0x2C`), to check that a
LUT written by a firmware drives four grey levels. Each copy names both copyrights and points to
this directory, as the MIT licence asks; the test
`the_copied_lut_keeps_freeink_s_licence_and_notice` in `crates/chips/tests/x4_driver.rs` builds
the two files in, so removing them breaks the tests.

Everything else the crate `chips` takes from FreeInk is a fact about the hardware or the protocol
(command bytes, their order, pins, delays, ADC thresholds, the divider ratio), read in its code and
cited by file and line; no code is copied.

## What the inkemu binary contains

Nothing from FreeInk. The crate `chips` is not a dependency of the binary yet, and the copies above
are in `#[cfg(test)]` code and in an integration test, which no build of the binary compiles. The
reference waveforms the crate builds (`crates/chips/src/ssd1677/reference.rs`) take their voltages
from the SSD1677 datasheet's reset values, not from FreeInk. This is why the root `NOTICE` and
`inkemu licenses` do not list FreeInk, as they do not list the CoreMark sources the tests build. If
a FreeInk table ever enters code that the binary compiles, its notice joins the root `NOTICE` and the
notices `inkemu licenses` prints in the same change.
