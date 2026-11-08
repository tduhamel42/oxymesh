"""A C++ firmware SPIFFS image for mc-spiffs's tests: made-up identity, settings, contacts,
channels and one stored advert, plus a deleted older settings file that must be skipped.

    python3 -I testdata/spiffs/gen.py > /dev/null    # writes testdata/spiffs/cpp.bin

A writer of our own, in pure Python with no dependencies, independent of the Rust reader: the
SPIFFS layout from `spiffs_nucleus.h` (P. Andersson, MIT) with ESP-IDF's settings (256-byte
pages, 4 KB blocks, 16-bit ids, 32-byte names, 4-byte metadata, magic with length, packed
headers). The real check is the M5's own partition, read but never committed.
Nothing here is anyone's real data.
"""

import hashlib
import importlib.util
import os
import struct

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("air", os.path.join(HERE, "..", "air", "gen.py"))
air = importlib.util.module_from_spec(spec)
spec.loader.exec_module(air)

PAGE = 256
BLOCK = 4096
PAGES_PER_BLOCK = BLOCK // PAGE
LOOKUP_PAGES = 1
IMAGE_SIZE = 0x20000
BLOCKS = IMAGE_SIZE // BLOCK
IX_FLAG = 0x8000
# Flags are active low: a written, final page clears USED and FINAL; index pages clear INDEX.
F_USED, F_FINAL, F_INDEX, F_IXDELE, F_DELET = 1, 2, 4, 0x40, 0x80
DATA_FLAGS = 0xFF & ~(F_USED | F_FINAL)
IX_FLAGS = 0xFF & ~(F_USED | F_FINAL | F_INDEX)
IX_HEADER = 5 + 3 + 4 + 1 + 32 + 4
HDR_IX_LEN = (PAGE - IX_HEADER) // 2
IX_LEN = (PAGE - 8) // 2
DATA = PAGE - 5
CONTACTS = 200  # more than one index header holds (103 data pages of 251 bytes)


class Image:
    def __init__(self):
        self.pages = {}  # page number -> bytes
        self.lut = {}  # page number -> object id
        self.next_page = LOOKUP_PAGES
        self.next_id = 1

    def alloc(self):
        p = self.next_page
        self.next_page += 1
        if self.next_page % PAGES_PER_BLOCK == 0:
            self.next_page += LOOKUP_PAGES
        return p

    def put(self, page, obj_id, data):
        assert len(data) == PAGE
        self.pages[page] = data
        self.lut[page] = obj_id

    def file(self, name, content, deleted=False):
        obj_id = self.next_id
        self.next_id += 1
        spans = [content[i:i + DATA] for i in range(0, len(content), DATA)] or []
        data_pages = []
        header = self.alloc()
        for span, chunk in enumerate(spans):
            p = self.alloc()
            data_pages.append(p)
            body = struct.pack("<HHB", obj_id, span, DATA_FLAGS) + chunk
            self.put(p, obj_id, body.ljust(PAGE, b"\xff"))
        flags = IX_FLAGS & ~F_DELET if deleted else IX_FLAGS
        hdr = struct.pack("<HHB", obj_id | IX_FLAG, 0, flags) + b"\xff" * 3
        hdr += struct.pack("<IB", len(content), 1) + name.encode().ljust(32, b"\0") + b"\xff" * 4
        first = data_pages[:HDR_IX_LEN]
        hdr += b"".join(struct.pack("<H", p) for p in first)
        self.put(header, obj_id | IX_FLAG, hdr.ljust(PAGE, b"\xff"))
        rest = data_pages[HDR_IX_LEN:]
        for k in range(0, len(rest), IX_LEN):
            p = self.alloc()
            ix = struct.pack("<HHB", obj_id | IX_FLAG, 1 + k // IX_LEN, IX_FLAGS) + b"\xff" * 3
            ix += b"".join(struct.pack("<H", q) for q in rest[k:k + IX_LEN])
            self.put(p, obj_id | IX_FLAG, ix.ljust(PAGE, b"\xff"))

    def binary(self):
        out = bytearray(b"\xff" * IMAGE_SIZE)
        for page, data in self.pages.items():
            out[page * PAGE:(page + 1) * PAGE] = data
        for block in range(BLOCKS):
            base = block * BLOCK
            for e in range(PAGES_PER_BLOCK - LOOKUP_PAGES):
                page = block * PAGES_PER_BLOCK + LOOKUP_PAGES + e
                if page in self.lut:
                    out[base + 2 * e:base + 2 * e + 2] = struct.pack("<H", self.lut[page])
            magic = (0x20140529 ^ PAGE ^ (BLOCKS - block)) & 0xFFFF
            end = base + LOOKUP_PAGES * PAGE
            out[end - 4:end] = struct.pack("<HH", magic, 0)
        return bytes(out)


def identity(label):
    seed = hashlib.sha256(label.encode()).digest()
    prv = air.expanded_from_seed(seed)
    return air.public_from_expanded(prv), prv


def contact(i):
    pub, _ = identity(f"contact {i}")
    name = (["Alice", "Bob", "Relay"][i] if i < 3 else f"Node {i}").encode()
    kind = 2 if i == 2 else 1
    rec = pub + name.ljust(32, b"\0")
    rec += bytes([kind, 0x02 if i == 0 else 0, 0])  # type, flags (favourite), unused
    rec += struct.pack("<I", 0)  # sync_since
    rec += bytes([0xFF])  # out_path_len: unknown
    rec += struct.pack("<I", 1_790_000_000 + i)  # last_advert
    rec += bytes(64)  # out_path
    rec += struct.pack("<I", 1_790_000_100 + i)  # lastmod
    rec += struct.pack("<ii", 45_000_000 + i, 6_000_000 - i)  # lat, lon
    assert len(rec) == 152
    return pub, rec


def channel(name, secret16):
    rec = bytes(4) + name.encode().ljust(32, b"\0") + secret16 + bytes(16)
    assert len(rec) == 68
    return rec


PREFS = (
    '{name:"C++ M5 \\"test\\"",lat:45.123456,lon:-5.654321,'
    "radio:{freq:869.4320,bw:125.0000,sf:9,cr:6,cad:0,int_thr:0,rxgain:1,fem_rxgain:0,"
    "fem_txgain:0,tx:20,af:1.5000,rxdelay:0,f_txdelay:0.5000,d_txdelay:0.2000,agc_int:0,"
    "hash_mode:1,multi_ack:1},gps:{en:1,int:60,adv_loc:1},repeat:{disable:1},"
    'comp:{auto_max:3,defs_nm:"",defs_key:"000102030405060708090A0B0C0D0E0F",pin:123456,'
    "buzz_q:1,vibe_q:0,auto_add:2,man_add:1,tel_base:2,tel_loc:1,tel_env:0,tz_offset:1},"
    "custom:{}}"
)


def main():
    pub, prv = identity("oxymesh spiffs test node")
    img = Image()
    # An older settings file, deleted: never read.
    img.file("/prefs.json", b'{name:"old"}', deleted=True)
    img.file("/identity/_main.id", pub + prv)
    recs = [contact(i) for i in range(CONTACTS)]
    public = bytes.fromhex("8b3387e9c5cdea6ac9e5edbaa115cd72")
    test = hashlib.sha256(b"#test").digest()[:16]
    chans = [channel("Public", public), channel("#test", test)] + [bytes(68)] * 38
    img.file("/channels2", b"".join(chans))
    img.file("/contacts3", b"".join(r for _, r in recs))
    img.file("/bl/" + recs[0][0][:8].hex().upper(), bytes(range(100)))
    img.file("/prefs.json", PREFS.encode())
    with open(os.path.join(HERE, "cpp.bin"), "wb") as f:
        f.write(img.binary())
    print(f"node {pub.hex()}")
    print(f"alice {recs[0][0].hex()}")


if __name__ == "__main__":
    main()
