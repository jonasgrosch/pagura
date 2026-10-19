"""Independently constructed issuer graphs; no mathematical signatures."""
from .paths import name, certificate, CA_BASIC
from .fixtures import seq, set_of, oid, tlv, extension



def main():
    root, shared = name('Root'), name('Shared')
    items = [certificate(root, root, 1),
             certificate(shared, root, 2, [CA_BASIC]),
             certificate(shared, root, 3, [CA_BASIC, extension(99, seq(), True)]),
             certificate(name('Target'), shared, 4),
             certificate(name('CycleA'), name('CycleB'), 5, [CA_BASIC]),
             certificate(name('CycleB'), name('CycleA'), 6, [CA_BASIC]),
             certificate(name('CycleTarget'), name('CycleA'), 7),
             certificate(seq(set_of(seq(oid(1, 2, 3), tlv(12, b'Unknown matching rule')))), root, 8),
             *[certificate(shared, shared, n, [CA_BASIC]) for n in range(9, 19)]]
    for n, data in enumerate(items):
        print(n, data.hex(), sep='\t')
