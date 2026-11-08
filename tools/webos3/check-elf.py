#!/usr/bin/env python3
"""Check minimal ELF properties of a proposed ARMv7 engine artifact."""
import pathlib, struct, sys
p = pathlib.Path(sys.argv[1])
data = p.read_bytes()
if data[:4] != b'\x7fELF' or len(data) < 52: raise SystemExit('Not ELF')
if data[4] != 1: raise SystemExit('Not 32-bit ELF')
if data[5] != 1: raise SystemExit('Not little-endian ELF')
e_machine = struct.unpack_from('<H', data, 18)[0]
if e_machine != 40: raise SystemExit(f'Not ARM ELF: e_machine={e_machine}')
phoff = struct.unpack_from('<I',data,28)[0]
phentsize,phnum=struct.unpack_from('<HH',data,42)
if phoff + phentsize*phnum > len(data): raise SystemExit('Invalid program headers')
for i in range(phnum):
    ptype=struct.unpack_from('<I',data,phoff+i*phentsize)[0]
    if ptype == 3: raise SystemExit('Has dynamic interpreter: not a standalone musl binary')
print('ARMv7 32-bit ELF, little endian, no PT_INTERP (candidate for standalone test)')
