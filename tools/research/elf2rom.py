"""Convert an Espressif ESP32-C3 ROM ELF (esp-rom-elfs) into flat ROM images.
Parses the ELF only; nothing from it is executed.
usage: python3 -I elf2rom.py <elf> [outdir]
"""
import struct, sys, hashlib, os

ROM_BASE, ROM_END = 0x40000000, 0x40060000        # ROM0, 384 KiB, instruction bus
DROM_BASE, DROM_END = 0x3FF00000, 0x3FF20000      # data-bus window onto the last 128 KiB of ROM0
DROM_PHYS_OFFSET = 0x40000                        # DROM byte 0 == ROM offset 0x40000

def parse(path):
    d = open(path, 'rb').read()
    assert d[:4] == b'\x7fELF' and d[4] == 1 and d[5] == 1, 'need ELF32 little endian'
    (_t, machine, _v, entry, phoff, shoff, _fl, _eh, phes, phn, shes, shn, shstr) = struct.unpack_from('<HHIIIIIHHHHHH', d, 16)
    segs = []
    for i in range(phn):
        t, off, va, pa, fs, ms, fl, al = struct.unpack_from('<IIIIIIII', d, phoff + i * phes)
        segs.append(dict(i=i, type=t, off=off, va=va, pa=pa, fs=fs, ms=ms, fl=fl))
    raw = [struct.unpack_from('<IIIIIIIIII', d, shoff + i * shes) for i in range(shn)]
    stroff = raw[shstr][4]
    def cstr(base, n):
        return d[base + n:d.index(b'\0', base + n)].decode()
    secs = [dict(name=cstr(stroff, r[0]), type=r[1], flags=r[2], addr=r[3], off=r[4], size=r[5], link=r[6]) for r in raw]
    syms = {}
    for sc in secs:
        if sc['type'] == 2:  # SYMTAB
            strtab = secs[sc['link']]['off']
            for k in range(sc['size'] // 16):
                n, val, sz, info, oth, shndx = struct.unpack_from('<IIIBBH', d, sc['off'] + k * 16)
                if n:
                    syms.setdefault(cstr(strtab, n), val)
    return dict(d=d, entry=entry, machine=machine, segs=segs, secs=secs, syms=syms)

def build(e):
    d, segs, secs, syms = e['d'], e['segs'], e['secs'], e['syms']
    rom = bytearray(ROM_END - ROM_BASE)
    cov = bytearray(len(rom))                      # 1 = byte provided by a PT_LOAD or a section
    log = []
    def put(pa, data, why):
        o = pa - ROM_BASE
        rom[o:o + len(data)] = data
        for k in range(o, o + len(data)): cov[k] = 1
        log.append('%-7s pa %#010x len %#07x' % (why, pa, len(data)))
    # 1. PT_LOAD with file data whose LOAD address lies inside ROM0. Drops the linker artefact at
    #    vaddr 0x3fcc9000 / paddr 0x3fcca000 whose bytes are the ELF header itself.
    for s in segs:
        if s['type'] == 1 and s['fs']:
            if ROM_BASE <= s['pa'] and s['pa'] + s['fs'] <= ROM_END:
                put(s['pa'], d[s['off']:s['off'] + s['fs']], 'PT_LOAD#%d' % s['i'])
            else:
                log.append('SKIPPED PT_LOAD#%d pa %#x (outside ROM; carries the ELF header)' % (s['i'], s['pa']))
    # 2. PROGBITS sections inside the DROM window that no PT_LOAD covers (.rodata.interface: W, not ALLOC).
    for sc in secs:
        if sc['type'] == 1 and sc['size'] and DROM_BASE <= sc['addr'] < DROM_END:
            if not any(s['type'] == 1 and s['fs'] and s['va'] <= sc['addr'] < s['va'] + s['fs'] for s in segs):
                put(sc['addr'] - DROM_BASE + ROM_BASE + DROM_PHYS_OFFSET, d[sc['off']:sc['off'] + sc['size']], 'SECTION ' + sc['name'])
    return rom, cov, log

def rd(e, rom, pa, n): return bytes(rom[pa - ROM_BASE:pa - ROM_BASE + n])

def check_unpack(e, rom):
    """_init copies initial .data from ROM using 16-byte entries (dst_start, dst_end, src, 0) located in
    [_data_end_btdm_rom, _data_end). Report whether the ELF section bytes at dst match the ROM bytes at src."""
    d, secs, syms = e['d'], e['secs'], e['syms']
    t0, t1 = syms['_data_end_btdm_rom'], syms['_data_end']
    res = dict(entries=0, nonempty=0, match=0, mismatch=0, fill=0, fill_bytes=0, ranges=[])
    for va in range(t0, t1, 16):
        dst, dend, src, _ = struct.unpack('<IIII', rd(e, rom, va, 16))
        res['entries'] += 1
        n = dend - dst
        if n <= 0: continue
        res['nonempty'] += 1
        sec = next((s for s in secs if s['type'] == 1 and s['addr'] <= dst and dend <= s['addr'] + s['size']), None)
        if sec is None: res['ranges'].append((dst, dend, src, 'NO-SECTION')); continue
        want = d[sec['off'] + dst - sec['addr']: sec['off'] + dst - sec['addr'] + n]
        have = rd(e, rom, src, n)
        res['ranges'].append((dst, dend, src, sec['name'], 'ok' if want == have else ('rom-zero' if not any(have) else 'DIFF')))
        if want == have: res['match'] += 1
        else: res['mismatch'] += 1
    return res

def fill_unpack(e, rom, cov):
    """Write the section bytes at the ROM source address of every unpack entry where the ELF has no PT_LOAD data."""
    d, secs, syms = e['d'], e['secs'], e['syms']
    n_filled = 0
    for va in range(syms['_data_end_btdm_rom'], syms['_data_end'], 16):
        dst, dend, src, _ = struct.unpack('<IIII', rd(e, rom, va, 16))
        n = dend - dst
        if n <= 0: continue
        sec = next(s for s in secs if s['type'] == 1 and s['addr'] <= dst and dend <= s['addr'] + s['size'])
        data = d[sec['off'] + dst - sec['addr']: sec['off'] + dst - sec['addr'] + n]
        o = src - ROM_BASE
        rom[o:o + n] = data
        for k in range(o, o + n): cov[k] = 1
        n_filled += n
    return n_filled

def fill_btdm(e, rom, cov):
    """.data_btdm is not ALLOC and has no unpack entry (the Bluetooth controller copies it itself), so
    neither a PT_LOAD nor the unpack table says where its initial values live in the ROM. In the three
    C3 revisions the first byte range after _bss_end that nothing else provides has exactly the size of
    .data_btdm. For rev3 the result is byte-identical to the reference ROM image (sha256 0de1e650...)."""
    d, secs, syms = e['d'], e['secs'], e['syms']
    sec = next((s for s in secs if s['name'] == '.data_btdm' and s['type'] == 1 and s['size']), None)
    if sec is None: return None
    k = syms['_bss_end'] - ROM_BASE
    while k < len(rom) and cov[k]: k += 1
    end = k
    while end < len(rom) and not cov[end]: end += 1
    assert end - k == sec['size'], 'first unprovided range after _bss_end is %#x bytes, .data_btdm is %#x' % (end - k, sec['size'])
    rom[k:end] = d[sec['off']:sec['off'] + sec['size']]
    for i in range(k, end): cov[i] = 1
    return (ROM_BASE + k, sec['size'])

def main():
    path = sys.argv[1]
    e = parse(path)
    syms = e['syms']
    rom, cov, log = build(e)
    print('file %s sha256 %s' % (os.path.basename(path), hashlib.sha256(e['d']).hexdigest()))
    print('e_machine %d entry %#x _start %#x _rom_chip_id=%d _rom_eco_version=%d' % (
        e['machine'], e['entry'], syms['_start'],
        struct.unpack('<I', rd(e, rom, syms['_rom_chip_id'], 4))[0], struct.unpack('<I', rd(e, rom, syms['_rom_eco_version'], 4))[0]))
    print('\n'.join('  ' + l for l in log))
    u = check_unpack(e, rom)
    print('unpack table: %d entries, %d non-empty; ROM bytes at their sources match the .data_* sections: %d, differ: %d (before reconstruction)' % (u['entries'], u['nonempty'], u['match'], u['mismatch']))
    nfill = fill_unpack(e, rom, cov)
    u2 = check_unpack(e, rom)
    print('init-data reconstruction: wrote %#x bytes from .data_* sections to their ROM source; recheck match=%d mismatch=%d' % (nfill, u2['match'], u2['mismatch']))
    btdm = fill_btdm(e, rom, cov)
    print('.data_btdm: ' + ('placed at %#x (%#x bytes)' % btdm if btdm else 'absent'))
    # irom = the executable PT_LOAD at 0x40000000 ; drom = the whole data-bus window
    ex = next(s for s in e['segs'] if s['type'] == 1 and s['fs'] and (s['fl'] & 1) and s['pa'] == ROM_BASE)
    irom = bytes(rom[:ex['fs']])
    drom = bytes(rom[DROM_PHYS_OFFSET:DROM_PHYS_OFFSET + (DROM_END - DROM_BASE)])
    print('irom  vaddr 0x40000000 size %#x sha256 %s' % (len(irom), hashlib.sha256(irom).hexdigest()))
    print('drom  vaddr 0x3ff00000 size %#x sha256 %s' % (len(drom), hashlib.sha256(drom).hexdigest()))
    print('  init _bss_end=%#x (== irom end: %s)' % (syms['_bss_end'], syms['_bss_end'] == ROM_BASE + len(irom)))
    print('rom384 size %#x sha256 %s provided=%d' % (len(rom), hashlib.sha256(bytes(rom)).hexdigest(), sum(cov)))
    holes, start = [], None
    for k, c in enumerate(cov + b'\x01'):
        if not c and start is None: start = k
        if c and start is not None: holes.append((ROM_BASE + start, k - start)); start = None
    print('unprovided ROM ranges (left 0x00): ' + ', '.join('%#x+%#x' % h for h in holes))
    if len(sys.argv) > 2:
        out = sys.argv[2]; os.makedirs(out, exist_ok=True)
        base = os.path.basename(path).replace('_rom.elf', '')
        open(os.path.join(out, base + '_irom.bin'), 'wb').write(irom)
        open(os.path.join(out, base + '_drom.bin'), 'wb').write(drom)
        open(os.path.join(out, base + '_rom384k.bin'), 'wb').write(bytes(rom))
main()
