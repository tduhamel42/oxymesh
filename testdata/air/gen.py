#!/usr/bin/env python3
"""Oracle indépendant des trames MeshCore sur l'air (docs/spec/air.md).

Python pur, sans dépendance hors de la bibliothèque standard : AES-128 d'après FIPS-197,
Ed25519 d'après RFC 8032 (signature avec clé privée « étendue » à la orlp), X25519 d'après
RFC 7748, SHA-256/512 et HMAC de hashlib/hmac. Chaque primitive est d'abord vérifiée contre les
vecteurs de sa norme. Le résultat, testdata/air/vectors.toml, est relu par
crates/mc-air/tests/vectors.rs.

Usage : python3 -I testdata/air/gen.py > testdata/air/vectors.toml
"""

import hashlib
import hmac
import sys

# ---------------------------------------------------------------------------------------------
# AES-128 (FIPS-197)


def _xtime(a):
    a <<= 1
    return (a ^ 0x11B) & 0xFF if a & 0x100 else a


def _gmul(a, b):
    r = 0
    while b:
        if b & 1:
            r ^= a
        a = _xtime(a)
        b >>= 1
    return r


def _sbox():
    inv = [0] * 256
    for x in range(1, 256):
        for y in range(1, 256):
            if _gmul(x, y) == 1:
                inv[x] = y
                break
    s = []
    for x in range(256):
        b = inv[x]
        r = b
        for i in range(1, 5):
            r ^= ((b << i) | (b >> (8 - i))) & 0xFF
        s.append(r ^ 0x63)
    return s


SBOX = _sbox()
INV_SBOX = [0] * 256
for _i, _v in enumerate(SBOX):
    INV_SBOX[_v] = _i


def _expand(key):
    w = [list(key[i:i + 4]) for i in range(0, 16, 4)]
    rcon = 1
    for i in range(4, 44):
        t = list(w[i - 1])
        if i % 4 == 0:
            t = t[1:] + t[:1]
            t = [SBOX[b] for b in t]
            t[0] ^= rcon
            rcon = _xtime(rcon)
        w.append([a ^ b for a, b in zip(w[i - 4], t)])
    return [sum(w[r * 4:r * 4 + 4], []) for r in range(11)]


def _shift(s, inv=False):
    out = [0] * 16
    for c in range(4):
        for r in range(4):
            src = (c + r) % 4 if not inv else (c - r) % 4
            out[c * 4 + r] = s[src * 4 + r]
    return out


def _mix(s, inv=False):
    m = [14, 11, 13, 9] if inv else [2, 3, 1, 1]
    out = []
    for c in range(4):
        col = s[c * 4:c * 4 + 4]
        for r in range(4):
            out.append(_gmul(col[0], m[(0 - r) % 4]) ^ _gmul(col[1], m[(1 - r) % 4])
                       ^ _gmul(col[2], m[(2 - r) % 4]) ^ _gmul(col[3], m[(3 - r) % 4]))
    return out


def aes_encrypt_block(key, block):
    ks = _expand(key)
    s = [a ^ b for a, b in zip(block, ks[0])]
    for rnd in range(1, 11):
        s = [SBOX[b] for b in s]
        s = _shift(s)
        if rnd != 10:
            s = _mix(s)
        s = [a ^ b for a, b in zip(s, ks[rnd])]
    return bytes(s)


def aes_decrypt_block(key, block):
    ks = _expand(key)
    s = [a ^ b for a, b in zip(block, ks[10])]
    for rnd in range(9, -1, -1):
        s = _shift(s, inv=True)
        s = [INV_SBOX[b] for b in s]
        s = [a ^ b for a, b in zip(s, ks[rnd])]
        if rnd != 0:
            s = _mix(s, inv=True)
    return bytes(s)


# ---------------------------------------------------------------------------------------------
# Ed25519 (RFC 8032 §5.1) et X25519 (RFC 7748)

P = 2**255 - 19
Q = 2**252 + 27742317777372353535851937790883648493
D = (-121665 * pow(121666, P - 2, P)) % P
SQRT_M1 = pow(2, (P - 1) // 4, P)


def _add(a, b):
    x1, y1, z1, t1 = a
    x2, y2, z2, t2 = b
    A = (y1 - x1) * (y2 - x2) % P
    B = (y1 + x1) * (y2 + x2) % P
    C = 2 * t1 * t2 * D % P
    Dd = 2 * z1 * z2 % P
    E, F, G, H = B - A, Dd - C, Dd + C, B + A
    return (E * F % P, G * H % P, F * G % P, E * H % P)


def _mul(s, pt):
    q = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            q = _add(q, pt)
        pt = _add(pt, pt)
        s >>= 1
    return q


def _recover_x(y, sign):
    if y >= P:
        return None
    x2 = (y * y - 1) * pow(D * y * y + 1, P - 2, P)
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (P + 3) // 8, P)
    if (x * x - x2) % P != 0:
        x = x * SQRT_M1 % P
    if (x * x - x2) % P != 0:
        return None
    if (x & 1) != sign:
        x = P - x
    return x


GY = 4 * pow(5, P - 2, P) % P
GX = _recover_x(GY, 0)
G = (GX, GY, 1, GX * GY % P)


def _compress(pt):
    zinv = pow(pt[2], P - 2, P)
    x = pt[0] * zinv % P
    y = pt[1] * zinv % P
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")


def _decompress(s):
    y = int.from_bytes(s, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    x = _recover_x(y, sign)
    if x is None:
        return None
    return (x, y, 1, x * y % P)


def _sha512_modq(b):
    return int.from_bytes(hashlib.sha512(b).digest(), "little") % Q


def expanded_from_seed(seed):
    """Clé privée orlp de 64 octets : SHA512(graine), scalaire clampé."""
    h = bytearray(hashlib.sha512(seed).digest())
    h[0] &= 248
    h[31] &= 63
    h[31] |= 64
    return bytes(h)


def public_from_expanded(prv):
    a = int.from_bytes(prv[:32], "little")
    return _compress(_mul(a, G))


def sign_expanded(prv, msg):
    """Signature orlp (sign.c) : scalaire pris tel quel, préfixe de nonce = prv[32..64]."""
    a = int.from_bytes(prv[:32], "little")
    A = public_from_expanded(prv)
    r = _sha512_modq(prv[32:] + msg)
    R = _compress(_mul(r, G))
    h = _sha512_modq(R + A + msg)
    s = (r + h * a) % Q
    return R + int.to_bytes(s, 32, "little")


def verify(pub, msg, sig):
    A = _decompress(pub)
    R = _decompress(sig[:32])
    if A is None or R is None:
        return False
    s = int.from_bytes(sig[32:], "little")
    if s >= Q:
        return False
    h = _sha512_modq(sig[:32] + pub + msg)
    sB = _mul(s, G)
    hA = _mul(h, A)
    lhs, rhs = sB, _add(R, hA)
    return (lhs[0] * rhs[2] - rhs[0] * lhs[2]) % P == 0 and (lhs[1] * rhs[2] - rhs[1] * lhs[2]) % P == 0


def x25519(k, u):
    k = bytearray(k)
    k[0] &= 248
    k[31] &= 127
    k[31] |= 64
    k = int.from_bytes(k, "little")
    u = int.from_bytes(u, "little") & ((1 << 255) - 1)
    x1, x2, z2, x3, z3, swap = u, 1, 0, u, 1, 0
    for t in range(254, -1, -1):
        kt = (k >> t) & 1
        swap ^= kt
        if swap:
            x2, x3, z2, z3 = x3, x2, z3, z2
        swap = kt
        A = (x2 + z2) % P
        AA = A * A % P
        B = (x2 - z2) % P
        BB = B * B % P
        E = (AA - BB) % P
        C = (x3 + z3) % P
        Dd = (x3 - z3) % P
        DA = Dd * A % P
        CB = C * B % P
        x3 = (DA + CB) ** 2 % P
        z3 = x1 * (DA - CB) ** 2 % P
        x2 = AA * BB % P
        z2 = E * (AA + 121665 * E) % P
    if swap:
        x2, z2 = x3, z3
    return int.to_bytes(x2 * pow(z2, P - 2, P) % P, 32, "little")


def ecdh_orlp(prv, peer_pub):
    """key_exchange.c : u = (1+y)/(1-y) depuis le y Ed25519 du pair, X25519 avec prv[0..32]."""
    y = int.from_bytes(peer_pub, "little") & ((1 << 255) - 1)
    u = (1 + y) * pow((1 - y) % P, P - 2, P) % P
    return x25519(prv[:32], int.to_bytes(u, 32, "little"))


# ---------------------------------------------------------------------------------------------
# Auto-vérification des primitives contre leurs normes


def _selftest():
    k = bytes(range(16))
    pt = bytes.fromhex("00112233445566778899aabbccddeeff")
    ct = aes_encrypt_block(k, pt)
    assert ct.hex() == "69c4e0d86a7b0430d8cdb78070b4c55a", ct.hex()  # FIPS-197 C.1
    assert aes_decrypt_block(k, ct) == pt
    seed = bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
    prv = expanded_from_seed(seed)
    pub = public_from_expanded(prv)
    assert pub.hex() == "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"  # RFC 8032 test 1
    sig = sign_expanded(prv, b"")
    assert sig.hex() == ("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b")
    assert verify(pub, b"", sig)
    out = x25519(bytes.fromhex("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4"),
                 bytes.fromhex("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c"))
    assert out.hex() == "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552"  # RFC 7748 §5.2
    # L'ECDH est symétrique.
    a = expanded_from_seed(b"\x01" * 32)
    b = expanded_from_seed(b"\x02" * 32)
    assert ecdh_orlp(a, public_from_expanded(b)) == ecdh_orlp(b, public_from_expanded(a))


# ---------------------------------------------------------------------------------------------
# MeshCore


def seal(secret, plain):
    """MAC(2) ‖ AES-128-ECB(clair complété par des zéros) ; clé AES = secret[0..16]."""
    padded = plain + b"\x00" * (-len(plain) % 16)
    body = b"".join(aes_encrypt_block(secret[:16], padded[i:i + 16]) for i in range(0, len(padded), 16))
    mac = hmac.new(secret, body, hashlib.sha256).digest()[:2]
    return mac + body


def open_(secret, sealed):
    mac, body = sealed[:2], sealed[2:]
    assert hmac.new(secret, body, hashlib.sha256).digest()[:2] == mac
    return b"".join(aes_decrypt_block(secret[:16], body[i:i + 16]) for i in range(0, len(body), 16))


def channel_hash(secret32):
    key = secret32[:16] if secret32[16:] == b"\x00" * 16 else secret32
    return hashlib.sha256(key).digest()[0]


def region_key(name):
    return hashlib.sha256(b"#" + name.lstrip("#").encode()).digest()[:16]


def transport_code(key, ptype, payload):
    c = int.from_bytes(hmac.new(key, bytes([ptype]) + payload, hashlib.sha256).digest()[:2], "little")
    return {0: 1, 0xFFFF: 0xFFFE}.get(c, c)


def ack_crc(ts, flags, text, sender_pub):
    return int.from_bytes(hashlib.sha256(ts.to_bytes(4, "little") + bytes([flags]) + text + sender_pub).digest()[:4], "little")


def packet_hash(ptype, path_len, payload):
    h = bytes([ptype])
    if ptype == 0x9:
        h += path_len.to_bytes(2, "little")
    return hashlib.sha256(h + payload).digest()[:8]


def frame(route, ptype, path_len, path, payload, transport=None):
    out = bytes([(ptype << 2) | route])
    if transport is not None:
        out += transport[0].to_bytes(2, "little") + transport[1].to_bytes(2, "little")
    return out + bytes([path_len]) + path + payload


def app_data(kind, name=None, loc=None):
    flags = kind
    body = b""
    if loc is not None:
        flags |= 0x10
        body += loc[0].to_bytes(4, "little", signed=True) + loc[1].to_bytes(4, "little", signed=True)
    if name:
        n = name.encode()[: 32 - 1 - len(body)]
        while True:
            try:
                n.decode()
                break
            except UnicodeDecodeError:
                n = n[:-1]
        if n:
            flags |= 0x80
            body += n
    return bytes([flags]) + body


# ---------------------------------------------------------------------------------------------
# Sortie TOML


def h(b):
    return b.hex()


def ident(label):
    """Identité de test : la première graine dérivée du libellé dont la clé ne commence ni par
    0x00 ni par 0xFF (clés refusées par MeshCore)."""
    i = 0
    while True:
        seed = hashlib.sha256(f"oxymesh-test-{label}-{i}".encode()).digest()
        prv = expanded_from_seed(seed)
        pub = public_from_expanded(prv)
        if pub[0] not in (0x00, 0xFF):
            return seed, prv, pub
        i += 1


def main():
    _selftest()
    out = []
    w = out.append
    w("# Vecteurs MeshCore sur l'air, produits par testdata/air/gen.py (oracle indépendant).")
    w("# Ne pas éditer à la main : relancer `python3 -I testdata/air/gen.py > testdata/air/vectors.toml`.")
    w("")

    ids = {k: ident(k) for k in ("alice", "bob", "relay")}
    for name, (seed, prv, pub) in ids.items():
        w("[[identity]]")
        w(f'name = "{name}"')
        w(f'seed = "{h(seed)}"')
        w(f'prv = "{h(prv)}"')
        w(f'pub = "{h(pub)}"')
        w("")

    a_prv, a_pub = ids["alice"][1], ids["alice"][2]
    b_prv, b_pub = ids["bob"][1], ids["bob"][2]
    r_prv, r_pub = ids["relay"][1], ids["relay"][2]
    ab = ecdh_orlp(a_prv, b_pub)
    assert ab == ecdh_orlp(b_prv, a_pub)
    w("[[ecdh]]")
    w('a = "alice"')
    w('b = "bob"')
    w(f'secret = "{h(ab)}"')
    w("")
    ar = ecdh_orlp(a_prv, r_pub)

    for plain in (b"", b"x", bytes(range(16)), bytes(range(40))):
        if not plain:
            continue
        w("[[seal]]")
        w(f'secret = "{h(ab)}"')
        w(f'plain = "{h(plain)}"')
        w(f'sealed = "{h(seal(ab, plain))}"')
        w("")

    def packet(name, raw, route, ptype, path_len, path, payload, transport=None, note=""):
        w("[[packet]]")
        w(f'name = "{name}"')
        if note:
            w(f'note = "{note}"')
        w(f'raw = "{h(raw)}"')
        w(f"route = {route}")
        w(f"ptype = {ptype}")
        w(f"path_len = {path_len}")
        w(f'path = "{h(path)}"')
        w(f'payload = "{h(payload)}"')
        if transport is not None:
            w(f"transport = [{transport[0]}, {transport[1]}]")
        w(f'hash = "{h(packet_hash(ptype, path_len, payload))}"')
        w("")

    # Annonces
    ts = 1793900000
    for name, who, ad, path_len, path in (
        ("advert_alice", "alice", app_data(1, "Alice"), 0, b""),
        ("advert_bob_loc", "bob", app_data(1, "Bob café", (48856613, 2352222)), 0, b""),
        ("advert_relay_2hops", "relay", app_data(2, "Relais Montmartre ⛰ très long nom"), 2, b"\x12\x34"),
    ):
        prv, pub = ids[who][1], ids[who][2]
        msg = pub + ts.to_bytes(4, "little") + ad
        sig = sign_expanded(prv, msg)
        assert verify(pub, msg, sig)
        payload = pub + ts.to_bytes(4, "little") + sig + ad
        raw = frame(1, 4, path_len, path, payload)
        packet(name, raw, 1, 4, path_len, path, payload)
        w("[[advert]]")
        w(f'packet = "{name}"')
        w(f'identity = "{who}"')
        w(f"ts = {ts}")
        w(f'app_data = "{h(ad)}"')
        w(f"kind = {ad[0] & 0x0F}")
        if ad[0] & 0x10:
            w(f"lat = {int.from_bytes(ad[1:5], 'little', signed=True)}")
            w(f"lon = {int.from_bytes(ad[5:9], 'little', signed=True)}")
        name_bytes = ad[9 if ad[0] & 0x10 else 1:] if ad[0] & 0x80 else b""
        w(f'adv_name = "{h(name_bytes)}"')
        w("")

    # Messages directs
    for name, attempt, text, route, path_len, path in (
        ("txt_flood", 0, "Hello Bob", 1, 0, b""),
        ("txt_attempt5", 5, "Salut, ça va ?", 1, 0, b""),
        ("txt_direct", 1, "par le chemin direct", 2, 2, b"\xab\xcd"),
    ):
        t = text.encode()
        flags = attempt & 3
        plain = ts.to_bytes(4, "little") + bytes([flags]) + t
        if attempt > 3:
            plain += b"\x00" + bytes([attempt])
        payload = bytes([b_pub[0], a_pub[0]]) + seal(ab, plain)
        raw = frame(route, 2, path_len, path, payload)
        packet(name, raw, route, 2, path_len, path, payload)
        w("[[txt]]")
        w(f'packet = "{name}"')
        w('from = "alice"')
        w('to = "bob"')
        w(f"ts = {ts}")
        w(f"attempt = {attempt}")
        w(f'text = "{h(t)}"')
        w(f'plain = "{h(plain)}"')
        w(f"ack = {ack_crc(ts, flags, t, a_pub)}")
        w("")

    # Canaux
    public = bytes.fromhex("8b3387e9c5cdea6ac9e5edbaa115cd72") + b"\x00" * 16
    test_ch = hashlib.sha256(b"#test").digest()[:16] + b"\x00" * 16
    fr = region_key("fr")
    for name, secret, ptype, plain, transport_region in (
        ("grp_txt_public", public, 5, ts.to_bytes(4, "little") + b"\x00" + "Alice: Bonjour à tous".encode(), None),
        ("grp_data_test", test_ch, 6, (0xFF4C).to_bytes(2, "little") + bytes([20]) + bytes(range(100, 120)), None),
        ("grp_txt_public_fr", public, 5, ts.to_bytes(4, "little") + b"\x00" + b"Alice: dans la portee fr", "fr"),
    ):
        payload = bytes([channel_hash(secret)]) + seal(secret, plain)
        if transport_region:
            code = transport_code(region_key(transport_region), ptype, payload)
            raw = frame(0, ptype, 0, b"", payload, (code, 0))
            packet(name, raw, 0, ptype, 0, b"", payload, (code, 0))
        else:
            raw = frame(1, ptype, 0, b"", payload)
            packet(name, raw, 1, ptype, 0, b"", payload)
        w("[[group]]")
        w(f'packet = "{name}"')
        w(f'secret = "{h(secret)}"')
        w(f"channel_hash = {channel_hash(secret)}")
        w(f'plain = "{h(plain)}"')
        if transport_region:
            w(f'region = "{transport_region}"')
            w(f'region_key = "{h(region_key(transport_region))}"')
        w("")

    # PATH de retour, avec accusé de 6 octets, chemin de 2 sauts en hashes de 2 octets
    ack6 = ack_crc(ts, 0, b"Hello Bob", a_pub).to_bytes(4, "little") + b"\x00\x5a"
    in_path_len = (1 << 6) | 2
    in_path = b"\x12\x34\x56\x78"
    plain = bytes([in_path_len]) + in_path + bytes([3]) + ack6
    payload = bytes([a_pub[0], b_pub[0]]) + seal(ab, plain)
    raw = frame(1, 8, 0, b"", payload)
    packet("path_return_ack", raw, 1, 8, 0, b"", payload)
    w("[[path]]")
    w('packet = "path_return_ack"')
    w(f"in_path_len = {in_path_len}")
    w(f'in_path = "{h(in_path)}"')
    w("extra_type = 3")
    w(f'extra = "{h(ack6)}"')
    w(f'plain = "{h(plain)}"')
    w("")

    # ACK direct de 6 octets, puis MULTIPART-ACK
    packet("ack_direct", frame(2, 3, 1, b"\xa1", ack6), 2, 3, 1, b"\xa1", ack6)
    mp = bytes([(1 << 4) | 3]) + ack6[:4]
    packet("multipart_ack", frame(2, 0xA, 0, b"", mp), 2, 0xA, 0, b"", mp)

    # ANON_REQ : connexion à un relais
    plain = ts.to_bytes(4, "little") + b"password"
    payload = bytes([r_pub[0]]) + a_pub + seal(ar, plain)
    packet("anon_login", frame(1, 7, 0, b"", payload), 1, 7, 0, b"", payload)
    w("[[anon]]")
    w('packet = "anon_login"')
    w('from = "alice"')
    w('to = "relay"')
    w(f'secret = "{h(ar)}"')
    w(f'plain = "{h(plain)}"')
    w("")

    # TRACE : le hash couvre path_len
    trace = (0x01020304).to_bytes(4, "little") + (0xA0B0C0D0).to_bytes(4, "little") + b"\x00" + b"\x11\x22\x33"
    packet("trace_one_snr", frame(2, 9, 1, b"\x28", trace), 2, 9, 1, b"\x28", trace,
           note="1 saut parcouru, SNR 10 dB")

    # CONTROL : découverte zéro saut
    ctrl = bytes([0x80, 1 << 2]) + (0xDEADBEEF).to_bytes(4, "little")
    packet("control_discover", frame(2, 0xB, 0, b"", ctrl), 2, 0xB, 0, b"", ctrl)

    # Codes de portée sur des charges arbitraires
    for region in ("fr", "#fr", "be"):
        for ptype, payload in ((5, b"\x01\x02\x03"), (4, bytes(range(100)))):
            w("[[transport]]")
            w(f'region = "{region}"')
            w(f"ptype = {ptype}")
            w(f'payload = "{h(payload)}"')
            w(f"code = {transport_code(region_key(region), ptype, payload)}")
            w("")

    # Trames refusées par Dispatcher::tryParsePacket
    for name, raw in (
        ("version_1", bytes([0x40 | (2 << 2) | 1, 0]) + b"x"),
        ("path_hash_size_4", bytes([(2 << 2) | 1, 0xC1, 0, 0, 0, 0, 0])),
        ("path_too_long", bytes([(2 << 2) | 1, (1 << 6) | 40]) + bytes(80) + b"x"),
        ("path_truncated", bytes([(2 << 2) | 1, 5, 1, 2])),
        ("transport_truncated", bytes([(5 << 2) | 0, 1, 2, 3])),
        ("payload_185", bytes([(5 << 2) | 1, 0]) + bytes(185)),
        ("empty", b""),
    ):
        w("[[reject]]")
        w(f'name = "{name}"')
        w(f'raw = "{h(raw)}"')
        w("")

    sys.stdout.write("\n".join(out))


if __name__ == "__main__":
    main()
