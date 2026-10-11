# Recherche ROM ESP32-C3 — esp-rom-elfs 20260528 (2026-10-11)

Tout le travail est dans `.../scratchpad/rom` (téléchargements, ELF extraits dans `rom/x/`, listes de symboles `rom/c3*.nm`)
et `.../scratchpad/romtools/` (scripts), images de test dans `.../scratchpad/romout/`. Rien n'est écrit dans le dépôt.
Aucun fichier téléchargé n'a été exécuté : `tar -x` (4 membres), `readelf`/`objdump`/`nm` (via Docker `inkemu/riscv-gcc`, montage `:ro`), et `python3 -I`.

## 1. Téléchargement et intégrité

```
gh release download 20260528 -R espressif/esp-rom-elfs -D <scratch>/rom
cat esp-rom-elfs-20260528-checksum.sha256 ; sha256sum esp-rom-elfs-20260528.tar.gz
```

Publié : `caa463d3cbef2430a5a35847c1d9f2f152403b17a802050927ff60c8da54fe46  esp-rom-elfs-20260528.tar.gz` (4 902 355 octets).
Calculé : identique. Le SHA-256 correspond à celui épinglé dans l'ADR 0001.

## 2. Contenu de l'archive (17 ELF, à plat, sans dossier)

`tar -tvzf` : esp32_rev0, esp32_rev300, esp32c2_rev100, esp32c2_rev200, **esp32c3_rev0 / esp32c3_rev101 / esp32c3_rev3**, esp32c5_rev0, esp32c5_rev100,
esp32c61_rev100, esp32c6_rev0, esp32h2_rev0, esp32p4_rev0, esp32p4_rev300, esp32s2_rev0, esp32s31_rev0, **esp32s3_rev0**, tous `*_rom.elf`.

Nommage des révisions : `rev<major*100+minor>` => rev0 = v0.0, rev3 = v0.3, rev101 = v1.1 (les `_rom_eco_version` lus dans les ROM valent respectivement 0, 3, 7).

| Fichier | Taille | SHA-256 |
|---|---|---|
| `esp32c3_rev0_rom.elf`   | 547 036 | `11d565922e0a405b477756de748a14046f86dfb40d997766252a04cb378f0694` |
| `esp32c3_rev3_rom.elf`   | 556 332 | `19ac22e08707df926fb0cf4c54795d4067b983fae8635f396ded173a6d78fc3c` |
| `esp32c3_rev101_rom.elf` | 559 244 | `9495e1453f36f7eec4112714ea1dca22d77808776adffa93dda7dda4393e8336` |
| `esp32s3_rev0_rom.elf`   | 949 552 | `c0ce0f338d1de1bdc6efbef1591779a2a42c1ab7d759d3c6ae8ae63a7dd34cfd` |

S3 : Xtensa (e_machine 94), entry 0x40000400, 44 en-têtes de programme. Non analysé plus loin (hors périmètre C3).

## 3. Contenu des ELF C3 : ROM complète ou symboles seulement ?

**Les ELF contiennent la ROM complète** (code + données), pas seulement des symboles. Mais trois pièges empêchent une conversion naïve « tous les PT_LOAD à filesz > 0 ».

`readelf -hlS -W esp32c3_rev3_rom.elf` : ELF32 LE, RISC-V, fanions 0x1 (RVC, soft-float), entry 0x40000000, 29 PT_LOAD, 75 sections.
PT_LOAD avec filesz > 0 (rev3) :

| # | vaddr | paddr (LMA) | filesz | memsz | flags | contenu |
|---|---|---|---|---|---|---|
| 1 | 0x3fcc9000 | 0x3fcca000 | 0x3d4 | 0x157b8 | RW | **artefact d'éditeur de liens : ce sont les octets de l'en-tête ELF lui-même** (`7f454c46…`) + bss. À exclure. |
| 2 | 0x40000000 | 0x40000000 | 0x59590 | 0x59590 | R E | **code IROM**, 365 968 octets (.fixed.text, .init.text, .bt_text, .text) |
| 6, 7 | 0x3fccae00 | 0x40059590 | 0xd8 | 0xd8 | RW | tout à zéro (doublon exact) |
| 14 | 0x3fccae00 | 0x40059664 | 0x4 | 0x4 | RW | tout à zéro |
| 27 | 0x3ff19c00 | 0x40059c00 | 0x4ec8 | 0x4ec8 | R | **.rodata (DROM)**, 20 168 octets |

Les autres PT_LOAD (filesz 0) sont des coquilles vides issues des sections `.data_*`/`.bss_*`.

Piège 1 : le PT_LOAD #1 (en-tête ELF) ne doit pas être chargé. Critère fiable : ne garder que les segments dont l'**adresse de chargement (paddr)** est dans la ROM `[0x40000000, 0x40060000)`.

Piège 2 : `.rodata.interface` (vaddr 0x3ff1ee3c, 0x11c4 = 4 548 octets, 3 258 octets non nuls) est une section PROGBITS `W` **non ALLOC**, donc dans **aucun** PT_LOAD. Elle se termine exactement à 0x3ff20000 = fin de la ROM. Il faut la lire par la table des sections.

Piège 3 : les valeurs initiales des `.data_*` (phyrom, pp_rom, spi_flash, interface…) sont lues au démarrage par `_init` dans la ROM, à des adresses `0x40059590…0x40059ac4`, c'est-à-dire **dans l'intervalle entre la fin de l'IROM et le début de .rodata**. Les PT_LOAD ne portent là que 0xd8+4 octets **à zéro**. Les vraies valeurs ne figurent dans l'ELF que comme sections non ALLOC aux adresses RAM de destination (ex. `.data_phyrom` @ 0x3fcdf300, 0x2bc octets, 699 non nuls). Voir §7 pour la reconstruction.

Adresses de ROM, rev3 (mappage vérifié par la paire vaddr/LMA de l'ELF) :
- IROM : 0x40000000..0x40059590 (code + tables `unpack` @0x40059200 et `clear` @0x40059410).
- Image physique ROM0 = 384 KiB = 0x40000000..0x40060000 (taille identique aux `.bin` de QEMU : 393 216).
- Fenêtre bus de données : vaddr `0x3ff00000 + X` <=> offset ROM `0x40000 + X` (rodata 0x3ff19c00 <=> LMA 0x40059c00 ; rodata.interface 0x3ff1ee3c <=> 0x4005ee3c). La DROM de 128 KiB 0x3ff00000..0x3ff20000 est donc les 128 derniers KiB de la même ROM.
- Données lisibles par le DROM-bus : 0x3ff19c00..0x3ff1eac8 (.rodata) + 0x3ff1ee3c..0x3ff20000 (.rodata.interface).

**Les trois révisions n'ont pas la même disposition** (ne jamais coder les tailles en dur) :

| rev | PT_LOAD code | rodata (vaddr, taille) | fin de rodata | octets fournis par l'ELF / 393 216 (après reconstruction §7) |
|---|---|---|---|---|
| rev0   | 0x40000000, 0x58c3e | 0x3ff19c00, 0x4ec8 | 0x3ff1eac8 | 389 484 |
| rev3   | 0x40000000, 0x59590 | 0x3ff19c00, 0x4ec8 | 0x3ff1eac8 | 392 004 |
| rev101 | 0x40000000, 0x599cc | 0x3ff19f70, 0x4d50 | 0x3ff1ecc0 | 392 712 |

rev0 n'a pas de PT_LOAD « en-tête ELF » ; rev3 et rev101 l'ont. Dans rev0, `_bss_end` (0x40058c0c) ≠ fin du PT_LOAD (0x40058c3e) ; dans rev3/rev101 ils coïncident.

Octets que l'ELF ne fournit **pas** (laissés à 0x00 par la conversion), rev3 : 0x4005966c+0xc (12 o, taille de `.data_btdm`), 0x40059ac4+0x13c (316 o, bourrage avant .rodata), 0x4005eac8+0x374 (884 o, bourrage entre .rodata et .rodata.interface). rev0 : 0x40058d1c+8, 0x400590e8+0xb18, 0x4005eac8+0x374. rev101 : 0x40059aa8+0x5c, 0x40059f50+0x20, 0x4005ecc0+0x17c. Ce sont a priori des bourrages d'alignement ; **non vérifié** faute d'un vidage de silicium.

## 4. Vecteur de reset / premières instructions

Entry ELF = `_start` = **0x40000000** pour les trois révisions. Hypothèse matérielle (le CPU démarre à 0x40000000 après reset) cohérente avec l'ELF, **non confirmée par une source externe** dans cette recherche.

```
docker run --rm -v <scratch>/rom/x:/w:ro -w /w inkemu/riscv-gcc \
  riscv64-unknown-elf-objdump -d -M no-aliases --start-address=0x40000000 --stop-address=0x40000010 -z esp32c3_rev3_rom.elf
40000000 <_start>:  6910106f   jal zero,40001e90 <_init>
40000004..4000000f : 0000 (c.unimp) x6   <- octets zéro, jamais exécutés
40000010 <_rom_chip_id>:    0x00000005   (5 = ESP32-C3)
40000014 <_rom_eco_version>: 0x00000003  (rev3 ; 0 pour rev0 ; 7 pour rev101)
40000018.. : table de sauts __call_* (une instruction `jal zero,<fonction>` par entrée, pas de code propre)
```

`_start` ne contient qu'**une** instruction (un `jal` vers `_init`). `_init` (rev3, 0x40001e90) :

```
40001e90 csrrw zero,mstatus,zero        ; mstatus = 0
40001e94 csrrw zero,0x0,zero            ; CSR 0x000 = 0 (ustatus)
40001e98 lui   t0,0x600c2
40001e9c addi  t0,t0,260                ; t0 = 0x600c2104
40001ea0 sw    zero,0(t0)               ; écriture registre périphérique (0x600c2104 <- 0)
40001ea4 c.li  t0,1
40001ea6 csrrw zero,0x800,t0            ; CSR custom 0x800 = 1
40001eaa c.li  t0,1
40001eac csrrw zero,0x801,t0            ; CSR custom 0x801 = 1
40001eb0 c.li  gp,0
40001eb2 auipc t0,0x0
40001eb6 addi  t0,t0,-434               ; t0 = 0x40001d00 (_vector_table)
40001eba ori   t0,t0,1                  ; mode vectorisé
40001ebe csrrw zero,mtvec,t0            ; mtvec = 0x40001d01
40001ec2 c.li  t1,1
40001ec4 lui   t0,0x600c2 ; addi t0,t0,404 ; sw t1,0(t0)   ; 0x600c2194 <- 1
40001ed0 c.li t0,9 ; c.li t1,1 ; csrrw mstatus,t0 ; csrrw CSR0x000,t1
40001edc sp = 0x3fcde710 (ets_intr_count)   ; pile
… boucle `unpackloop` (table @0x40059200..0x40059400, entrées de 16 o : dst_start, dst_end, src, 0)
… boucle `clearloop`  (table @0x40059410..0x40059590, entrées de 12 o)
40001f40 jal zero,40047e3c <main>
```

Points utiles à l'émulateur : le CSR custom 0x800/0x801 (activés tôt), les écritures `0x600c2104` / `0x600c2194` (bloc périphérique 0x600c2000, à identifier avec le SVD), `mtvec` = `_vector_table | 1` (32 entrées `jal` vers `_interrupt_handler`), et le fait que `_init` **lit des tables de ROM par `lw`** sur le bus d'instructions (0x40059200…).
Positions de `_vector_table`/`_init` : rev0 0x40001c00/0x40001d90 ; rev3 0x40001d00/0x40001e90 ; rev101 0x40001f00/0x40002090. Pour les trois, l'octet 0 est un `jal zero,<_init>` et les octets 4..15 sont nuls (vérifié par script).

## 5. Symboles utiles (adresses, par révision)

Source : `riscv64-unknown-elf-nm -n` (4 469 symboles pour rev3). Sortie brute dans `rom/keysyms.txt`.

| symbole | rev0 | rev101 | **rev3** |
|---|---|---|---|
| `_start` | 40000000 | 40000000 | **40000000** |
| `_rom_chip_id` / `_rom_eco_version` | 40000010 / 40000014 | idem | idem |
| `_vector_table` | 40001c00 | 40001f00 | **40001d00** |
| `_init` | 40001d90 | 40002090 | **40001e90** |
| `boot_prepare` | 4004745c | 400497e4 | **40047c5c** |
| `main` | 4004763c | 400499c4 | **40047e3c** |
| `ets_printf` | 40047ef2 | 4004a216 | **400486d4** |
| `ets_install_uart_printf` | 40047ed6 | 4004a1fa | **400486b8** |
| `uart_tx_one_char` | 4004bb50 | 4004df1e | **4004c3ec** |
| `uart_tx_one_char2` | 4004bbb8 | 4004df86 | **4004c454** |
| `uart_rx_one_char` | 4004bc72 | 4004e040 | **4004c50e** |
| `ets_delay_us` | 40045aa0 | 40047e8e | **400462cc** |
| `ets_update_cpu_frequency` | 40045abc | 40047eaa | **400462e8** |
| `ets_get_cpu_frequency` | 40045ac6 | 40047eb4 | **400462f2** |
| `ets_get_apb_freq` | 400490de | 4004b3a6 | **40049866** |
| `Cache_Enable_ICache` | 40050ac4 | 40052ff8 | **40051436** |
| `Cache_Disable_ICache` | 40050a76 | 40052faa | **400513e8** |
| `Cache_Suspend_ICache` | 40050ad4 | 40053008 | **40051446** |
| `Cache_Resume_ICache` | 40050b24 | 40053058 | **40051496** |
| `Cache_Invalidate_ICache_All` | 400507b4 | 40052ce8 | **40051126** |
| `Cache_MMU_Init` | 40050cfe | 40053232 | **40051670** |
| `Cache_Ibus_MMU_Set` | 40050d14 | 40053248 | **40051686** |
| `Cache_Dbus_MMU_Set` | 40050de4 | 40053318 | **40051756** |
| `esp_rom_spiflash_read` | 4004d6e2 | 4004fa70 | **4004df94** |
| `esp_rom_spiflash_config_readmode` | 4004d376 | 4004f704 | **4004dc28** |
| `esp_rom_spiflash_unlock` | 4004d2ce | 4004f65c | **4004db80** |
| `esp_rom_spiflash_erase_sector` | 4004d58c | 4004f91a | **4004de3e** |
| `esp_rom_spiflash_write` | 4004d604 | 4004f992 | **4004deb6** |
| `esp_rom_spiflash_wait_idle` | 4004ce92 | 4004f220 | **4004d744** |
| `ets_loader_map_range` | 40049260 | 4004b536 | **400499f6** |
| `ets_set_user_start` | 400473e0 | 40049768 | **40047be0** |
| `ets_efuse_read` | 40045f34 | 40048324 | **40046760** |
| `software_reset` | 40048392 | 4004a6b6 | **40048b74** |
| `rtc_get_reset_reason` | 400482ac | 4004a5d0 | **40048a8e** |
| `memcpy` | 40057e52 | 40058bdc | **400587ba** |

Absents sur C3 (aucun symbole) : `Cache_Read_Enable` (famille ESP32 ; C3 a `Cache_Enable_ICache`/`Cache_Resume_ICache`), `esp_rom_printf` (C3 : `ets_printf`), `ets_set_appcpu_boot_addr` (mono-cœur). Pas de `ResetVector` ; `_start` fait office de vecteur de reset. Les adresses diffèrent d'une révision à l'autre : **elles ne doivent pas être codées en dur sans clé de révision**.

## 6. Révision par défaut pour Xteink X4 / X3

Preuves :
- README esp-rom-elfs : « You can check the revision of a chip either using `esptool.py chip_id` … If the archive doesn't contain the ROM ELF file for your chip revision, use the next lower one. » => règle « la plus haute révision d'ELF ≤ révision de la puce ».
- En-tête du firmware `crosspoint-1.6.5-x3-x4.bin` (5 589 776 octets) : magic 0xE9, 7 segments, entry 0x40380994, `chip_id` = 5 (ESP32-C3), octet 14 (min_chip_rev legacy) = 0, `min_chip_rev_full` (octets 15..17) = 0x0000 = **0**, `max_chip_rev_full` (17..19) = 0xFFFF = **65535 (aucune borne haute)**, hash ajouté = 1. Descripteur d'application : version 1.6.5, projet `crosspoint-reader`, IDF 5.5.5, `min_efuse_blk_rev_full` 0, `max_efuse_blk_rev_full` 199, MMU page 2^16.
- Aucune mention de révision de puce dans le dépôt inkemu (`grep`). `builder.rs` écrit 0 / 0xFFFF par défaut.

Conclusion : le firmware accepte **toute** révision de puce, donc l'image ne permet **pas** de déduire la révision matérielle des Xteink X4/X3. **Inconnu** : la révision réelle du silicium. Il faudrait lire `esptool chip_id` / `esp_chip_info` sur un appareil, ou le bootloader/log série.

Recommandation (jugement, pas une preuve) : défaut **`esp32c3_rev3`** (v0.3), qui d'après la règle « révision inférieure » sert aussi les puces v0.3 jusqu'à < v1.1 (donc aussi une éventuelle v0.4), avec `rev101` (v1.1) et `rev0` sélectionnables par option, et la révision exposée dans le manifeste. Le choix d'une ROM n'est pas neutre : adresses de fonctions, `_rom_eco_version` (3 vs 7) et corrections de ROM diffèrent. Je n'ai pas vérifié dans cette recherche quelles versions de silicium (v0.4 ?) Espressif a effectivement livrées : cette partie vient de ma connaissance générale, à confirmer.

## 7. Recette de conversion testée

Script : `.../scratchpad/romtools/elf2rom.py` (hors du dossier des téléchargements ; `python3 -I`; n'importe rien du dossier). Sortie complète des trois révisions : `romtools/elf2rom-output.txt`.

```
python3 -I elf2rom.py <elf> [outdir]
```

Algorithme (analyse ELF32 LE à la main avec `struct`, sans exécution) :

1. Lire phdr, shdr, `.symtab`.
2. Pour chaque PT_LOAD avec `filesz > 0` dont `[paddr, paddr+filesz)` est dans `[0x40000000, 0x40060000)` : copier `file[offset:offset+filesz]` à `rom[paddr-0x40000000]`. Ignorer les autres (PT_LOAD #1 = en-tête ELF).
3. Pour chaque section PROGBITS non vide dont l'adresse est dans `[0x3ff00000, 0x3ff20000)` et qu'aucun PT_LOAD ne couvre (`.rodata.interface`) : copier à `rom[(addr-0x3ff00000)+0x40000]`.
4. **Reconstruction des valeurs initiales de .data** (nécessaire, les PT_LOAD n'y mettent que des zéros) : lire la table `unpack` dans `[_data_end_btdm_rom, _data_end)` (entrées de 16 octets `dst_start, dst_end, src, 0`, symboles lus dans `.symtab`), et pour chaque entrée non vide copier les octets de la section couvrant `[dst_start,dst_end)` vers `rom[src-0x40000000]`. Contrôle : relu après coup, les 14 entrées non vides concordent (avant : 0 concordent, 14 à zéro ; après : 14/14).
5. Images : IROM = `rom[0:filesz du PT_LOAD exécutable]` ; DROM = `rom[0x40000:0x60000]` (128 KiB, base vaddr 0x3ff00000) ; ROM complète = 384 KiB à la base 0x40000000.

Sorties (SHA-256) — à épingler dans le manifeste ; elles dépendent de l'algorithme ci-dessus (la reconstruction §7.4 est la partie la moins sûre) :

| rev | irom (vaddr 0x40000000) | taille | drom (vaddr 0x3ff00000, 128 KiB) | rom384 (0x40000000, 384 KiB) |
|---|---|---|---|---|
| rev0   | `77ed19f8eb6cdd9a43fdba33dd6cb577f6a88ef5a37522167282b9ea48742630` | 0x58c3e | `e4b26d597ec96a1ab3b01bf97850269c19a80961fa1196cb3813800cb0242939` | `cc0862284deb70ce41d7e3cc4c378bf2221c809362ef4aa0fa6a37e388bf141b` |
| rev101 | `669f4cb91fdc029e93021d16f369e8e504e135a35b572b111c9500b99951ebe1` | 0x599cc | `2abfba16c262b69580c49ceab723d0ab232e2604089530fd5b67588c744fd535` | `4449c3ad12aad45c68dad925c18fbe01b6a5d4658ae0e50f4991195f9e173c94` |
| **rev3** | `79f086df2c58f018041957460aae5eab735e3fb950a18e307a0a4cffa12e1102` | 0x59590 | `d30c6b83a8d997a634df1d611aefd7b398d5f7506f6ec440a83449600abb8eaf` | `3a69ac75662367dc99fbd5b8ddf265515b6288e79e51e4c4168c597aced7b79d` |

Remarque : le SHA-256 de l'IROM seule (code PT_LOAD) ne dépend pas de la reconstruction ; celui de la DROM et de la ROM 384 KiB, si.

## 8. Points ouverts

1. **Révision réelle du silicium des X4/X3** : inconnue (le firmware accepte 0..65535).
2. **Pas de vérité terrain** : les `.bin` de QEMU (393 216 o) n'ont pas été téléchargés (licence non énoncée, cf. ADR) ; la reconstruction des valeurs initiales de `.data` (§7.4) et les bourrages à zéro (§3) ne sont donc pas comparés à un vidage de ROM réel. L'ELF indique de plus deux sources divergentes pour les premiers 0xd8 octets de la zone : le PT_LOAD (zéros) et les sections `.data.interface.*` (131 octets non nuls). La reconstruction retient les sections. À trancher par une comparaison à un `.bin` de référence ou par exécution (les valeurs `.data.interface.*` sont des pointeurs de tables d'interface).
3. **Vecteur de reset matériel** non confirmé hors ELF (0x40000000 supposé).
4. **Fenêtre DROM** : l'équivalence « vaddr 0x3ff00000+X = offset ROM 0x40000+X » est déduite des paires vaddr/LMA de l'ELF (cohérentes pour .rodata et .rodata.interface sur 3 révisions), pas d'une source Espressif consultée ici.
5. Le S3 n'a pas fait l'objet de la recette (Xtensa ; disposition différente : 44 PT_LOAD, entry 0x40000400, rodata @0x3ff18c00).
6. L'archive est extraite avec `tar -x` sur 4 membres nommés ; le chargeur final doit vérifier le SHA-256 de l'archive avant extraction (ADR) et ne jamais extraire de chemins fournis par l'archive.

## 9. Addendum du 2026-10-11 : la reconstruction est validée, `.data_btdm` comprise

Le point ouvert n° 2 est levé pour rev3. L'image de 384 Kio produite par la recette du §7 a été comparée, dans le conteneur `inkemu/esp-idf`, à l'image de référence `esp32c3-rom.bin` livrée avec le QEMU d'Espressif (SHA-256 `0de1e65020e803bea0d7443dca149d61895e01fca3bb9c82d073234eebd73f99`). Ce fichier n'est ni copié ni distribué : il n'a servi qu'à cette comparaison.

- Résultat avant correction : 8 octets différents sur 393 216, tous dans la plage `0x4005966c+0xc`, c'est-à-dire le premier « trou » du §3.
- Cause : `.data_btdm` (12 octets en rev3 : `01010000 01090000 b0dd0240`) est une section non ALLOC **sans entrée dans la table `unpack`** ; le contrôleur Bluetooth copie lui-même ses valeurs initiales. Ni les PT_LOAD ni la table ne disent donc où elles se trouvent en ROM.
- Règle ajoutée à la recette (étape 4 bis) : la première plage que rien ne fournit après `_bss_end` reçoit le contenu de `.data_btdm`. Sa longueur doit être exactement celle de la section (8 octets en rev0, 12 en rev3, 0x5c en rev101 : vérifié sur les trois ELF) ; sinon la conversion échoue.
- La table `unpack` est bornée par `_data_start` et `_data_end` (`_data_end_btdm_rom` a la même valeur que `_data_start`).
- Résultat après correction : rev3 est **identique octet pour octet** à l'image de référence. Les bourrages laissés à zéro (316 et 884 octets) sont donc eux aussi exacts.

Empreintes des images de 384 Kio, à épingler (elles remplacent la colonne « rom384 » du §7) :

| rev | SHA-256 de l'image | Statut |
|---|---|---|
| rev0   | `832b9a17b4bca5fe307d2d84b3fcd9cdc75f6a8a51956a3e114b4d6713710565` | produite par la même règle ; aucune image de référence pour comparer |
| rev3   | `0de1e65020e803bea0d7443dca149d61895e01fca3bb9c82d073234eebd73f99` | identique à l'image de référence |
| rev101 | `8b41b9b114e110e373a58188e6e86b044719f3b4e6da0441c6e8b68b72986651` | produite par la même règle ; aucune image de référence pour comparer |

Le script `tools/research/elf2rom.py` contient la règle et reproduit ces trois empreintes ; il sert de contre-épreuve indépendante à l'implémentation Rust.
