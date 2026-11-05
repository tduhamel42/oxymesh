# Protocole compagnon MeshCore, côté appareil (v1.18)

Source de vérité pour `mc-proto` et la couche compagnon de `mc-node`. Référence : MeshCore `companion-v1.18.0`, commit 81dbae5 (ADR-0001). Entiers en petit-boutiste. Les positions `[i]` comptent le code de commande en `[0]`.

Abréviations : `M` = `examples/companion_radio/MyMesh.cpp`, `M.h` = `MyMesh.h`, `DS` = `examples/companion_radio/DataStore.cpp`, `NP` = `examples/companion_radio/NodePrefs.h`, `BCM` = `src/helpers/BaseChatMesh.cpp`, `BLE` = `src/helpers/esp32/SerialBLEInterface.cpp`, `ASI` = `src/helpers/ArduinoSerialInterface.cpp`.

## 0. Constantes

| Élément | Valeur | Source |
|---|---|---|
| `FIRMWARE_VER_CODE` | 14 | M.h:7 |
| Version, date de build | `"v1.18.0"`, `"15 Oct 2026"` (les builds de release mettent `v1.18.0-81dbae5`, `15-Oct-2026`) | M.h:9-15, `build.sh:133-150` |
| Trame | 176 octets au plus, dans les deux sens | `src/helpers/BaseSerialInterface.h:5` |
| Build M5 | 350 contacts, 40 canaux, file de 256, PIN 123456 (→ aléatoire, §1.1), 22 dBm max | `variants/thinknode_m5/platformio.ini:174-181` |
| Fabricant (M5) | `"Elecrow ThinkNode M5"` | `variants/thinknode_m5/ThinknodeM5Board.cpp` |
| Contacts temporaires (« anon ») | 8, aux indices 0..7 | `BaseChatMesh.h:41, 97-100` |
| Accusés attendus | 8, en anneau | M.h:285 |
| Chemins d'annonce gardés | 16 | M.h:289 |
| Connexions (keep-alive) | 16 | `BaseChatMesh.h:44` |
| Texte | 160 octets | `BaseChatMesh.h:8` |

**Règle d'aiguillage** (M:1055-2062) : une commande dont la précondition de longueur échoue tombe dans le dernier `else` et reçoit `ERR 1` (UNSUPPORTED_CMD), pas `ERR 6`.

### Codes d'erreur (M:135-140)

| Code | Nom |
|---|---|
| 1 | UNSUPPORTED_CMD (aussi : précondition de longueur) |
| 2 | NOT_FOUND |
| 3 | TABLE_FULL |
| 4 | BAD_STATE |
| 5 | FILE_IO_ERROR |
| 6 | ILLEGAL_ARG |
| réponse 15 | DISABLED |

`ERR` = `[0x01, code]`. `OK` = `[0x00]` seul.

## 1. Porteurs

### 1.1 Bluetooth (BLE)

- Service Nordic UART `6E400001-B5A3-F393-E0A9-E50E24DCCA9E` ; RX `…0002…` (écriture, chiffrement + MITM exigés) ; TX `…0003…` (lecture et notification, chiffrement + MITM) (BLE:7-9, 45-51). L'UUID du service est annoncé (BLE:53).
- Nom : `"MeshCore-" + nom du nœud`, fixé au démarrage (un changement de nom prend effet au redémarrage). Un nom `"@@MAC"` est remplacé par les 12 chiffres hexadécimaux de l'adresse MAC (BLE:16-24).
- Sécurité : LE Secure Connections + MITM + appairage mémorisé, passkey affiché par l'appareil (l'utilisateur le tape sur le téléphone) (BLE:31-33, 58-75). Appairages en NVS, effacés par `FACTORY_RESET`.
- PIN (`examples/companion_radio/main.cpp:188-202`) : `prefs.ble_pin` s'il est non nul ; sinon, avec un écran et le PIN de build 123456, un **PIN aléatoire de 6 chiffres** affiché à l'écran (cas du M5) ; sinon le PIN de build.
- « Connecté » veut dire **authentifié** : les trames sortantes sont jetées tant que l'appairage n'a pas réussi ; en cas d'échec, déconnexion et nouvelle annonce après 1 s (BLE:77-88, 174).
- Une écriture = une trame ; une notification = une trame. Pas de tramage en plus.
- Entrée : file de 4 trames ; une trame > 176 octets ou une file pleine → trame jetée (BLE:114-129).
- Sortie : file de 4 trames (pleine → jetée) ; une notification au plus toutes les **60 ms** (BLE:168-209).
- Déconnexion : files vidées, nouvelle annonce après 1000 ms. L'état de la machine n'est **pas** remis à zéro (`app_target_ver`, file d'attente, itération de contacts) (BLE:104-110, 218-237). Pas d'annonce pendant une connexion.

### 1.2 USB / série (ASI:24-73)

- App → appareil : `'<'` (0x3C), longueur u16, charge. Appareil → app : `'>'` (0x3E), longueur u16, charge.
- Octets avant `'<'` ignorés ; longueur 0 → remise à zéro du lecteur ; octets au-delà de 176 lus et jetés (trame tronquée à 176) ; pas de délai entre octets.
- Toujours « connecté » : les notifications partent toujours. 115200 bauds (sur le M5, UART0 via le pont CH340).

### 1.3 Plusieurs porteurs (`src/helpers/MultiSerialInterface.h:117-198`)

Chaque trame sortante part sur **tous** les porteurs actifs ; on lit la première trame disponible ; « connecté » si l'un l'est.

## 2. Commandes

`SENT` (réponse 6) fait toujours 10 octets : `[1]` 1 = inondation / 0 = direct, `[2..5]` accusé attendu ou tag, `[6..9]` délai estimé en ms (u32), calculé ainsi (M:888-896) :
- inondation : `500 + 16 × airtime_ms` ;
- direct : `500 + (airtime × 6 + 250) × (sauts + 1)`, sauts = `path_len & 63`.

| Code | Nom | Requête | Réponse et effets | Source |
|---|---|---|---|---|
| 1 | APP_START | len ≥ 8 ; `[1..7]` réservés, `[8..]` nom de l'app | SELF_INFO (§3.1) ; interrompt une itération de contacts | M:1076-1117 |
| 2 | SEND_TXT_MSG | len ≥ 14 ; `[1]` txt_type, `[2]` essai, `[3..6]` ts, `[7..12]` préfixe de clé (6), `[13..]` texte | type 0 : SENT avec accusé attendu (entré dans l'anneau de 8). Types 1 et 3 : ts remplacé par l'heure unique de l'appareil, accusé 0. Contact inconnu → ERR 2 ; autre type → ERR 1 ; échec ou texte trop long → ERR 3 | M:1132-1177 |
| 3 | SEND_CHANNEL_TXT_MSG | `[1]` txt_type (0), `[2]` canal, `[3..6]` ts, `[7..]` texte ; pas de contrôle de longueur | **OK** (pas SENT). Émis : `"<nom>: <texte>"` tronqué à 160. Canal invalide → ERR 2 ; type ≠ 0 → ERR 1 | M:1178-1197, BCM:518-537 |
| 4 | GET_CONTACTS | `[1..4]` since (optionnel) | CONTACTS_START (2) `[1..4]` = nombre total ; un CONTACT (3) par contact de `lastmod > since` ; END_OF_CONTACTS (4) `[1..4]` = plus grand lastmod envoyé (0 si aucun). Un contact par tour de boucle, au rythme du porteur. Nouvelle demande pendant l'itération → ERR 4 | M:1238-1258, 2388-2409 |
| 5 | GET_DEVICE_TIME | — | CURR_TIME (9) `[1..4]` = heure Unix | M:1281-1286 |
| 6 | SET_DEVICE_TIME | len ≥ 5 ; `[1..4]` heure | OK si ≥ heure actuelle, sinon **ERR 6** (l'horloge ne recule jamais) | M:1287-1296 |
| 7 | SEND_SELF_ADVERT | `[1]` optionnel : 1 = inondation (portée par défaut), sinon zéro saut | OK ou ERR 3 ; position incluse si `advert_loc_policy ≠ 0` | M:1297-1316 |
| 8 | SET_ADVERT_NAME | len ≥ 2 ; nom (31 octets max) | OK ; réglages sauvés | M:1259-1265 |
| 9 | ADD_UPDATE_CONTACT | len ≥ 36 ; contact (§3.2) depuis `[1]` ; lat/lon si len ≥ 144 ; lastmod si len ≥ 148 | mise à jour champ par champ, ou ajout (sync_since = 0) ; lastmod = valeur fournie ou maintenant. OK ou ERR 3 (plein). Les champs sont lus jusqu'à `[135]` même si la trame est plus courte (on lit des zéros). Écriture des contacts différée de 5 s | M:1328-1348, 194-217 |
| 10 | SYNC_NEXT_MESSAGE | — | tête de la file d'attente telle quelle (7/8/16/17/27) ou NO_MORE_MESSAGES (10) | M:1415-1423 |
| 11 | SET_RADIO_PARAMS | `[1..4]` fréquence kHz, `[5..8]` BW Hz, `[9]` SF, `[10]` CR, `[11]` repeat (optionnel, v9+) | bornes : 150000..2500000, 7000..500000, 5..12, 5..8. repeat ≠ 0 exige une fréquence permise, sinon ERR 6. **Sans l'octet repeat, repeat est désactivé.** Appliqué et sauvé ; OK | M:1424-1459 |
| 12 | SET_RADIO_TX_POWER | `[1]` i8 dBm | −9..22 sur le M5, sinon ERR 6 ; OK | M:1460-1469 |
| 13 | RESET_PATH | len ≥ 33 ; clé | out_path_len = 0xFF ; OK / ERR 2 | M:1317-1327 |
| 14 | SET_ADVERT_LATLON | len ≥ 9 ; lat, lon i32 ×1e6, alt optionnelle (ignorée) | bornes, sinon ERR 6 ; OK | M:1266-1280 |
| 15 | REMOVE_CONTACT | clé | retire le contact et son annonce gardée ; OK / ERR 2 | M:1349-1358 |
| 16 | SHARE_CONTACT | clé | réémet l'annonce gardée en zéro saut, codes de transport {0,0} ; OK / ERR 3 / ERR 2 | M:1359-1370, BCM:570-582 |
| 17 | EXPORT_CONTACT | clé ; len < 33 = soi-même | EXPORT_CONTACT (11) + trame brute (§3.3) ; ERR 2 | M:1379-1408 |
| 18 | IMPORT_CONTACT | len > 98 ; trame d'annonce brute | rejouée comme reçue par inondation (auto-ajout et anti-rejeu s'appliquent). OK envoyé avant traitement ; ERR 6 si ce n'est pas une annonce | M:1409-1414, BCM:588-601 |
| 19 | REBOOT | `[1..6]` = `"reboot"` | pas de réponse ; écrit les contacts en attente, redémarre | M:1512-1516 |
| 20 | GET_BATT_AND_STORAGE | — | BATT_AND_STORAGE (12) : mV u16, utilisé Ko u32, total Ko u32 | M:1517-1527 |
| 21 | SET_TUNING_PARAMS | rx_delay ×1000 u32, airtime_factor ×1000 u32 ; pas de contrôle de longueur | sauvés ; OK | M:1470-1480 |
| 22 | DEVICE_QUERY | len ≥ 2 ; `[1]` app_target_ver | garde app_target_ver ; DEVICE_INFO (§3.1) | M:1056-1075 |
| 23 | EXPORT_PRIVATE_KEY | — | PRIVATE_KEY (14) + 64 octets | M:1528-1536 |
| 24 | IMPORT_PRIVATE_KEY | len ≥ 65 ; clé de 64 octets | contrôlée (pub[0] ∉ {00, FF}, auto-test ECDH) ; sauvée ; OK ; contacts rechargés. ERR 6 invalide, ERR 5 écriture | M:1537-1556, `src/Identity.cpp:67-109` |
| 25 | SEND_RAW_DATA | len ≥ 6 ; `[1]` path_len (0xFF → ERR 1), chemin, charge (≥ 4) | RAW_CUSTOM direct ; OK / ERR 3 / ERR 6 | M:1557-1576 |
| 26 | SEND_LOGIN | len ≥ 33 ; clé, mot de passe (15 car. utiles) | SENT avec tag = **4 premiers octets de la clé** ; annule les requêtes en cours. Résultat : 0x85 ou 0x86 | M:1577-1598, BCM:603-636 |
| 27 | SEND_STATUS_REQ | len ≥ 33 ; clé | SENT (tag) ; résultat 0x87 (associé par préfixe de clé) | M:1630-1650, BCM:690-715 |
| 28 | HAS_CONNECTION | len ≥ 33 ; clé | OK si connexion keep-alive active, sinon ERR 2 | M:1741-1747 |
| 29 | LOGOUT | len ≥ 33 ; clé | retire la connexion ; toujours OK | M:1748-1751 |
| 30 | GET_CONTACT_BY_KEY | clé | CONTACT (3) / ERR 2 (cherche aussi dans les contacts temporaires) | M:1371-1378 |
| 31 | GET_CHANNEL | len ≥ 2 ; `[1]` indice | CHANNEL_INFO (18) : `[1]` indice, `[2..33]` nom (32), `[34..49]` secret (16) ; 50 octets ; ERR 2 si indice ≥ 40 | M:1752-1766 |
| 32 | SET_CHANNEL | len 50..65 ; indice, nom (32), secret (16) | hash = SHA256(secret)[0] ; sauvé tout de suite ; OK / ERR 2. **len ≥ 66 (clé de 32) → ERR 1** | M:1767-1780 |
| 33 | SIGN_START | — | SIGN_START (19) : `[1]` 0, `[2..5]` taille max 8192 | M:1781-1792 |
| 34 | SIGN_DATA | len > 1 ; morceau | OK ; ERR 4 sans SIGN_START ; ERR 3 au-delà de 8 Ko | M:1793-1800 |
| 35 | SIGN_FINISH | — | SIGNATURE (20) + 64 octets / ERR 4 | M:1801-1812 |
| 36 | SEND_TRACE_PATH | len > 10 et len − 10 < 179 ; tag u32, auth u32, flags, hashes | SENT (drapeau 0, tag de l'app, délai direct) ; ERR 6 si longueur incohérente ou > 64 sauts ; ERR 3 | M:1813-1838 |
| 37 | SET_DEVICE_PIN | len ≥ 5 ; u32 | 0 ou 100000..999999 ; sauvé (actif au redémarrage) ; OK / ERR 6 | M:1839-1852 |
| 38 | SET_OTHER_PARAMS | `[1]` manual_add ; optionnels : `[2]` télémétrie (bits 0-1 base, 2-3 position, 4-5 environnement), `[3]` advert_loc_policy, `[4]` multi_acks | sauvé ; OK | M:1488-1503 |
| 39 | SEND_TELEMETRY_REQ | len ≥ 36 : `[4..35]` clé → distant ; len == 4 → soi-même ; autre → ERR 1 | distant : SENT puis 0x8B. Soi-même : **notification 0x8B immédiate** (tension, température MCU, capteurs en CayenneLPP) | M:1680-1719 |
| 40 | GET_CUSTOM_VARS | — | CUSTOM_VARS (21) + `"k:v,k:v"` ; M5 avec GPS : `"gps:0"` ou `"gps:1"` | M:1853-1866 |
| 41 | SET_CUSTOM_VAR | len ≥ 4 ; `"nom:valeur"` | OK / ERR 6 ; `gps` et `gps_interval` sauvés | M:1867-1892 |
| 42 | GET_ADVERT_PATH | len ≥ 34 ; `[1]` réservé, clé (comparée sur 7 octets) | ADVERT_PATH (22) : ts reçu u32, path_len, chemin / ERR 2 | M:1893-1913 |
| 43 | GET_TUNING_PARAMS | — | TUNING_PARAMS (23) : rx_delay ×1000, airtime_factor ×1000 | M:1481-1487 |
| 44-49 | (WiFi, réservés) | | ERR 1 | M:52 |
| 50 | SEND_BINARY_REQ | len ≥ 34 ; clé, données | SENT (tag) ; résultat 0x8C | M:1720-1740 |
| 51 | FACTORY_RESET | `[1..5]` = `"reset"` | coupe tous les porteurs, efface le stockage et les appairages, OK (jamais reçu), redémarre après 1 s | M:1965-1977 |
| 52 | SEND_PATH_DISCOVERY_REQ | `[1]` = 0, clé | requête de télémétrie forcée en inondation ; SENT ; résultat 0x8D ; le chemin du contact n'est **pas** mis à jour | M:1651-1679, 779-808 |
| 54 | SET_FLOOD_SCOPE_KEY (v8+) | `[1]` = 0 : `[2..17]` clé (len ≥ 18) fixe la portée, plus court l'efface ; `[1]` = 1 (v12+) : sans portée | OK ; **RAM seulement** | M:1978-1988 |
| 55 | SEND_CONTROL_DATA (v8+) | charge avec `[1] & 0x80` | CONTROL zéro saut ; OK / ERR 3 | M:2015-2022 |
| 56 | GET_STATS (v8+) | `[1]` type 0/1/2 | STATS (24) + type : 0 = mV u16, uptime s u32, err_flags u16, file u8 ; 1 = bruit i16, RSSI i8, SNR×4 i8, airtime TX s u32, RX s u32 ; 2 = sept compteurs u32 (reçus, émis, émis inondation, émis direct, reçus inondation, reçus direct, erreurs). Autre → ERR 6 | M:1914-1964 |
| 57 | SEND_ANON_REQ | len > 33 ; clé, données | v13+ : clé inconnue → contact temporaire (type 0, chemin zéro saut). SENT ; résultat 0x8C ; plein → ERR 3 | M:1599-1629 |
| 58 | SET_AUTOADD_CONFIG | `[1]` masque, `[2]` max_hops optionnel (≤ 64) | sauvé ; OK | M:2023-2029 |
| 59 | GET_AUTOADD_CONFIG | — | AUTOADD_CONFIG (25) : masque, max_hops | M:2030-2035 |
| 60 | GET_ALLOWED_REPEAT_FREQ | — | (26) + paires (bas u32, haut u32) en kHz : {433000,433000}, {869495,869495}, {918000,918000} | M:1032-1040, 2036-2044 |
| 61 | SET_PATH_HASH_MODE | `[1]` = 0, `[2]` mode 0..2 | sauvé ; OK / ERR 6 | M:1504-1511 |
| 62 | SEND_CHANNEL_DATA | len ≥ 4 ; canal, path_len (0xFF = inondation), chemin, data_type u16, charge | OK. ERR 2 canal ; ERR 6 chemin invalide, data_type 0 ou charge > 167 ; ERR 3 échec (y compris 166-167 octets, qui échouent toujours) | M:1198-1237 |
| 63 | SET_DEFAULT_FLOOD_SCOPE | len ≥ 48 : nom `[1..31]` (NUL, 1..30 car.), clé `[32..47]` ; plus court efface | sauvé ; OK / ERR 6 | M:1989-2005 |
| 64 | GET_DEFAULT_FLOOD_SCOPE | — | (28) + nom[31] + clé[16] (48 octets), ou 1 octet si aucun | M:2006-2014 |
| 65 | SEND_RAW_PACKET | len ≥ 4 ; `[1]` priorité, trame brute | émise telle quelle ; OK / ERR 6 / ERR 3 | M:2045-2058 |
| 66 | RUN_CLI_COMMAND (v14+) | len ≥ 3 ; commande ASCII | CLI_REPLY (29) + texte ; un préfixe `"xx|"` est renvoyé. Commandes : `src/helpers/CommonRadioPrefs.cpp:31-231`, M:2079-2211 | M:1118-1131 |

## 3. Formats des réponses

### 3.1 SELF_INFO (5) et DEVICE_INFO (13)

SELF_INFO, 89 octets au plus (M:1085-1117) :

| Position | Champ |
|---|---|
| [1] | 1 (type chat) |
| [2] | puissance TX (dBm) |
| [3] | puissance max (22) |
| [4..35] | clé publique |
| [36..43] | lat, lon (i32 ×1e6) |
| [44] | multi_acks |
| [45] | advert_loc_policy |
| [46] | télémétrie : env << 4 \| pos << 2 \| base |
| [47] | manual_add_contacts |
| [48..51] | fréquence kHz (**écart volontaire 2** : valeur exacte, le C++ donne 869617 pour 869,618) |
| [52..55] | BW en Hz |
| [56] | SF |
| [57] | CR |
| [58..] | nom, sans NUL |

DEVICE_INFO, 82 octets (M:1059-1075) :

| Position | Champ |
|---|---|
| [1] | 14 |
| [2] | contacts max / 2 (175) |
| [3] | canaux max (40) |
| [4..7] | `prefs.ble_pin` (0 quand le PIN est aléatoire) |
| [8..19] | date de build, complétée par des NUL (12) |
| [20..59] | fabricant, complété par des zéros (40) |
| [60..79] | version, complétée par des zéros (20) |
| [80] | repeat activé |
| [81] | path_hash_mode |

Pour oxymesh : fabricant `"Elecrow ThinkNode M5"`, version `"v1.18.0-oxymesh"` (20 octets au plus), date de build réelle au format `"dd-Mmm-yyyy"`.

### 3.2 Contact (CONTACT 3 et notification NEW_ADVERT 0x8A), 148 octets (M:171-192)

| Position | Champ |
|---|---|
| [1..32] | clé publique |
| [33] | type (0 aucun, 1 chat, 2 relais, 3 salon, 4 capteur) |
| [34] | flags (bit 0 favori, 1 télémétrie base, 2 position, 3 environnement, 4 CLI à distance) |
| [35] | out_path_len |
| [36..99] | out_path (64 octets) |
| [100..131] | nom (32, complété par des NUL) |
| [132..135] | dernier ts d'annonce (horloge de l'émetteur) |
| [136..143] | lat, lon |
| [144..147] | lastmod (notre horloge) |

### 3.3 Trame d'annonce brute (EXPORT, IMPORT, annonces gardées)

`en-tête ‖ path_len ‖ chemin ‖ charge` (`src/Packet.cpp:52-63`). Export de soi : en-tête 0x11 (FLOOD, ADVERT), path_len 0 (M:1389). Les annonces gardées conservent le chemin reçu, route forcée à FLOOD, codes de transport retirés (BCM:141-149).

## 4. Notifications (≥ 0x80)

Sauf mention, une notification n'est envoyée que si un porteur est connecté, sinon elle est **perdue**. Seuls les messages survivent, par la file d'attente.

| Code | Nom | Format | Quand | Source |
|---|---|---|---|---|
| 0x80 | ADVERT | clé (32) | annonce d'un contact connu, ou d'un contact qu'on vient d'ajouter automatiquement | BCM:156, 200 ; M:358-367 |
| 0x81 | PATH_UPDATED | clé (32) | chemin appris par un PATH | M:409-415 |
| 0x82 | SEND_CONFIRMED | accusé (4), aller-retour ms u32 ; 9 octets | accusé reconnu dans l'anneau (l'entrée est effacée) | M:424-435 |
| 0x83 | MSG_WAITING | 1 octet | après chaque message mis en file | M:469-473 |
| 0x84 | RAW_DATA | SNR×4, RSSI, 0xFF, charge | RAW_CUSTOM direct reçu | M:832-853 |
| 0x85 | LOGIN_SUCCESS | nouveau, 14 octets : permissions, préfixe (6), tag (4), permissions ACL, niveau firmware ; ancien « OK », 8 octets : 0, préfixe (6) | réponse à une connexion en cours | M:694-723 |
| 0x86 | LOGIN_FAIL | 0, préfixe (6) | autre réponse à une connexion | M:717-722 |
| 0x87 | STATUS_RESPONSE | 0, préfixe (6), données | réponse d'état | M:724-738 |
| 0x88 | LOG_RX_DATA | SNR×4 (i8), RSSI (i8), trame brute | chaque trame reçue, si connecté et `len + 3 ≤ 176` | M:291-302 |
| 0x89 | TRACE_DATA | 0, path_len, flags, tag, auth, hashes, SNR par saut, SNR final | trace arrivée au bout | M:855-886 |
| 0x8A | NEW_ADVERT | contact complet (§3.2) | annonce d'un contact **non** ajouté (mode manuel, type exclu, trop de sauts, table pleine) | BCM:158-182 |
| 0x8B | TELEMETRY_RESPONSE | 0, préfixe (6), CayenneLPP | réponse de télémétrie (et télémétrie de soi) | M:739-749 |
| 0x8C | BINARY_RESPONSE | 0, tag (4), données | réponse à BINARY_REQ ou ANON_REQ, et toute réponse non traitée | M:750-776 |
| 0x8D | PATH_DISCOVERY_RESPONSE | 0, préfixe (6), out_path_len, out_path, in_path_len, in_path | réponse de découverte de chemin | M:779-804 |
| 0x8E | CONTROL_DATA | SNR×4, RSSI, path_len, charge | CONTROL zéro saut avec 0x80 | M:810-830 |
| 0x8F | CONTACT_DELETED | clé (32) | plus ancien contact écrasé (table pleine) | M:342-349 |
| 0x90 | CONTACTS_FULL | 1 octet | pas de place pour une annonce | M:351-356 |

Une seule requête (connexion, état, télémétrie, découverte, binaire) en cours à la fois : chaque nouvelle annule les autres (M.h:192-194).

## 5. Messages reçus et file d'attente

**File d'attente** (M:224-260) : FIFO de 256 trames (messages directs 7/16, canaux 8/17, données de canal 27). Pleine : on retire la plus ancienne trame de canal ; s'il n'y en a pas, la nouvelle est jetée. MSG_WAITING part quand même. **Le format est figé au moment de la mise en file**, selon `app_target_ver` de ce moment. (**Écart volontaire 1** : notre file survit au redémarrage.)

**CONTACT_MSG_RECV_V3 (16)** (M:440-467) : SNR×4, 0, 0, préfixe émetteur (6), path_len (celui de la trame si reçue par inondation, **0xFF si directe**), txt_type, ts (4), préfixe de l'auteur (4, seulement pour txt_type 2), texte. **Ancien (7)** : le même sans `[1..3]`.

**CHANNEL_MSG_RECV_V3 (17)** (M:558-583) : SNR×4, 0, 0, canal, path_len, txt_type (0), ts (4), `"Nom: texte"`. **Ancien (8)** : le même sans `[1..3]`.

**CHANNEL_DATA_RECV (27)** (M:601-627) : SNR×4, 0, 0, canal, path_len, data_type u16, longueur, données.

`app_target_ver` (DEVICE_QUERY, RAM, 0 au démarrage) : **≥ 3 → trames V3 (16/17), sinon 7/8** (M:443, 561). Txt_type 1 reçu : mis en file. Txt_type 3 reçu : exécuté si le contact a le flag 0x10, sinon mis en file (M:538-548).

## 6. Accusés, essais, auto-ajout

- Accusé attendu = `SHA256(ts ‖ essai & 3 ‖ texte ‖ notre clé)[0..4]` (BCM:451-471). Si essai > 3 : `0x00 ‖ essai` après le texte, texte limité à 158.
- **Le firmware ne renvoie jamais** et ne signale pas l'échec (`onSendTimeout` vide, M:898) : l'app renvoie avec le même ts et essai + 1, et peut passer en inondation par RESET_PATH. (**Écart volontaire 6** : sauf pour les messages envoyés depuis notre écran.)
- Auto-ajout (M:304-340, BCM:72-106, 122-201) :
  - `manual_add_contacts` bit 0 à 0 : tout type ajouté ;
  - bit 0 à 1 : seulement les types cochés dans `autoadd_config` (0x02 chat, 0x04 relais, 0x08 salon, 0x10 capteur) ;
  - `autoadd_max_hops` N > 0 : refus si sauts ≥ N ;
  - table pleine : avec `autoadd_config & 0x01`, on écrase le plus ancien (lastmod) non favori (notification 0x8F, annonce gardée effacée) ; sinon 0x8A puis 0x90 ;
  - annonce avec ts ≤ celui connu : ignorée ;
  - chaque annonce acceptée réécrit l'annonce gardée et programme l'écriture des contacts (5 s).
- Canaux : le canal 0 est préchargé « Public » avec la PSK `izOH6cXN6mrJ5e26oRXNcg==`, puis remplacé par la sauvegarde (M:1002-1003).
- Portée d'inondation : `send_unscoped` > portée en RAM (cmd 54) > portée par défaut (sauvée, cmd 63) > aucune (M:491-525).

## 7. Réglages par défaut (build M5 BLE)

Nom = 4 premiers octets de la clé en 8 hexadécimaux majuscules ; 869,618 MHz / 62,5 kHz / SF8 / CR 5 ; 22 dBm ; airtime_factor 1,0 ; tx_delay 0,5 ; direct 0,2 ; rx_delay 0 ; multi_acks 0 ; manual_add 0 ; télémétrie 0/0/0 ; advert_loc_policy 0 ; ble_pin 0 ; repeat désactivé ; path_hash_mode 0 ; autoadd 0/0 ; gain RX renforcé ; GPS désactivé ; pas de portée par défaut ; pas d'annonce au démarrage (M:916-1007).

## 8. Persistance

### 8.1 Firmware C++ (à lire pour la migration, ADR-0006)

SPIFFS (`SPIFFS.begin(true)`), table arduino-esp32 par défaut : `spiffs` en 0x290000, 0x160000 (à confirmer sur l'appareil).

| Fichier | Format | Source |
|---|---|---|
| `/identity/_main.id` | 96 octets : pub(32) ‖ prv(64) orlp étendue | `src/Identity.cpp:111-121`, `src/helpers/IdentityStore.cpp:45-65` |
| `/prefs.json` | pseudo-JSON de ConfigSerializer (clés sans guillemets, flottants à 4 décimales, lat/lon à 6, binaire en hexa majuscule) : `{name, lat, lon, radio:{freq,bw,sf,cr,cad,int_thr,rxgain,fem_rxgain,fem_txgain,tx,af,rxdelay,f_txdelay,d_txdelay,agc_int,hash_mode,multi_ack}, gps:{en,int,adv_loc}, repeat:{disable}, comp:{auto_max,defs_nm,defs_key,pin,buzz_q,vibe_q,auto_add,man_add,tel_base,tel_loc,tel_env,tz_offset}, custom:{…}}` | NP:62-207, `src/helpers/ConfigSerializer.cpp` |
| `/new_prefs` | ancien binaire, lu si `prefs.json` manque | DS:212-240 |
| `/contacts3` | enregistrements de 152 octets : pub 32, nom 32, type 1, flags 1, inutilisé 1, sync_since 4, out_path_len 1, last_advert 4, out_path 64, lastmod 4, lat 4, lon 4 | DS:259-321 |
| `/channels2` | 40 enregistrements de 68 octets : 4 zéros, nom 32, secret 32 ; indice = numéro de canal | DS:323-366 |
| `/bl/<16 hexa majuscules des 8 premiers octets de la clé>` | trame d'annonce brute ≤ 255 octets | DS:559-603 |

### 8.2 oxymesh

Partition `mcdata` (sequential-storage, ADR-0006) : identité, réglages (binaire versionné), contacts, canaux, annonces gardées, file d'attente, historique local de l'écran. Le format est défini en MC4 dans ce document.

## 9. Écarts volontaires (ADR-0001)

1. File d'attente gardée en flash.
2. Fréquence exacte dans SELF_INFO.
3. Nom de canal complété par des zéros dans CHANNEL_INFO.
4. Pas de troncature des trames de 174-176 octets en Bluetooth (MTU ≥ 179).
5. PIN aléatoire tiré à chaque appairage.
6. Renvois automatiques pour les messages envoyés depuis l'écran.

## 10. Décodage côté appareil (ADR-0002)

Le mode `Device` de `mc-proto` reproduit les tolérances ci-dessus : champs finaux optionnels (repeat de SET_RADIO_PARAMS, octets 2-4 de SET_OTHER_PARAMS, lat/lon/lastmod d'ADD_UPDATE_CONTACT, max_hops de SET_AUTOADD_CONFIG, since de GET_CONTACTS, mode de SEND_SELF_ADVERT, alt de SET_ADVERT_LATLON), octets en trop ignorés, préconditions de longueur → ERR 1. Les vecteurs listent chaque cas avec sa ligne C++.
