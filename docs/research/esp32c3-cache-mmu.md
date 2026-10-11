# ESP32-C3 flash cache and MMU: software-visible behaviour for inkemu SP1b-2

Scope: the exact behaviour of the flash cache controller (`EXTMEM`, `0x600C4000`) and the MMU table (`0x600C5000`) that the mask ROM, the ESP-IDF v5.5.2 2nd-stage bootloader and an ESP-IDF application rely on, so the SP1b-2 plan can list what to implement.
Date of research: 2026-10-11. No code was changed; this file is the only repository edit.

Tags: **MEASURED** = a trace or a run I did (QEMU 9.2.2 `esp_develop_9.2.2_20250817`, inside Docker); **READ** = disassembly or source (cited as `function@address` or `file:line`); **ASSUMED** = inference with no direct evidence.

## 0. Sources and method

| Tag | What | Where |
|---|---|---|
| `ROM3:` | Disassembly of `esp32c3_rev3_rom.elf` (`esp-rom-elfs` 20260528). I compared the cache functions listed in section 5 across rev0, rev3 and rev101: **instruction-for-instruction identical** (same mnemonics, only call targets differ). The internal function table at `0x3ff1e434` (rev3), `0x3ff1e410` (rev0), `0x3ff1e72c` (rev101) holds the same 15 functions in the same order in all three. | `scratchpad/boot/romelf/dis_rev{0,3,101}.txt` |
| `IDF:` | ESP-IDF v5.5.2 sources from the `inkemu/esp-idf` image, `/opt/esp/idf` | `components/hal/esp32c3`, `components/soc/esp32c3`, `components/esp_rom/esp32c3`, `components/esp_mm`, `components/bootloader_support`, `components/esp_system/port/soc/esp32c3` |
| `SVD:` | `third_party/espressif-svd/esp32c3.svd` (reset values, field names, descriptions) | repo |
| `TRM:` | ESP32-C3 Technical Reference Manual v1.4, downloaded to `scratchpad/cm/dl/trm.pdf` (never in the repo). Its only cache/MMU content is section 3.3.3 (address mapping, geometry, operations). **EXTMEM and the MMU table are not documented in the TRM** (`0x600C_4000-0x600C_BFFF` is listed "Reserved"). | |
| `QEMU:` | Espressif QEMU fork `esp32c3_cache.c/.h` (GPL), read for facts only, nothing copied. Behaviour confirmed by running the binary shipped in the image. | `scratchpad/boot/qemu/` |
| MEASURED traces | `-trace "memory_region_ops_*"` on the hello_world image (`oracle/flash_hello.bin`) and on a 16 MiB image with the real CrossPoint 1.6.5 app at `0x10000` (CrossPoint partition table, hello_world bootloader). Cache-only extracts and the probe programs are saved in `scratchpad/cm/art/` (`hello_cache_trace.txt` 2,460 events, `cp_cache.txt` 13,292 events, `t_probe1.S`, `t2.S`). | |

Probe method for sections 6: two bare-metal RISC-V programs (assembled with `riscv32-esp-elf-gcc`, loaded at `0x40380000` through `-device loader,cpu-num=0`, own trap handler) that exercise the cache windows on QEMU's `esp32c3` machine. They test QEMU, not silicon.

### 0.1 Corrections to earlier reports

These supersede the matching lines of `esp32c3-boot-chain.md`.

| Where | Earlier statement | Correct |
|---|---|---|
| §J row 15 and §C.1 (`0x040` row) | `ROM_Boot_Cache_Init` does RMW on `EXTMEM.AUTOLOAD_CTRL 0x600C4040` (b0, b1, clear b1) | The base register in `ROM3:ROM_Boot_Cache_Init@0x40050ee6` is `lui a5,393408` = **`0x600C0000`**, so it is **`SYSTEM.CACHE_CONTROL` (`0x600C0040`)**: `\|=1` (ICACHE_CLK_ON), `\|=2` (ICACHE_RESET), `&=~2`. MEASURED: `read/write 0x600c0040 = 0,1 / 0,2 / 0,0`. The EXTMEM `0x40` register is only polled (bit 3), never written, on the boot path. |
| §J row 15 | order of the init | Real order (READ + MEASURED, section 5.2): SYSTEM.CACHE_CONTROL x3, `CACHE_MMU_OWNER \|= 0xF`, 128 MMU writes, `ILG_INT_CLR = 0xFFFFFFFF`, `WRAP_AROUND &= ~1`, `SENSITIVE+0x1C &= ~1`, invalidate-all, `ICACHE_CTRL \|= 1`, `ICACHE_CTRL1 &= ~2`. |
| §J rows 17 and 23 | `Cache_Suspend_ICache` = "RMW CTRL1 \|=3, CTRL &=~1, poll `+0x34 & 2`" | Incomplete. It also reads/polls `AUTOLOAD_CTRL` (bit 3 DONE, unconditional) and does a **freeze enable + freeze disable** pair on `ICACHE_FREEZE (0xCC)`, each with a DONE poll (section 5.1). |
| §C.1 `0x0CC` row | "set DONE when ENA set; clear DONE when ENA cleared" | Correct, and it is **required**: `Cache_Suspend_ICache` spins on it. |

## 1. MMU table (`0x600C5000`)

| Item | Fact | Tag / source |
|---|---|---|
| Base, size | `0x600C5000`, **128 entries x 4 bytes = 0x200 bytes** (`0x600C5000-0x600C51FF`) | READ `IDF:reg_base.h DR_REG_MMU_TABLE`, `ext_mem_defs.h SOC_MMU_ENTRY_NUM 128`, `CACHE_IBUS_MMU_END 0x200` |
| Page size | **64 KiB, fixed** (`mmu_ll_get_page_size` returns `MMU_PAGE_64KB` unconditionally; ROM `Cache_*_MMU_Set` returns error 3 for `psize != 64`) | READ `mmu_ll.h`, `ROM3:Cache_Dbus_MMU_Set@0x40051756` |
| Entry bits | `[7:0]` flash page number (`SOC_MMU_VALID_VAL_MASK 0xFF`, 256 pages = **16 MiB**, `SOC_MMU_MAX_PADDR_PAGE_NUM 256`); **bit 8 = invalid** (`SOC_MMU_INVALID = BIT(8)`); `SOC_MMU_VALID = 0`; `SOC_MMU_ACCESS_FLASH = 0`, `SOC_MMU_TYPE = 0` | READ `ext_mem_defs.h` |
| Software writes | `mmu_ll_write_entry` stores `page \| 0 \| 0`; `mmu_ll_set_entry_invalid` stores exactly `0x100`; `Cache_MMU_Init` stores `0x100` into all 128 words (loop of `sw 256`) | READ `mmu_ll.h`, `ROM3:Cache_MMU_Init@0x40051670` |
| Width of storage | `0x1FF` kept after writing `0xFFFFFFFF` in QEMU (reserved bits forced to 0). Real width unknown: `CACHE_MMU_FAULT_CONTENT` is a **10-bit** field (`SVD` `CACHE_MMU_FAULT_CONTENT[9:0]`), so the hardware entry may be 10 bits wide with bit 9 unused. | MEASURED (QEMU), ASSUMED (hardware) |
| Translation | `entry_id = (vaddr & 0x7FFFFF) >> 16`; `paddr = (entry & 0xFF) << 16 \| (vaddr & 0xFFFF)` | READ `mmu_ll_get_entry_id`, `mmu_ll_entry_id_to_paddr_base` |
| Virtual windows | Data bus `0x3C000000-0x3C7FFFFF` (8 MiB, read-only), instruction bus `0x42000000-0x427FFFFF` (8 MiB). `EXTMEM.IBUS_TO_FLASH_START/END_VADDR` reset to `0x42000000/0x427FFFFF`, DBUS `0x3C000000/0x3C7FFFFF`. | READ `TRM` §3.3.3.1 table 3.3-2; `SVD` reset values |
| One table or two? | **One table shared by both buses**: "the instruction bus address space (8 MB) and the data bus address space (8 MB) is always shared". Entry `i` maps `0x3C000000 + i*64K` **and** `0x42000000 + i*64K` to the same flash page. The IROM/DROM split is a software convention only (IDF puts IROM in the low entries, DROM above it; `Cache_Get_IROM_MMU_End` etc. are ROM RAM variables). | READ `TRM` §3.3.3.1, `mmu_ll.h`; MEASURED (QEMU probe: `lw` at `0x3C030000` and at `0x42030000` returned the same word `0x4F0206E9` with one entry written) |
| Reads of the table | A read returns the last written value (RAM-like). IDF reads it (`mmu_ll_read_entry`, `check_entry_valid`, `find_entry_id_based_on_map_value`), so the table **must be readable and coherent**. The ROM reads it in `Cache_Count_Flash_Pages`. | READ `mmu_ll.h`, `ROM3:Cache_Count_Flash_Pages@0x40051826` |
| Reset content | The table is SRAM-like: contents at power-on are not documented. The ROM always re-initialises it (`ROM_Boot_Cache_Init` and again at the end of `ets_run_flash_bootloader`), so only the post-ROM state matters: **all 128 entries `0x100`**. QEMU resets to all-invalid. After an `esp_restart` the table keeps its old contents until the ROM re-inits (the app calls `Cache_Disable_ICache` first, `esp_restart_noos`). | READ + MEASURED (first 128 writes `0x100` at the start of both traces); ASSUMED (power-on) |
| Access control | `SENSITIVE.CACHE_MMU_ACCESS_1 (0x600C1034)` bits `PRO_MMU_RD_ACS/WR_ACS`, reset `3` = allowed. No boot path writes it. | READ `sensitive_reg.h`, `SVD` |
| Beyond 128 entries | `0x600C5200-0x600C5FFF`: nothing documented. The ROM reads/writes `0x600C6000` as **ICache tag memory** (`ROM3:Cache_Travel_Tag_Memory@0x40051886`, base `lui s7,393414`), used only by `Cache_Travel_Tag_Memory`, unused by any boot path or by IDF. | READ |

### 1.1 What the bootloader and ROM write: real values

**ROM loader** (`ROM3:ets_loader_map_range@0x400499f6`): state words `{base, size}`; if the wanted range is not inside the current window it sets `base = offset & ~0xFFFF`, `size` rounded up to 64 KiB (max 1 MiB asserted), then `Cache_Suspend_ICache; Cache_Invalidate_ICache_All; Cache_Dbus_MMU_Set(0, 0x3C000000, base, 64, size>>16, 0); Cache_Resume_ICache` and returns `0x3C000000 + (offset-base)`. MEASURED first write: `write 0x600c5000 = 0x0` (flash page 0 for the image header at flash offset 0), later `0x1`, `0x2`, `0x3` as the ROM walks the bootloader image.

**hello_world** (`oracle/r1`, bootloader at 0, app at `0x10000`; the QEMU golden log gives the segments). Final table after `set_cache_and_start_app`, MEASURED by `xp /128wx 0x600c5000` three seconds after boot:

| Entry | Value | Meaning |
|---|---|---|
| 0 | `0x00000002` | IROM seg `vaddr=0x42000020 paddr=0x00020020` -> page 2 |
| 1 | `0x00000001` | DROM seg `vaddr=0x3c010020 paddr=0x00010020` -> page 1 |
| 2-126 | `0x00000100` | invalid |
| 127 | `0x00000001` | the bootloader maps the first DROM page again at `0x3C7F0000` so the app can find the boot partition (`set_cache_and_start_app`, "`MMU_DROM_END_ENTRY`") |

Write order seen in the trace (MEASURED): `unmap_all` (128x `0x100`), entry 1 = 1, entry 127 = 1, entry 0 = 2.

**CrossPoint 1.6.5 `x3-x4` app, `app0` at `0x10000`** (MEASURED on QEMU, matches my computation from the image header, which I verified by parsing `fw/crosspoint-1.6.5-x3-x4.bin`; the earlier `x3-crosspoint.bin` used DROM at `0x3C180020`, this release at `0x3C210020`):

| Entries | Value | Segment |
|---|---|---|
| 0..32 (33 pages) | `0x35..0x55` | IROM `vaddr=0x42000020`, `paddr=0x350020`, `size=0x20A5EC` |
| 33..84 (52 pages) | `0x01..0x34` | DROM `vaddr=0x3C210020`, `paddr=0x10020`, `size=0x334E50` |
| 85..126 | `0x100` | invalid |
| 127 | `0x01` | DROM-end page |

If the same image is the OTA slot `app1` at `0x650000` the values become IROM `0x99..0xB9`, DROM `0x65..0x98`, entry 127 = `0x65` (computed, ASSUMED same algorithm; not run).

During the bootloader, before the final mapping, MEASURED sequences on the CrossPoint run: entries `0..51` = `0x01..0x34` (image verification maps the DROM segment at `0x3C000000`), entry 127 = a single page for `bootloader_flash_read`; every `bootloader_mmap`/`bootloader_flash_read` is `Cache_Disable_ICache` + MMU write + `Cache_Enable_ICache` (about 500 times in that boot).

### 1.2 App-side remap (MEASURED, CrossPoint boot, 2 occurrences)

`esp_mmu_map` / `s_do_mapping` on C3 (`SOC_CACHE_FREEZE_SUPPORTED 1`, `IDF:esp_mmu_map.c:377,440`): freeze, write MMU, enable buses, invalidate the range, unfreeze. The exact register trace of one remap of flash page 0 at `vaddr 0x3C550000` (entry 85), event numbers from `cp_cache.txt`:

```
2977 R CTRL(0x00)=0   W CTRL=1                       -- Cache_Enable_ICache (end of the previous op)
2979 R CTRL1=0        W CTRL1=0                       -- cache_ll_l1_enable_bus (ibus mask cleared)
2981 R FREEZE(0xCC)   W 0 ; R ; W 1 ; R =4            -- Cache_Freeze_ICache_Enable(ACK_BUSY): DONE seen
2986 W MMU[85] = 0x00000000                          -- flash page 0
2987 R CTRL1 / W CTRL1 (twice)                       -- enable_bus for the new range
2991 W SYNC_ADDR = 0x3c550000
2993 W SYNC_SIZE = 0x800                              -- 64 KiB / 32-byte lines (Cache_Invalidate_Addr)
2995 R SYNC_CTRL=2 ; W SYNC_CTRL=3 ; R =2            -- invalidate, poll SYNC_DONE
2997 R FREEZE=4 ; W ; R ; W 0 ; ... ; W MMU[85]=0x100 -- unfreeze, later unmap, same sequence again
```

## 2. EXTMEM registers (`0x600C4000`)

Reset values are `SVD:` unless "none" (the SVD gives none; QEMU resets them to 0). "Used by": R = ROM boot path, B = IDF 2nd-stage bootloader, A = app (all MEASURED in the traces above unless marked READ only). Disposition: **S** plain storage, **C** command/done logic, **K** constant/status, **E** real effect, **X** not touched by any measured path (storage is enough).

| Offset | Register | Reset | Bits | Used by | Behaviour required | Disp. |
|---|---|---|---|---|---|---|
| 0x000 | `ICACHE_CTRL` | none (0) | b0 `ENABLE` | R B A | Plain R/W. `Cache_Enable_ICache` sets, `Cache_Suspend`/`Disable` clear, `Cache_Resume` sets. Gates the windows on real silicon (section 6). QEMU stores it in a field but ignores it for data. | S, E (optional) |
| 0x004 | `ICACHE_CTRL1` | **3** | b0 `SHUT_IBUS`, b1 `SHUT_DBUS` (1 = bus disabled) | R B A | Plain R/W with reset 3 (the repo already does this). QEMU instead stores the value but reads **0 always**; do not copy that. Gates fetch (b0) and load (b1) on silicon. | S, E (optional) |
| 0x008 | `ICACHE_TAG_POWER_CTRL` | 5 | b0 FORCE_ON, b1 FORCE_PD, b2 FORCE_PU | A | App `rtc_init.c:81` clears b0. Plain R/W. | S |
| 0x00C-0x018 | `ICACHE_PRELOCK_*` | none | prelock enable/addr/size | - | X | S |
| 0x01C | `ICACHE_LOCK_CTRL` | **4** | b0 LOCK_ENA, b1 UNLOCK_ENA (both self-clear), b2 `LOCK_DONE` (RO) | READ only (`Cache_Lock/Unlock_ICache_Items` poll b2) | Command: on write with b0 or b1 set -> clear them, keep DONE=1. | C |
| 0x020, 0x024 | `ICACHE_LOCK_ADDR/SIZE` | none | | - | X | S |
| 0x028 | `ICACHE_SYNC_CTRL` | **1** (see Q5) | b0 `INVALIDATE_ENA` (RW, **cleared by hardware when done**), b1 `SYNC_DONE` (RO) | R B A | Command: on write with b0=1 -> invalidate (no-op) -> clear b0, set b1. Software polls b1 after setting b0. | C |
| 0x02C | `ICACHE_SYNC_ADDR` | none | vaddr | R B A | Plain R/W. MEASURED values: `0`, `0x3c550000`. | S |
| 0x030 | `ICACHE_SYNC_SIZE` | none | b22:0 = number of 32-byte lines; **0 = whole cache** (ROM `Cache_Invalidate_ICache_All` = items(0,0)) | R B A | Plain R/W (23 bits). MEASURED `0x800` for 64 KiB. | S |
| 0x034 | `ICACHE_PRELOAD_CTRL` | **2** | b0 ENA (self-clear), b1 `DONE` (RO), b2 ORDER | R B A | **DONE must read 1**: `Cache_Suspend_ICache`, `Cache_Disable_ICache` spin on `(reg & 2)`. Command semantics as 0x01C. | C |
| 0x038, 0x03C | `ICACHE_PRELOAD_ADDR/SIZE` | none | | - | X | S |
| 0x040 | `ICACHE_AUTOLOAD_CTRL` | **8** | b0 SCT0_ENA, b1 SCT1_ENA, b2 `ENA` (RW, persistent), b3 `DONE` (RO), b4 ORDER, b6:5 RQST | R B A | **DONE must read 1** (`Cache_Suspend_ICache_Autoload` polls b3 unconditionally, MEASURED 1,024 reads in the CrossPoint trace). `ENA` is a stored configuration bit, not a command (IDF never sets it: `cache_hal_init` reads it as 0, so `Cache_Enable_ICache(0)`). QEMU wrongly clears `ENA` on every read. Model: DONE=1 always; other bits stored. | S, K |
| 0x044-0x050 | `ICACHE_AUTOLOAD_SCT*` | none | | - | X | S |
| 0x054-0x060 | `IBUS/DBUS_TO_FLASH_START/END_VADDR` | `0x42000000, 0x427FFFFF, 0x3C000000, 0x3C7FFFFF` | | - | Plain R/W with those reset values. | S |
| 0x064 | `CACHE_ACS_CNT_CLR` | none | b0/b1 clear counters (WO) | - | X; accept writes. | S |
| 0x068-0x074 | `IBUS_ACS_MISS_CNT`, `IBUS_ACS_CNT`, `DBUS_ACS_FLASH_MISS_CNT`, `DBUS_ACS_CNT` | none | 32-bit counters | - | X. Reading 0 is fine; counting accesses is optional (feeds a later miss-cost model). | S |
| 0x078 | `CACHE_ILG_INT_ENA` | none | b0 SYNC_OP_FAULT, b1 PRELOAD_OP_FAULT, b5 MMU_ENTRY_FAULT, b7 IBUS_CNT_OVF, b8 DBUS_CNT_OVF | A | App writes `0x23` (`esp_cache_err_int_init`). Plain R/W. | S |
| 0x07C | `CACHE_ILG_INT_CLR` | none | WO, same bits | R A | ROM writes `0xFFFFFFFF` (`Cache_Set_Default_Mode`), app `0x23`. Write-only, clears `ILG_INT_ST`. | S |
| 0x080 | `CACHE_ILG_INT_ST` | none | b0, b1, b5 (`MMU_ENTRY_FAULT_ST`), b7..b10 | A (panic only) | RO status = latched events, gated by `ILG_INT_ENA` for the interrupt line (source 36). Never set in a healthy boot. | S, E (optional) |
| 0x084 | `CORE0_ACS_CACHE_INT_ENA` | none | b0 IBUS_ACS_MSK, b1 IBUS_WR, b2 IBUS_REJECT, b3 DBUS_ACS_MSK, b4 DBUS_REJECT, b5 DBUS_WR | A | App writes `0x3F`. Plain R/W. | S |
| 0x088 | `CORE0_ACS_CACHE_INT_CLR` | none | WO | A | App writes `0x3F`. | S |
| 0x08C | `CORE0_ACS_CACHE_INT_ST` | none | same six bits | A (panic only) | RO status, interrupt source 61. Never set in a healthy boot. | S, E (optional) |
| 0x090-0x09C | `CORE0_D/IBUS_REJECT_ST/VADDR` | `VADDR = 0xFFFFFFFF` | PMS reject records | - | X | S |
| 0x0A0, 0x0A4 | `CACHE_MMU_FAULT_CONTENT/VADDR` | none | content[9:0], code[13:10] | - | RO record of the last MMU fault. X in healthy boot. | S |
| 0x0A8 | `CACHE_WRAP_AROUND_CTRL` | none | b0 | R B | ROM clears b0 (RMW) in `Cache_Set_Default_Mode`. Plain R/W. | S |
| 0x0AC | `CACHE_MMU_POWER_CTRL` | 5 | b0 FORCE_ON, b1 FORCE_PD, b2 FORCE_PU | A | App clears b0 (`rtc_init.c:79`). Plain R/W. | S |
| 0x0B0 | `CACHE_STATE` | **1** | b11:0 `ICACHE_STATE`, 1 = idle | B A | RO. **Must read exactly `1`**: `Cache_Disable_ICache` spins until `(reg & 0xFFF) == 1` (MEASURED 113 reads in hello_world). | K |
| 0x0B4, 0x0B8 | `CACHE_ENCRYPT_DECRYPT_*` | none, 7 | | - | X (flash-encryption plumbing) | S |
| 0x0BC, 0x0C0 | `CACHE_PRELOAD_INT_CTRL`, `CACHE_SYNC_INT_CTRL` | none | ST (RO) / ENA / CLR (WO) | - | X; interrupt sources 41, 42 never needed by the boot. | S |
| 0x0C4 | `CACHE_MMU_OWNER` | none | b3:0 ("bit0/bit2 ibus, bit1/bit3 dbus") | R B | `Cache_Owner_Init` does `\|= 0xF` (MEASURED read 0, write 0xF, twice). Plain R/W. No observable effect. | S |
| 0x0C8 | `CACHE_CONF_MISC` | 7 | b0/b1 ignore MMU fault in preload/sync, b2 trace | - | X | S |
| 0x0CC | `ICACHE_FREEZE` | none | b0 `ENA` (RW), b1 `MODE` (RW, 0 = busy, 1 = error), b2 `DONE` (RO) | R B A | **`DONE` mirrors `ENA`** immediately. MEASURED about 3,200 accesses over the two boots of the CrossPoint trace; `Cache_Suspend_ICache` and the app remap path poll it. Software writing b2 must be ignored. | C |
| 0x0D0 | `ICACHE_ATOMIC_OPERATE_ENA` | 1 | | - | X | S |
| 0x0D4 | `CACHE_REQUEST` | none | | - | X | S |
| 0x0D8-0x0FC | `IBUS_PMS_TBL_*`, `DBUS_PMS_TBL_*` | boundaries `0x800`, attr `0xFF`/`0xF` | PMS | - | `Cache_Ibus/Dbus_Pms_*` (READ) only; not touched by hello_world or CrossPoint boots. Plain R/W. | S |
| 0x100 | `CLOCK_GATE` | 1 | b0 | - | Plain R/W. | S |
| 0x3FC | `REG_DATE` | `0x02007160` | | - | RO constant. | K |

Registers touched in the measured boots (union): `0x000 0x004 0x008 0x028 0x02C 0x030 0x034 0x040 0x078 0x07C 0x080 0x084 0x088 0x08C 0x0A8 0x0AC 0x0B0 0x0C4 0x0CC` (hello_world: 17 offsets; CrossPoint adds only one read each of `0x080` and `0x08C`). Everything else is X.

### 2.1 Registers outside EXTMEM that the cache path needs

| Address | Register | Used as | Behaviour | Tag |
|---|---|---|---|---|
| `0x600C0040` | `SYSTEM.CACHE_CONTROL`, reset 5 | `ROM_Boot_Cache_Init`: `\|=1`, `\|=2`, `&=~2` (icache clock on, reset pulse) | Plain R/W (not yet in `system.rs`). | READ + MEASURED |
| `0x600C101C` | `SENSITIVE.INTERNAL_SRAM_USAGE_1`, reset `0xF` | b0 = "SRAM0 stays plain IRAM". `Cache_Occupy_ICache_MEMORY(1)` clears b0 (cache takes the 16 KiB at `0x4037C000`), `(0)` sets it. `Cache_Get_Memory_BaseAddr` returns `0x4037C000` when b0 = 0, else 0. | Plain R/W (SENSITIVE is not modelled yet). No boot path reads SRAM0 as RAM after this. | READ (`ROM3:Cache_Occupy_ICache_MEMORY@0x40051650`) + MEASURED (R 0, W 0) |
| `0x600C1030`, `0x600C1034` | `SENSITIVE.CACHE_MMU_ACCESS_0/1`, reset none/3 | MMU table access permission | Plain R/W, not enforced. | SVD |
| `0x600C2090`, `0x600C20A4`, `0x600C20A8`, `0x600C20F4` | `INTERRUPT_CORE0` maps for sources 36 `CACHE_IA`, 41 `ICACHE_PRELOAD`, 42 `ICACHE_SYNC`, 61 `CACHE_CORE0_ACS` | cache error interrupts (app routes 36 and 61 to CPU interrupt 25) | Source lines stay low in a healthy boot. | READ `IDF:interrupts.h`, `SVD` |
| `0x600C6000` | ICache tag memory | `Cache_Travel_Tag_Memory` only | Not needed. | READ |
| eFuse `DIS_ICACHE`, `DIS_DOWNLOAD_ICACHE` | | disable the cache in hardware | Default 0; not read by software. | TRM table 4.3-1 |

## 3. Cache geometry and operations the emulator can ignore

| Fact | Value | Tag |
|---|---|---|
| Cache | one shared I/D cache, **16 KiB, 8-way, 32-byte lines** (`Cache_Get_Mode` returns size `0x4000`, line 32 via table[0], ways 8: `ROM3:Cache_Get_Mode@0x40050e52`; `TRM` "eight-way set-associative, 16 KB, 32 bytes") | READ |
| Read-only | "read-only uniform cache"; the data bus window is read-only | TRM §3.3.3.2 |
| Backing store | when the cache is on, internal SRAM0 (`0x4037C000-0x4037FFFF`) is occupied by the cache | TRM §3.3.3.2, `ROM3:Cache_Occupy_ICache_MEMORY` |
| Operations | invalidate (auto/manual), preload (manual/auto), lock/unlock, freeze | TRM §3.3.3.3 |
| Stale data | The cache is read-only, so flash writes through SPI1 are only visible through the window after `Cache_Invalidate_Addr` (`spi_flash_check_and_flush_cache`, `IDF:flash_mmap.c:306-321`). A model that reads the flash image live shows new data immediately: this only differs from silicon for software that forgets to invalidate. | READ |

## 4. ROM cache API as called: exact register sequences

All from `ROM3` disassembly; rev0 and rev101 are identical (section 0). The shared `rom_cache_internal_table_ptr` (RAM `0x3fcdffd4`) points to the 15-entry table below; most functions reach each other through it. Registers are `EXTMEM+offset` unless written otherwise.

| Table idx | Function | Address |
|---|---|---|
| 0 | `Cache_Get_ICache_Line_Size` (returns 32) | `0x40050e4c` |
| 1, 2 | `Cache_Address_Through_IBus/DBus` (`vaddr - base < 8 MiB`) | `0x40050e7c`, `0x40050e8c` |
| 3 | `Cache_Set_Default_Mode` | `0x40050e9c` |
| 4 | `Cache_Owner_Init` | `0x4005163e` |
| 5 | `Cache_Occupy_ICache_MEMORY` | `0x40051650` |
| 6 | `Cache_Get_Memory_BaseAddr` | `0x400519a6` |
| 7 | `Cache_Invalidate_ICache_Items` | `0x40050f28` |
| 8, 9 | `Cache_Lock/Unlock_ICache_Items` | `0x40051008`, `0x40051030` |
| 10, 11 | `Cache_Suspend/Resume_ICache_Autoload` | `0x40050fe8`, `0x400510a0` |
| 12, 13 | `Cache_Freeze_ICache_Enable/Disable` | `0x40051058`, `0x40051088` |
| 14 | `Cache_Op_Addr` | `0x40050f56` |

### 4.1 Primitives

| Function (addr) | Register sequence |
|---|---|
| `Cache_Mask_All@0x40051138` | `CTRL1 \|= 3` |
| `Cache_UnMask_Dram0@0x40051146` | `CTRL1 &= ~2` |
| `Cache_Enable_ICache(a)@0x40051436` | `CTRL \|= 1`; then `Cache_Resume_ICache_Autoload(a)` |
| `Cache_Resume_ICache_Autoload(a)@0x400510a0` | if `a & 4`: `AUTOLOAD_CTRL \|= 4`; else nothing |
| `Cache_Suspend_ICache_Autoload@0x40050fe8` | read `AUTOLOAD_CTRL`; if b2: clear b2 (write), return 4, else return 0; then **spin until `AUTOLOAD_CTRL & 8`** |
| `Cache_Freeze_ICache_Enable(m)@0x40051058` | read `FREEZE`; set/clear b1 per `m` (write); read, `\|= 1` (write); **spin until `FREEZE & 4`** |
| `Cache_Freeze_ICache_Disable@0x40051088` | read, `&= ~1` (write); **spin until `(FREEZE & 4) == 0`** |
| `Cache_Invalidate_ICache_Items(addr, n)@0x40050f28` | `SYNC_ADDR = addr`; `SYNC_SIZE = (SYNC_SIZE & ~0x7FFFFF) \| (n & 0x7FFFFF)`; `SYNC_CTRL \|= 1`; **spin until `SYNC_CTRL & 2`** |
| `Cache_Invalidate_ICache_All@0x40051126` | `Cache_Invalidate_ICache_Items(0, 0)` through table[7] |
| `Cache_Invalidate_Addr(addr, size)@0x400510b2` | if `addr` and `addr+size-1` are both in the D window or both in the I window: `Cache_Op_Addr(addr, size, 32, 0x400000, Invalidate_Items)`, return 0; else return 1. `Cache_Op_Addr` aligns to 32-byte lines, calls the optional callbacks at `rom_cache_op_cb` (null in practice) and issues `Invalidate_Items(addr, lines)` per chunk. MEASURED: `(0x3c550000, 0x10000)` -> `SYNC_ADDR=0x3c550000`, `SYNC_SIZE=0x800`. |
| `Cache_Suspend_ICache@0x40051446` | **1.** `CTRL1 \|= 3`; **2.** `CTRL &= ~1`; **3.** `Cache_Suspend_ICache_Autoload` (spin on `AUTOLOAD_CTRL & 8`); **4.** spin until `PRELOAD_CTRL & 2`; **5.** `Cache_Freeze_ICache_Enable(0)`; **6.** `Cache_Freeze_ICache_Disable`; returns the autoload status (0 or 4). MEASURED order identical. |
| `Cache_Resume_ICache(a)@0x40051496` | `CTRL \|= 1`; `CTRL1 &= ~3`; `Cache_Resume_ICache_Autoload(a)` |
| `Cache_Disable_ICache@0x400513e8` | `CTRL &= ~1`; `Cache_Suspend_ICache_Autoload`; spin until `PRELOAD_CTRL & 2`; `Cache_Invalidate_ICache_All`; **spin until `(CACHE_STATE & 0xFFF) == 1`**. It does **not** touch `CTRL1` or the freeze register. Used by the bootloader (`cache_hal_disable`) and by `esp_restart_noos`. MEASURED pattern (49 times in hello_world): `R/W CTRL`, `R AUTOLOAD x2`, `R PRELOAD`, `W SYNC_ADDR=0`, `RMW SYNC_SIZE`, `R/W/R SYNC_CTRL`, `R CACHE_STATE`. |
| `Cache_Owner_Init@0x4005163e` | `CACHE_MMU_OWNER \|= 0xF` |
| `Cache_Occupy_ICache_MEMORY(a)@0x40051650` | a = 1: `SENSITIVE+0x1C &= ~1`; a = 0: `\|= 1`; else nothing |
| `Cache_MMU_Init@0x40051670` | 128 x `sw 0x100` to `0x600C5000..0x600C51FC` |
| `Cache_Set_Default_Mode@0x40050e9c` | `ILG_INT_CLR = 0xFFFFFFFF`; `WRAP_AROUND &= ~1`; `Cache_Occupy_ICache_MEMORY(1)`; `Cache_Invalidate_ICache_All` |
| `ROM_Boot_Cache_Init@0x40050ee6` | `SYSTEM.CACHE_CONTROL \|=1, \|=2, &=~2`; `Cache_Owner_Init`; `Cache_MMU_Init`; `Cache_Set_Default_Mode`; `Cache_Enable_ICache(0)`; `Cache_UnMask_Dram0` (so afterwards `CTRL=1`, `CTRL1=1`: ibus still shut, dbus open) |
| `Cache_Enable_Defalut_ICache_Mode@0x40050ec8` | occupy(1), invalidate-all, `Cache_Enable_ICache(0)` (not on the boot path) |
| `Cache_Lock/Unlock_ICache_Items(addr, n)@0x40051008/0x40051030` | `LOCK_ADDR = addr`; `LOCK_SIZE[15:0] = n`; `LOCK_CTRL \|= 1` (lock) or `\|= 2` (unlock); spin until `LOCK_CTRL & 4` |
| `Cache_Start_ICache_Preload(addr, size, order)@0x40051152` | `Suspend_Autoload`; `PRELOAD_ADDR = addr`; set/clear `PRELOAD_CTRL` b2 (order); `PRELOAD_SIZE[15:0] = size/32`; `PRELOAD_CTRL \|= 1`. `Cache_ICache_Preload_Done` returns `PRELOAD_CTRL & 2`. (READ only, not used by any measured boot.) |
| `Cache_Config_ICache_Autoload`, `Cache_Enable/Disable_ICache_Autoload`, `Cache_Enable/Disable_ICache_PreLock`, `Cache_Ibus/Dbus_Pms_*`, `Cache_Pms_Lock` | RMW on `AUTOLOAD_*`, `PRELOCK_*`, `IBUS/DBUS_PMS_TBL_*` (`0xD8`-`0xFC`). READ only, none measured. |
| `Cache_Get_Memory_value(addr)@0x400519f6` | temporarily sets `SENSITIVE+0x1C` b0, reads a word, restores b0. Debug only. |

### 4.2 MMU setters

`Cache_Ibus_MMU_Set@0x40051686` and `Cache_Dbus_MMU_Set@0x40051756` are the same code with different window bases (I: `0x42000000`, D: `0x3C000000`). Signature `(ext, vaddr, paddr, psize, num, fixed)`; returns:

| Return | Condition |
|---|---|
| 3 | `psize != 64` |
| 2 | `(vaddr \| paddr) & 0xFFFF != 0` |
| 4 | the chunk's `vaddr - window_base >= 0x800000` (checked per chunk **before** it writes, so a request that crosses the end of the 8 MiB window writes the in-window part and then returns 4) |
| 0 | done |

Entry written = `((paddr >> 16) + i) \| ext` for page `i` (`fixed == 0`) or `(paddr >> 16) \| ext` for all pages (`fixed != 0`); address `0x600C5000 + ((vaddr >> 16) & 0x7F) * 4`. `ext` is OR-ed in raw: passing `0x100` writes invalid entries. Callers (ROM loader) suspend the cache around it; the function does not.

## 5. Register sequences of the boot, in order (MEASURED, QEMU, cache-only)

### 5.1 ROM, from `Cache_Mask_All` to the first byte read through the window

```
R/W CTRL1 |=3                              boot_prepare (Cache_Mask_All)
W SYSTEM+0x40 =1, =2, =0                   ROM_Boot_Cache_Init (RMW)
R 0xC4 -> W 0xC4 = 0xF                     Cache_Owner_Init
W MMU[0..127] = 0x100                      Cache_MMU_Init
W 0x7C = 0xFFFFFFFF ; R/W 0xA8 &= ~1      Cache_Set_Default_Mode
R/W SENSITIVE+0x1C &= ~1                   Cache_Occupy_ICache_MEMORY(1)
W 0x2C = 0 ; RMW 0x30 ; R 0x28 ; W 0x28 = 1 ; R 0x28 -> 0x2   Invalidate_All (poll sees DONE)
R CTRL ; W CTRL = 1                        Cache_Enable_ICache(0)
R CTRL1 ; W CTRL1 = 0 (reads 3: writes 1 on silicon)   Cache_UnMask_Dram0
--- later, in ets_loader_map_range ---
Cache_Suspend_ICache (6 steps, 4.1) ; Invalidate_All ; W MMU[0]=n ; Cache_Resume_ICache
```

### 5.2 hello_world footprint (10 s of QEMU time, 2,460 events)

`R/W 0x000` ~100 times each, `R 0x028` 105 (poll), `R 0x040` 118, `R 0x034` 55, `R 0x0B0` 49, `R 0x0CC` 30 (freeze pairs: 6 in hello), `R 0x004` 27, plus one-off `0x008 0x078 0x07C 0x084 0x088 0x0A8 0x0AC 0x0C4` (app init, section 2). Every poll finished on the first read in QEMU.

## 6. Invalid entry, disabled bus: hardware vs QEMU

### 6.1 Real hardware (what the sources establish)

| Case | Behaviour | Tag / source |
|---|---|---|
| Cached window accessed while the cache or the matching bus is disabled (`CTRL.ENABLE=0` or `CTRL1.SHUT_x=1`) | The cache raises the **`CORE0_ACS_CACHE_INT`** interrupt: `IBUS_ACS_MSK_ICACHE_ST` (b0) "Cached memory region accessed while ibus or cache is disabled", `DBUS_ACS_MSK_ICACHE_ST` (b3) "...dbus or cache is disabled". The IDF comment says the point is to panic "which is better than grabbing bad data from the bus", so the access itself **returns bad data**. | READ `IDF:cache_err_int.c` |
| Instruction fetch in that state | "Cache errors when reading instructions will result in an **illegal instruction**, before any cache error interrupts trigger": the CPU takes `mcause` **1 (instruction access fault)**, **2 (illegal instruction)** or, for loads, **5 (load access fault)** first; the panic handler then rewrites the cause to the cache error (`mcause = 25`, `ETS_CACHEERR_INUM`) if `EXTMEM_CORE0_ACS_CACHE_INT_ST` or `CACHE_ILG_INT_ST` is non-zero. Test expectation on C3: `Cache error` + "Cached memory region accessed while ibus or cache is disabled". | READ `IDF:panic_arch.c:196-210`, `tools/test_apps/system/panic/pytest_panic.py:336-338` |
| Invalid MMU entry (bit 8 set) | `CACHE_ILG_INT_ST.MMU_ENTRY_FAULT_ST` (b5) "MMU entry fault"; `CACHE_MMU_FAULT_CONTENT` records the entry content and an operation code ("1: cpu miss, 2: preload miss..."), `CACHE_MMU_FAULT_VADDR` the address. | READ `SVD` descriptions, `cache_err_int.c` |
| Value returned for such an access | **Unknown.** Only "wrong data" is stated: ROM header `cache.h`, `CACHE_FREEZE_ACK_ERROR`: "cache ack wrong data to CPU and trigger an error if a cache miss happens". | READ |
| Store to either window | `DBUS_WR_ICACHE_ST` / `IBUS_WR_ICACHE_ST` ("Dbus tried to write cache", "Ibus tried to write cache"). Whether the CPU also takes a store fault is unknown. | READ `cache_err_int.c` |
| `CTRL1` bits and `CTRL.ENABLE` otherwise | The only software that sets `SHUT_*` while running is the flash-operation guard (`Cache_Suspend_ICache`) where all code runs from IRAM; so a healthy firmware never trips the check. | READ |
| Freeze with `MODE=0` | A cache miss is answered "busy" (the CPU stalls); `MODE=1`: wrong data and an error. | READ `cache.h` |

### 6.2 QEMU `esp32c3` (source read + two probe programs run)

| Case | QEMU | Tag |
|---|---|---|
| Architecture | The 8 MiB D window is a ROM-device RAM region that is **filled with a copy of the 64 KiB flash page at the moment an MMU entry is written** (only if the written value differs). The I window is an alias of it. Later flash writes are not seen. | READ `esp32c3_cache.c:59-91, 243-254` |
| Valid page | load returns flash data via both windows (`0x4F0206E9`) | MEASURED |
| Entry written from valid to invalid | page filled with `0xDEADBEEF` | READ + MEASURED (`lw` -> `0xdeadbeef`, `lbu` of byte 1 -> `0xbe`) |
| Entry never mapped (reset state) | loads return `0` (no fill at reset) | MEASURED |
| Fetch from an unmapped entry | reads zeros: `mcause 2`, `mepc = 0x42040000`, `mtval 0` | MEASURED |
| Fetch after valid -> invalid | decodes `0xDEADBEEF` as a `jal` (low 7 bits `0x6F`) and runs on: `mepc 0x4200B5EA`, illegal instruction, `mtval 0xFFFFFFFF` (the erased flash word) | MEASURED |
| `CTRL.ENABLE` / `CTRL1.SHUT_*` | **no effect** on data (probe: with `CTRL1 = 3`, `lw` still returns flash data); no cache error ever raised | MEASURED |
| Store to a window | `mcause 7` (store access fault), `mtval` = address, memory unchanged | MEASURED |
| Load at `0x3C800000` (past the window) | `mcause 5`, `mtval 0x3C800000` | MEASURED |
| Register reads | `ICACHE_CTRL` reads the stored enable; `ICACHE_CTRL1` and all unlisted registers read **0 whatever was written** (`CTRL1` reads 0 after writing 3; MEASURED); `CACHE_STATE` = 1; `SYNC/PRELOAD/AUTOLOAD_CTRL` return the "done" form on read (QEMU clears the `ENA` bit **on read**) and keep `DONE` sticky forever; `FREEZE.DONE` set/cleared on write | READ + MEASURED |
| MMU table read | stored 9-bit value (`0xFFFFFFFF` -> `0x1FF`) | MEASURED |
| Timing | zero cost for misses | oracle report §1d |

## 7. Tests to define (values from sections 1 to 5)

| # | Test | Expected |
|---|---|---|
| T1 | After the ROM's `ROM_Boot_Cache_Init`, read all 128 entries | all `0x100` |
| T2 | Write entry 3 = 1; read `0x3C030000` and `0x42030000` | both = `flash[0x10000..]` first word |
| T3 | Rewrite entry 3 = 2 without invalidating; read | new page immediately (live view, differs from QEMU's snapshot and from silicon-with-stale-lines; document) |
| T4 | hello_world image: table at app start | `[2, 1, 0x100 x125, 1]`; `0x42000020` reads the app's first instruction |
| T5 | CrossPoint 1.6.5 at `0x10000` | entries 0..32 = `0x35..0x55`, 33..84 = `0x01..0x34`, 85..126 = `0x100`, 127 = `0x01` |
| T6 | `Cache_Dbus_MMU_Set(0, 0x3C000000, 0x20000, 64, 3, 0)` through the jump table `0x40000564` | returns 0, entries 0..2 = `2,3,4`; `fixed=1` -> `2,2,2` |
| T7 | `Cache_Dbus_MMU_Set` error paths | `vaddr=0x3C000010` -> 2; `psize=32` -> 3; `vaddr=0x3C7F0000, num=2` -> writes entry 127 then returns 4 |
| T8 | `Cache_Suspend_ICache` then `Cache_Resume_ICache(0)` with the CPU running from IRAM | returns 0; ends with `CTRL=1`, `CTRL1=0`, no hang; exactly the accesses of 4.1 |
| T9 | `Cache_Disable_ICache` | ends with `CTRL=0`, no hang on `CACHE_STATE`, `SYNC_CTRL = 2` |
| T10 | `Cache_Invalidate_Addr(0x3c550000, 0x10000)` | `SYNC_ADDR=0x3c550000`, `SYNC_SIZE=0x800`, returns 0; `(0x40000000, 4)` returns 1 |
| T11 | App remap trace (1.2) replayed against the model | each poll terminates on the first read |
| T12 | Instruction fetch through the I window while `CTRL1.SHUT_IBUS=1` | (policy choice, section 10) |

Differential check (cheap, high value): run the same ROM + hello_world image on inkemu and compare the sequence of `(op, offset, value)` on `esp32c3.cache` against `scratchpad/cm/art/hello_cache_trace.txt` after mapping QEMU's known divergences (`CTRL1` reads, `AUTOLOAD` read quirk).

## 8. Timing: documented sources (no model proposed)

| Source | What it gives |
|---|---|
| `TRM` §3.3.3.2 | "When a cache miss occurs, the cache controller will initiate a request to the external memory"; the CPU stalls; no cycle count |
| `IDF:docs/en/api-guides/unit-tests.rst:265-267`, `performance/speed.rst:61,153,286`, `memory-types.rst:81` | qualitative: hit is "very fast", miss stalls "whilst waiting"; no numbers |
| Cache geometry (section 3) and fill = one 32-byte line over SPI0 in the configured read mode | the model inputs |
| SPI0 registers programmed by ROM/bootloader (`CTRL`, `USER`, `USER1` dummy cycles, `CLOCK`, `CACHE_FCTRL`; read mode DIO, clock div 1 at 80 MHz, bootloader prints "SPI Mode : SLOW READ"), `esp32c3-boot-chain.md` §D | the SPI clocks per fill |
| `EXTMEM.IBUS_ACS_CNT / IBUS_ACS_MISS_CNT / DBUS_ACS_*` (0x068-0x074) | hardware hit/miss counters: an oracle on a real unit would be the first measurement |
| Published CoreMark numbers (`sp1b-test-oracle.md` §3) | whole-system timing with the cache in the loop |
| Back-of-envelope (ASSUMED, not a source): `Fast Read Dual I/O` at 80 MHz needs about 8 + 12 + 4 command/address/mode clocks plus 128 data clocks for 32 bytes, i.e. roughly 150 SPI clocks (about 1.9 us, about 300 CPU cycles at 160 MHz). To be measured. | |

No data exists in the repository's sources for hit latency or for the interaction between a miss and a concurrent SPI1 command. QEMU charges nothing, which the oracle runs rely on (CPI = 1 at 8 ns per instruction).

## 9. Open questions (ranked by risk to SP1b-2)

1. **High: value returned and exception raised for a fetch/load from an invalid MMU entry or with a disabled bus.** Sources say "illegal instruction before the interrupt" and "wrong data", nothing more. A real unit or a datasheet erratum would settle it. Risk if wrong: only matters for buggy software; healthy ROM, bootloader and CrossPoint boot never trigger it (no `ILG_INT_ST`, no `ACS_INT_ST` in the measured boots). Mitigation: a policy switch (section 10) and a test that the healthy boots never reach the fault path.
2. **High: staleness model.** Should the window show flash content live (simple, proposed) or snapshot at invalidate/MMU-write time like silicon? A live view diverges from silicon for firmware that rewrites flash and re-reads the mapping without `Cache_Invalidate_Addr`; QEMU diverges the other way (snapshot at MMU write, never refreshed by flash writes). Needed for OTA/NVS flows only. Decide with the SPI1 write path in the plan.
3. **Medium: decode-cache invalidation.** Code fetched through `0x42000000` is cached by the CPU's decode cache; an MMU write or an `ICACHE_CTRL`/`SHUT_IBUS` change must invalidate decoded flash code, otherwise a remapped page runs stale. Not a hardware question but a model requirement (the repository already tracks `DecodeCache::clear()`).
4. **Medium: store to a flash window.** Hardware raises `DBUS_WR_ICACHE_ST`/`IBUS_WR_ICACHE_ST`; whether the CPU also gets a store access fault is unknown (QEMU: fault, `mcause 7`). Proposed: fault plus latch the status bit.
5. **Medium: reset value of `ICACHE_SYNC_CTRL`.** SVD says `1` (`INVALIDATE_ENA=1`), which suggests a hardware tag-initialise at reset that completes before the ROM runs. Proposed model reset: `0x2` (DONE=1, ENA=0). Either works for the ROM (it writes `|=1` then polls bit 1), but tests that read the reset value must agree.
6. **Low: width of an MMU entry** (9 vs 10 bits, bits above 8). Software only uses `[7:0]` and bit 8.
7. **Low: power-on content of the MMU table** (the ROM re-inits it). Do CPU resets (`RTC_SW_CPU_RST`) clear EXTMEM registers and the table? The ROM re-inits both anyway, `esp_restart_noos` disables the cache first.
8. **Low: flash page beyond the flash size** (e.g. page `0x80` on a 4 MiB part). Not reachable on the 16 MiB Xteink flash.
9. **Low: SRAM0 (`0x4037C000-0x4037FFFF`) while the cache is on.** The model can keep it as plain RAM; the hardware dedicates it to cache lines. Nothing in the ROM, bootloader or app uses it afterwards (IDF IRAM starts at `0x40380000`).
10. **Low: eFuse `DIS_ICACHE` / flash-encryption (XTS-AES) path through the cache.** Out of scope for the X4 (no flash encryption found).

## 10. Minimal model: what the emulator must implement

**Decode**: `EXTMEM` at `0x600C4000-0x600C4FFF` (registers below `0x104` plus `0x3FC`); the MMU table at `0x600C5000-0x600C51FF`; two flash windows `0x3C000000-0x3C7FFFFF` (data, read) and `0x42000000-0x427FFFFF` (fetch + read), both backed by the same 128-entry table and the flash image. Everything else unchanged (unlisted EXTMEM offsets keep the existing "unmodelled" path).

**MMU table**: 128 x u32, reset `0x100` each (post-ROM state; ROM re-inits). Word writes store `value & 0x1FF` (bit 9 free); reads return the stored value. Byte/half accesses: not needed (every observed access is a word access).

**Window access**: for vaddr in a window, `i = (vaddr & 0x7FFFFF) >> 16`; if `entry[i]` bit 8 is clear, read `flash[(entry[i] & 0xFF) << 16 | (vaddr & 0xFFFF)]` live (little-endian; 8/16/32-bit loads and instruction fetch including 16-bit-aligned compressed instructions; the same flash for both bases). Stores to a window raise a store access fault (QEMU behaviour, MEASURED). Loads/fetch past `0x3C7FFFFF`/`0x427FFFFF` are access faults like any unmapped address.

**Invalid-entry policy** (open question 1), default to the diagnosable choice: return `0` and set `CACHE_ILG_INT_ST.MMU_ENTRY_FAULT_ST` (b5) with `CACHE_MMU_FAULT_VADDR`/`CONTENT`; a fetch of zero already raises `mcause 2`, matching the QEMU probe and IDF's "illegal instruction first" comment. Raise the interrupt line (source 36) only if `ILG_INT_ENA` b5 is set. Add a `strict` flag that stops the run with a named diagnostic on the first such access (healthy boots must not trip it; the plan should assert that).

**Bus gating** (optional, default off): when `CTRL.ENABLE == 0` or `CTRL1.SHUT_IBUS == 1` a fetch in the I window, and when `CTRL.ENABLE == 0` or `CTRL1.SHUT_DBUS == 1` a load in the D window, latch `CORE0_ACS_CACHE_INT_ST` b0/b3 and behave as for an invalid entry. The measured boots never rely on it. Without it, the model is exactly QEMU's.

**Registers with logic** (everything else in section 2 is plain storage with the SVD reset value, `INT_ST = RAW & ENA` not required):
1. `ICACHE_SYNC_CTRL (0x28)`: write with b0=1 -> run the (no-op) invalidate, clear b0, set b1. Reading never shows b0=1 after a write. `DONE` (b1) is read-only.
2. `ICACHE_PRELOAD_CTRL (0x34)`: reset `0x2`; write with b0=1 -> clear b0; DONE (b1) stays 1.
3. `ICACHE_LOCK_CTRL (0x1C)`: reset `0x4`; write with b0 or b1 -> clear them; DONE (b2) stays 1.
4. `ICACHE_AUTOLOAD_CTRL (0x40)`: reset `0x8`; DONE (b3) always 1 and read-only; other bits plain storage; **do not** clear `ENA` on read.
5. `ICACHE_FREEZE (0xCC)`: `DONE` (b2) = `ENA` (b0) after every write; software writes to b2 ignored.
6. `CACHE_STATE (0xB0)`: constant `1`.
7. `REG_DATE (0x3FC)`: constant `0x02007160`.
8. `ICACHE_CTRL (0x00)` reset 0; `ICACHE_CTRL1 (0x04)` reset 3 (already implemented).
9. Counters `0x68-0x74`, `0x64` write-only: storage; optionally count window accesses and misses for the timing work.

**Companion registers** (plain storage, SVD reset): `SYSTEM.CACHE_CONTROL` (`0x600C0040`, reset 5) and `SENSITIVE.INTERNAL_SRAM_USAGE_1` (`0x600C101C`, reset `0xF`), plus `SENSITIVE.CACHE_MMU_ACCESS_1` (`0x600C1034`, reset 3). Without the first two, the ROM stops at `ROM_Boot_Cache_Init` as an unmodelled access.

**Not needed for SP1b-2**: cache line contents, tags, locking, preload and autoload engines, hit/miss accounting, timing, flash encryption through the cache, PMS tables, the cache-error interrupt sources beyond latching the status bits.

**Re-decode hook**: any write to the MMU table, to `ICACHE_CTRL`, or to `ICACHE_CTRL1` bit 0 must invalidate the CPU decode cache for the I window (open question 3).

**Acceptance**: with these pieces the unmodified ROM runs `ROM_Boot_Cache_Init`, `ets_loader_map_range` and the exit sequence, the 2nd-stage bootloader's `bootloader_mmap`/`set_cache_and_start_app`, and the app's `esp_mmu_map`, without any poll that fails to terminate; the cache-register trace of the hello_world boot equals `hello_cache_trace.txt` up to the two documented QEMU quirks (T1 to T11).

## Appendix: artefacts

`scratchpad/cm/` holds: `dl/trm.pdf` and `trm.txt` (TRM v1.4, extracted with `pypdf` inside the container); `art/hello_cache_trace.txt`, `art/cp_cache.txt` (cache-only traces), `art/t_probe1.S`, `art/t2.S` (probe programs), `art/cm_*.sh` (run scripts), `rd.py` (reads ROM ELF data by VMA). The trace logs of the CrossPoint boot reached 2.6 GB uncompressed and were deleted after extraction.
