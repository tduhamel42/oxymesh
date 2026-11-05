# Trames MeshCore sur l'air (v1.18)

Source de vérité pour `mc-air` et la couche mesh de `mc-node`. Référence : MeshCore `companion-v1.18.0`, commit 81dbae5 (ADR-0001). Les citations `fichier:ligne` renvoient à ce commit. Entiers en **petit-boutiste** sauf mention contraire.

Abréviations : `rpt` = `examples/simple_repeater/MyMesh.cpp`, `comp` = `examples/companion_radio/MyMesh.cpp`, `room` = `examples/simple_room_server/MyMesh.cpp`, `BCM` = `src/helpers/BaseChatMesh.cpp`.

## 1. Trame

```
en-tête(1) | [tc0 u16 | tc1 u16]  (routes 0 et 3 seulement) | path_len(1) | chemin | charge utile (le reste)
```

- Pas de champ de longueur : la charge utile est ce qui reste de la trame LoRa.
- Trame ≤ 255 octets (`MAX_TRANS_UNIT`, `src/MeshCore.h:23`) ; charge utile ≤ 184 (`MAX_PACKET_PAYLOAD`, `:20`) ; chemin ≤ 64 octets (`MAX_PATH_SIZE`, `:22`).
- Réception (`Dispatcher::tryParsePacket`, `src/Dispatcher.cpp:149-189`) :
  - version de l'en-tête > 0 → rejet ;
  - taille de hash codée 3 (4 octets) → rejet ;
  - octets de chemin > 64 ou au-delà de la trame → rejet ;
  - charge utile > 184 → rejet ;
  - **une charge utile vide est acceptée** à ce niveau ; chaque type impose ensuite sa longueur minimale (§3).

### 1.1 En-tête `0bVVPPPPRR` (`src/Packet.h:8-37`)

| Bits | Champ | Valeurs |
|---|---|---|
| 0-1 | route | 0 TRANSPORT_FLOOD, 1 FLOOD, 2 DIRECT, 3 TRANSPORT_DIRECT |
| 2-5 | type de charge utile | tableau ci-dessous |
| 6-7 | version | 0 seulement (`PAYLOAD_VER_1` : hashes d'1 octet, MAC de 2 octets) |

| Type | Nom | Type | Nom |
|---|---|---|---|
| 0x0 | REQ | 0x8 | PATH |
| 0x1 | RESPONSE | 0x9 | TRACE |
| 0x2 | TXT_MSG | 0xA | MULTIPART |
| 0x3 | ACK | 0xB | CONTROL |
| 0x4 | ADVERT | 0xC-0xE | non attribués |
| 0x5 | GRP_TXT | 0xF | RAW_CUSTOM |
| 0x6 | GRP_DATA | | |
| 0x7 | ANON_REQ | | |

L'en-tête 0xFF sert en interne de marque « ne pas relayer » (`Packet.h`, `markDoNotRetransmit`) : jamais émis.

### 1.2 `path_len` (`src/Packet.h:79-83`, `src/Packet.cpp:9-30`)

- Bits 0-5 : nombre de sauts (0-63). Bits 6-7 : taille d'un hash de chemin moins 1 (1, 2 ou 3 octets ; 3 → 4 octets est réservé et rejeté).
- Longueur du chemin = nombre × taille, ≤ 64. Limites pratiques : 63 sauts en 1 octet, 32 en 2, 21 en 3.
- `0xFF` = « chemin inconnu » dans l'état des contacts et le protocole compagnon (`OUT_PATH_UNKNOWN`, `src/helpers/ContactInfo.h:6`) ; jamais valide sur l'air.
- Le firmware d'avant 1.14 lit `path_len` comme un nombre d'octets brut : une trame en hashes de 2 ou 3 octets lui paraît trop longue et il la jette. Par défaut on émet en hashes d'1 octet (`path_hash_mode` 0) et on accepte les trois tailles.
- **Débordement à corriger** : avec des hashes d'1 octet et 63 sauts, le C++ ajoute encore un saut et écrit 0x40 (lu comme « 2 octets, 0 saut ») (`src/Mesh.cpp:347-350`). Nous plafonnons à 63 sauts.

### 1.3 Codes de transport (routes 0 et 3)

- `tc0` = code de portée ; `tc1` réservé, toujours émis à 0 (rpt:1003-1006, comp:495-498).
- `code = u16le(HMAC-SHA256(clé16, [type] ‖ charge utile)[0..2])` ; 0x0000 devient 0x0001 et 0xFFFF devient 0xFFFE (`src/helpers/TransportKeyStore.cpp:4-18`).
- Ne couvre ni le chemin ni l'en-tête : il survit au relais.
- Clé d'une région : `SHA256("#" ‖ nom)[0..16]` ; « fr » et « #fr » donnent la même clé (`src/helpers/RegionMap.cpp:173-188`, `TransportKeyStore.cpp:44-47`). Les régions privées `$nom` ne sont pas implémentées en v1.18 (`TransportKeyStore.cpp:52-68`).
- `{0, 0}` en TRANSPORT_DIRECT zéro saut veut dire « partage local » (`BCM:578-580`).

### 1.4 Hash anti-doublon (`src/Packet.cpp:41-50`)

`SHA256([type] ‖ (TRACE seulement : path_len en u16 LE, 2 octets) ‖ charge utile)[0..8]`. L'en-tête (route, version), le chemin et les codes de transport n'entrent pas dans le hash.

## 2. Crypto

| Élément | Règle exacte |
|---|---|
| Ed25519 | orlp/ed25519 (`lib/ed25519`). Vérification par rweather `Ed25519::verify` (`src/Identity.cpp:38`). Nous : ed25519-dalek `verify` (non strict) |
| Clé privée | **64 octets « étendus » orlp** : `SHA512(graine)`, scalaire clampé `[0..32]` puis préfixe de nonce `[32..64]` (`lib/ed25519/keypair.c:6-16`). La graine n'est pas gardée. Fichier d'identité : pub(32) ‖ prv(64) (`src/Identity.cpp:111-149`) |
| Signature | Ed25519 standard avec `prv[32..64]` comme préfixe de nonce (`lib/ed25519/sign.c:14-30`). Nous : `hazmat::ExpandedSecretKey` + `raw_sign` |
| Clés interdites | `pub[0]` ∈ {0x00, 0xFF} → clé régénérée (`src/Identity.cpp:71-72`, comp:951) |
| ECDH | `x25519(clamp(prv[0..32]), u)` avec `u = (1+y)/(1−y)` tiré du `y` Ed25519 du pair (bit de signe ignoré, point non validé) (`lib/ed25519/key_exchange.c:20-78`). Secret de 32 octets, calculé une fois par contact et gardé (`src/helpers/ContactInfo.h:21-27`) |
| Chiffrement | AES-128-**ECB**, clé = `secret[0..16]`, dernier bloc complété par des zéros, pas de bloc en plus si aligné (`src/Utils.cpp:85-125`) |
| MAC | `HMAC-SHA256(clé = secret[0..32], chiffré)[0..2]`, placé **devant** le chiffré (`src/Utils.cpp:127-145`). Réception : chiffré non multiple de 16 → rejet ; MAC faux → rejet (`:147-175`) |
| Secret de canal | tampon de 32 octets ; une PSK de 16 octets est complétée par des zéros (équivalent pour HMAC). `hash = SHA256(secret[0..16 ou 32])[0]`, 16 si les octets 16..32 sont nuls (`BCM:911-946`). Canal Public : `8b3387e9c5cdea6ac9e5edbaa115cd72` (comp:114). Canal « #nom » : `SHA256("#nom")[0..16]`, dérivé par l'app |

Bloc chiffré, partout : `MAC(2) ‖ AES-ECB(texte clair complété)`.

## 3. Charges utiles

Les hashes source et destination font **toujours 1 octet** (`PATH_HASH_SIZE`, `src/MeshCore.h:18`), quelle que soit la taille des hashes de chemin. Le hash d'une identité est `pub[0]`.

| Type | Clair | Chiffré | Réception |
|---|---|---|---|
| REQ, RESPONSE, TXT_MSG, PATH | `dest(1) src(1)` | bloc chiffré, clé = ECDH | longueur > 4 (`src/Mesh.cpp:139`). Si `dest == self.pub[0]`, on essaie chaque contact avec `pub[0] == src` (24 au plus, `BCM:203-211`) ; le premier MAC valide gagne (`src/Mesh.cpp:147-185`) |
| ANON_REQ | `dest(1) pub_émetteur(32)` | bloc chiffré, clé = ECDH(nous, pub_émetteur) | longueur > 35 (`src/Mesh.cpp:197-224`). La clé est l'identité **statique** de l'émetteur (le commentaire « ephemeral » de `Packet.h:26` est faux) |
| GRP_TXT, GRP_DATA | `hash_canal(1)` | bloc chiffré, clé = secret du canal | essaie jusqu'à 4 canaux de même hash (`src/Mesh.cpp:225-251`) ; les canaux sans nom sont ignorés (`BCM:401`) |
| ACK | `crc(4)` puis octets libres | — | lit 4 octets (`src/Mesh.cpp:117-129`). Longueurs émises : 4 (serveurs), 6 (compagnons : crc, octet d'essai étendu, octet aléatoire, `BCM:244-256`), 5 (accusé de keep-alive d'un salon, room:571-574) |
| ADVERT | `pub(32) ts u32 sig(64) app_data(≤ 32)` | — | §5 |
| TRACE | `tag u32 auth u32 flags u8 hashes…` | — | §4.6 |
| MULTIPART | `(restants << 4 \| type_interne)(1) contenu…` | — | seul le type interne ACK existe ; ≥ 5 octets (`src/Mesh.cpp:300-324, 574-587`) |
| CONTROL | `flags(1) données` (sous-type = quartet haut) | — | non chiffré. `flags & 0x80` = zéro saut direct seulement, jamais relayé (`src/Mesh.cpp:70-76`) |
| RAW_CUSTOM | opaque | — | livré seulement en direct à 0 saut ; jamais relayé par inondation (`src/Mesh.cpp:292-299`) |

### 3.1 Textes clairs

**TXT_MSG** (`BCM:451-471`) : `ts u32 ‖ (txt_type << 2 | essai & 3) ‖ texte` (sans NUL ; le bourrage de zéros vient du chiffrement). Si `essai > 3`, on ajoute `0x00 ‖ essai u8` après le texte, et le texte est limité à 158 octets (160 sinon, `MAX_TEXT_LEN`, `src/helpers/BaseChatMesh.h:8`).

| txt_type | Sens |
|---|---|
| 0 PLAIN | texte, accusé |
| 1 CLI_DATA | sortie de commande, pas d'accusé |
| 2 SIGNED_PLAIN | `préfixe_pub_auteur(4) ‖ texte`, émis par les serveurs de salon (room:69-91) |
| 3 CLI_COMMAND | nouveau en v1.18 |

**REQ** : `tag/ts u32 ‖ type ‖ paramètres`.
- Types des relais (rpt:46-51) : 1 GET_STATUS, 2 KEEP_ALIVE, 3 TELEMETRY, 5 GET_ACL, 6 NEIGHBOURS, 7 OWNER_INFO.
- Requête générique de 13 octets : `tag u32, type, 00 00 00 00, aléa 4` (`BCM:693-700`). Keep-alive : `ts, 0x02, sync_since u32` (`BCM:814-818`).

**RESPONSE** : `tag u32` (l'horodatage de la requête, renvoyé) `‖ corps` (rpt:214).
- Connexion acceptée, 13 octets : `ts_serveur u32, 0x00, 0x00 (ancien), est_admin, permissions, aléa 4, niveau_firmware = 2` (rpt:135-144, room:369-376).
- Ancienne réponse de connexion : `"OK"` en position 4 (comp:699-703).

**PATH** (`src/Mesh.cpp:443-486`, décodage `:160-172`) : `path_len (codé) ‖ chemin ‖ type_extra ‖ extra`.
- Seul le quartet bas de `type_extra` est lu.
- Sans extra : `0xFF` puis 4 octets aléatoires (rend le hash unique).
- Extra = ACK (4 ou 6 octets) ou RESPONSE.
- Refusé à la création si `octets_chemin + longueur_extra + 5 > 166`.

**ANON_REQ** (`BCM:603-661`, rpt:582-592) :
- connexion à un relais ou capteur : `ts ‖ mot de passe (≤ 15 octets)` ;
- connexion à un salon : `ts ‖ sync_since u32 ‖ mot de passe` ;
- requêtes anonymes à un relais (acceptées seulement arrivées en direct) : `ts ‖ type (1 régions, 2 propriétaire, 3 horloge) ‖ reply_path_len ‖ reply_path` ; le relais lit `data[4] == 0 || data[4] ≥ 0x20` comme un mot de passe.

**GRP_TXT** : `ts u32 ‖ 0x00 ‖ "nom: texte"` ; rejeté si `(pt[4] >> 2) != 0` (`BCM:410-430, 518-537`).

**GRP_DATA** : `data_type u16 ‖ longueur u8 ‖ données` (`BCM:431-448, 539-555`) ; données ≤ 165 octets (`MAX_GROUP_DATA_LENGTH`).

**DISCOVER** (CONTROL, rpt:786-860) :
- requête : `0x80 | préfixe_seul, filtre_type (1 << ADV_TYPE), tag u32, [since u32]` ;
- réponse : `0x90 | type_nœud, snr×4 (i8), tag u32, clé publique (8 ou 32 octets)`, émise en zéro saut après `délai_relais × 4` ; au plus 4 réponses par 120 s.

### 3.2 Accusés (CRC d'accusé)

`SHA256(ts u32 ‖ flags ‖ texte sans NUL ‖ pub de l'ÉMETTEUR)[0..4]`, gardé comme un u32 petit-boutiste et émis tel quel.
- L'émetteur le calcule avec sa propre clé (`BCM:462`), le destinataire avec celle de l'émetteur (`BCM:245`).
- La queue d'essai étendu (`0x00 ‖ essai`) n'y entre pas ; seul `essai & 3` compte.
- SIGNED_PLAIN : `SHA256(pt[0..9+strlen] ‖ pub du DESTINATAIRE)` (`BCM:298`, room:88).
- Keep-alive : `SHA256(requête de 9 octets ‖ pub du client)` (`BCM:821`, room:569).

## 4. Routage

### 4.1 Réception (`src/Mesh.cpp:41-332`), dans l'ordre

1. TRACE.
2. CONTROL avec le bit 0x80.
3. Direct avec un chemin non vide : relais seulement, avec lecture au vol des accusés (`:78-110`).
4. Inondation : filtre `filterRecvFloodPacket`.
5. Aiguillage par type. Un type inconnu n'est pas relayé (`:326-329`).

### 4.2 Relais direct

- Si `chemin[0..taille] == self.pub[0..taille]` et `allow_forward` : on retire la première entrée (décalage, `:334-342`) et on remet en file en priorité 0 après le délai direct.
- Les ACK et MULTIPART-ACK ne sont pas relayés tels quels : on fabrique de nouveaux ACK, plus `multi_acks` copies MULTIPART espacées de 300 ms (`:91-99, 359-402`).
- Le destinataire reçoit la trame avec un chemin vide et la traite normalement.

### 4.3 Relais en inondation (`routeRecvPacket`, `:344-357`)

- Conditions : pas marquée « ne pas relayer », `(n+1) × taille ≤ 64`, `allow_forward`.
- On ajoute `self.pub[0..taille]` et on met en file en priorité = nouveau nombre de sauts, après le délai d'inondation.
- Une trame qui se déchiffre pour nous est marquée « ne pas relayer » (`:188, 218` ; `BCM:376`).
- Une annonce n'est relayée que si sa signature est valide (`:282-288`).

### 4.4 Anti-doublon (`src/helpers/SimpleMeshTables.h:9-57`)

Anneau de 160 hashes de 8 octets, vérifié **avant** le déchiffrement. Nos propres émissions y sont inscrites (sendFlood, sendDirect, sendZeroHop).

### 4.5 Délais, files, duty cycle

| Qui | Délai d'inondation | Délai direct |
|---|---|---|
| compagnon, relais, salon | `rand[0, 5·t]` avec `t = airtime(octets_chemin + charge + 2) × f_txdelay` (0,5), calculé après ajout de soi | même formule avec `d_txdelay` = 0,2 (compagnon, salon) ou 0,3 (relais), calculé avant retrait de soi (comp:278-285, rpt:547-554) |

- `nextInt(a, b) = rand32 % (b − a) + a` (`src/Utils.cpp:18-22`).
- Délai de score à la réception : désactivé partout (`rx_delay_base = 0`, comp:273-276).
- Priorités (la plus petite d'abord parmi les trames dues, puis ordre d'arrivée ; `src/helpers/StaticPoolPacketManager.cpp:22-44`, `src/Mesh.cpp:653-725`) :

| Trame | Priorité |
|---|---|
| sendDirect | 0 (PATH : 1, TRACE : 5) |
| sendZeroHop | 0 |
| sendFlood à nous | 1 (PATH : 2, ADVERT : 3) |
| inondation relayée | nombre de sauts |
| TRACE relayée | 5 |

- Duty cycle (`src/Dispatcher.cpp:19-53, 92-106, 275-286`) : budget = fenêtre d'1 h × 1/(1 + af) (af = 1 → 50 %). Une émission demande un budget ≥ airtime(255)/2 ; sous 100 ms de budget, on attend. Délai de garde d'émission : 1,5 × airtime (`:327-339`). **Chez nous, la borne réglementaire de `config/eu868.toml` s'applique en plus** (g3 : 10 %).
- Écoute avant émission (`src/Dispatcher.cpp:289-305`) : canal occupé si préambule ou en-tête détecté, si RSSI > plancher + `interference_threshold` (0 = désactivé par défaut), ou si CAD matériel (désactivé par défaut). Occupé : nouvel essai après 120, 240 ou 360 ms ; au bout de 4 s, on émet quand même (`ERR_EVENT_CAD_TIMEOUT`).

### 4.6 Qui relaie (`allow_forward`)

| Nœud | Par défaut | Règles |
|---|---|---|
| Compagnon | **non** (`repeat.disable_fwd = 1`) | s'il est activé : sans limite de sauts ni de région ; activable seulement sur 433,000, 869,495 ou 918,000 MHz (comp:487-489, 1032-1040, 1439-1447) |
| Relais | oui | limites `flood_max` 64, `flood_max_unscoped` 64, `flood_max_advert` 8 (sauts avant ajout) ; région ; détection de boucle (rpt:396-459, `src/helpers/RoutingPolicy.h:15-22`) |
| Salon, capteur | non | |

Portée par région (relais, rpt:556-569) : TRANSPORT_FLOOD doit correspondre à une région sans `DENY_FLOOD` ; FLOOD simple seulement si la région `*` le permet ; le direct n'est jamais filtré. Rien de cela ne touche la livraison locale.

### 4.7 Apprentissage du chemin

1. Une REQ, un TXT ou une ANON_REQ arrive par inondation chez B. B inonde (dans la portée) un PATH qui contient le chemin d'arrivée, avec l'ACK ou la réponse en extra (`BCM:250-254, 316-320`).
2. A garde ce chemin comme `out_path` du contact.
3. Un compagnon renvoie alors un PATH réciproque **en direct** le long de ce chemin, après 500 ms (`src/Mesh.cpp:173-178`) ; les serveurs non (rpt:782-783).
4. Si un ACK ou une RESPONSE arrive par inondation alors qu'un chemin direct est connu, on renvoie un PATH en direct après 3 s (`BCM:378-390`).

### 4.8 TRACE (`src/Mesh.cpp:42-68, 605-619, 698-704`)

- L'initiateur émet en direct, `path_len = 0`, la liste des hashes dans la charge utile. TRACE en inondation refusée (`:638-641`).
- À chaque saut : `off = path_len << (flags & 3)`. Si `off ≥ longueur des hashes`, la trace est finie (`onTraceRecv`). Sinon, si `hashes[off .. off + (1 << sz)]` est à nous, qu'on peut relayer et que la trame est nouvelle : on ajoute `i8(snr × 4)` au champ chemin de l'en-tête, on incrémente `path_len`, priorité 5.
- Les hashes de trace font 1, 2, 4 ou 8 octets (pas 1/2/3).

## 5. Annonce (ADVERT)

- Octets signés : `pub(32) ‖ ts u32 ‖ app_data` (`src/Mesh.cpp:404-438`). À la réception, `app_data` est tronqué à 32 octets **avant** la vérification (`:269-281`).
- `app_data` (`src/helpers/AdvertDataHelpers.h:7-17`, `.cpp:4-69`), dans l'ordre :

| Champ | Présence | Format |
|---|---|---|
| flags | toujours | quartet bas = type (0 aucun, 1 chat, 2 relais, 3 salon, 4 capteur) ; 0x10 position ; 0x20 feat1 ; 0x40 feat2 ; 0x80 nom |
| lat, lon | 0x10 | i32 chacun, degrés × 1e6 tronqués |
| feat1, feat2 | 0x20, 0x40 | u16 chacun |
| nom | 0x80 | le reste, sans NUL, coupé sur une frontière UTF-8 |

- Un compagnon ignore une annonce sans nom (`BCM:124-127`).
- `ts` = heure RTC en secondes Unix (`src/Mesh.cpp:418`). Anti-rejeu : un compagnon rejette `ts ≤ dernier ts` pour un contact connu (`BCM:133-136`).
- Le compagnon n'annonce pas au démarrage ; il annonce sur demande de l'app (`SEND_SELF_ADVERT`), en zéro saut ou en inondation dans la portée par défaut.

## 6. Radio

| Paramètre | Valeur | Source |
|---|---|---|
| Fréquence / BW / SF / CR par défaut du build | 869,618 MHz / 62,5 kHz / SF8 / CR 4/5 | `platformio.ini:29-31`, `src/helpers/radiolib/CustomSX1262.h:27-31` |
| Région France (préréglage de l'app, meshcore.fr) | 869,618 / 62,5 / SF8 / **CR 4/8** | `config/regions.toml` |
| Mot de synchronisation | « privé » RadioLib (0x12, registres SX126x 0x0740-0x0741 = 0x14, 0x24) | `CustomSX1262.h:56` |
| Préambule | 32 symboles si SF ≤ 8, sinon 16 | `RadioLibWrappers.h:56`, `CustomSX1262Wrapper.h:20` |
| CRC | activé | `CustomSX1262.h:69` |
| En-tête, IQ, LDRO | défauts RadioLib : explicite, IQ normal, LDRO automatique (symbole ≥ 16 ms) | à confirmer contre le Heltec en MC1 |
| Gain RX renforcé | activé | comp:936-941 |
| SNR | SNR de la trame × 4, en i8 | `src/Dispatcher.cpp:206` |
| Plancher de bruit | moyenne de 64 RSSI instantanés au repos, en gardant ceux < plancher + 14, plancher ≥ −120 ; recalé toutes les 2 s | `RadioLibWrappers.cpp:90-110`, `src/Dispatcher.cpp:15-17, 67-71` |

## 7. Compatibilité avec les versions anciennes

- Version d'en-tête ≠ 0 : rejet.
- Hashes de chemin de 2 ou 3 octets : acceptés, jamais émis par défaut (§1.2).
- `flags & 3` de TRACE (taille des hashes) : depuis v1.11.
- TXT type 3 : depuis v1.18.
- ACK : accepter ≥ 4 octets, ne lire que les 4 premiers.
- Réponse de connexion « OK » (ancienne) et keep-alive (REQ 2, obsolète mais encore servi par les salons).
- Commande CLI en texte simple avec accusé (ancien) ; les serveurs répondent maintenant en CLI_DATA (rpt:717-731, 749).

## 8. Vecteurs et oracle

- `testdata/air/gen.py` : oracle **indépendant**, en Python pur (AES FIPS-197, SHA-256 et HMAC de `hashlib`/`hmac`, X25519 RFC 7748, Ed25519 RFC 8032 avec clés étendues). Il produit `testdata/air/vectors.toml`.
- Captures de **nos** appareils (identités de test importées dans le Heltec par `IMPORT_PRIVATE_KEY`) : annonces, DM, canal Public, datagrammes.
- Cible de fuzz `air_decode` : jamais de panic ; toute trame acceptée se réécrit à l'identique.
