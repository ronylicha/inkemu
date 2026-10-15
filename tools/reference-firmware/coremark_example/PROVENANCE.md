# coremark_example: provenance

Every file of this directory but this one is a copy, byte for byte, of the ESP Component Registry
release `espressif/coremark` 1.1.0~2 (published 2025-07-12, component hash
`1f95044d7fe042d997b7715e0dc9753e65c6724158c2d028997f7816df1329d7`, built from
`espressif/idf-extra-components` at commit `746e83ddbea0db9c3d24993a87c4c737a60337ae`, directory
`coremark`), downloaded on 2026-10-15:

- archive `https://components-file.espressif.com/components/espressif/coremark/1.1.0~2/espressif__coremark-v1.1.0_2.zip`,
  SHA-256 `b7ba74c9baafe1b5ab6d8a61886f21ce4183fd19f820f8a8c45e186d00f7b0af`;
- each file checked against the registry's `CHECKSUMS.json` of the release
  (`https://components-file.espressif.com/components/espressif/coremark/1.1.0~2/CHECKSUMS.json`).

| Here | In the release | Licence |
|---|---|---|
| `components/coremark/` | the component: `CMakeLists.txt`, `linker.lf.in`, `idf_component.yml`, `README.md`, `LICENSE`, `port/`, and the EEMBC sources of `coremark/` (`core_*.c`, `coremark.h`, `coremark.md5`, `LICENSE.md`, `README.md`) | EEMBC sources: Apache-2.0, with the CoreMark Acceptable Use Agreement on the trademark (`LICENSE.md`); port layer: Apache-2.0, Espressif |
| `CMakeLists.txt`, `sdkconfig.defaults`, `main/CMakeLists.txt`, `main/coremark_example_main.c` | `examples/coremark_example/` | CC0-1.0 (Espressif) |

Left out: the HTML documentation and the ports for other systems (`coremark/docs`,
`coremark/barebones`, `coremark/posix`, ...), the configurations of other chips
(`sdkconfig.defaults.esp32*`), the example's `README.md`, `pytest_coremark.py` and
`main/idf_component.yml`. The last one asks the component manager for `espressif/coremark: '*'`;
the reference build runs without network and with the component manager off, so the component is
found in `components/` instead, under the name `coremark` the example's `PRIV_REQUIRES` gives.

Nothing is modified. `../SHA256SUMS.coremark-sources` pins every copied file; `in-container.sh`
checks them before the build; `sha256sum` reports every file that differs, and the build stops.
First, it checks that the directory holds those files and this one only, all regular files: a
file added here, which the build could pick up, or a symbolic link, which `sha256sum` would
follow, stops it as a changed file does. The CoreMark run rules ask for the
benchmark's files unmodified (`components/coremark/coremark/README.md`, and the MD5 sums of
`coremark.md5`, which these copies match).

CoreMark is a trademark of EEMBC; EEMBC is a registered trademark of the Embedded Microprocessor
Benchmark Consortium. inkemu is not affiliated with EEMBC. What this firmware prints under inkemu
is the output of an emulated chip, not a result of a device, and is not reported as a CoreMark
score of any hardware.
