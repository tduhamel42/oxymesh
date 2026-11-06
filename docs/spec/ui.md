# Interface du ThinkNode M5

Source de vérité pour `mc-ui`. Textes affichés **en anglais**. Écran e-paper 200×200 noir et blanc, deux boutons, potentiomètre de luminosité, LED bleue, buzzer.

## 1. Principes

- **Les deux boutons gardent toujours le même sens**, rappelé par une barre d'aide en bas de l'écran.
- **Tout l'écran sert** (la mise en page du C++ est un écran 128×64 agrandi, qui laisse 45 % de vide).
- **Accents lisibles** (polices u8g2 avec Latin-1 et Latin étendu) ; un caractère sans glyphe s'affiche comme un losange, pas comme un pavé.
- **L'écran ne bloque jamais la radio ni le Bluetooth** : l'attente de fin de rafraîchissement est asynchrone.
- **Rien ne s'efface tout seul** de l'app : la file des messages en attente reste celle du protocole (`docs/spec/companion.md` §5).

## 2. Boutons

| Bouton | Appui court | Appui long (≥ 600 ms) | Double appui (< 300 ms) |
|---|---|---|---|
| A (GPIO21, bas) | suivant / descendre | ouvrir / valider | précédent / monter |
| B (GPIO14, haut) | retour (accueil depuis le premier niveau) | allume ou éteint l'éclairage | — |

- Le premier appui quand l'écran est en veille d'éclairage ne fait que rallumer l'éclairage s'il était allumé avant.
- Pendant 8 s après le démarrage, un appui long sur A ouvre l'écran de secours (réinitialisation des réglages, reprise de la migration).

## 3. Éclairage et potentiomètre

- L'éclairage reste allumé une fois activé, pour qu'on puisse régler sa luminosité au potentiomètre.
- Le potentiomètre n'est pas lisible par le firmware (`docs/spec/m5.md`) : pas de zone morte possible en bas de course. L'extinction à 100 % est garantie par le bouton B et l'extinction automatique, qui coupent l'alimentation des LED (IO5), quelle que soit la position de la molette. L'en-tête montre toujours l'état de l'éclairage (Tanguy, 2026-11-06).
- Extinction automatique réglable : 30 s, 2 min (défaut), jamais ; le délai repart à chaque appui.
- Icône d'ampoule dans l'en-tête quand il est allumé.

## 4. En-tête (tous les écrans)

`HH:MM` · non-lus (enveloppe + nombre) · Bluetooth (connecté, en attente, coupé) · éclairage · **batterie en pourcentage** (symbole de charge quand l'USB est branché ; `docs/spec/m5.md` §3).

## 5. Écrans

| Écran | Contenu |
|---|---|
| **Home** | Grande horloge, date. Non-lus : DM et canaux. Aperçu du dernier message (expéditeur, début du texte, âge). Batterie en pourcentage et en tension. Bluetooth : nom de l'app connectée, ou « Pair: 123456 » pendant un appairage. Réseau : préréglage (ex. « EU Narrow · fr »), dernier relais entendu, plancher de bruit. GPS : fix et satellites si activé |
| **Inbox** | Conversations (contacts et canaux), triées par dernier message, avec point de non-lu et aperçu d'une ligne |
| **Conversation** | Bulles (reçues à gauche, envoyées à droite) avec retour à la ligne. Envoi : `sending…`, `delivered ✓ 2.4 s` (aller-retour), `failed`. Réception : sauts (`direct` ou `3 hops`) et SNR. A long = Quick reply |
| **Quick reply** | 8 réponses prêtes (défaut : OK, Yes, No, On my way, Call me, Where are you?, Running late, Thanks) et « Send my position ». Modifiables par la commande CLI `set reply.N <texte>`. Envoi depuis l'appareil : 3 essais directs, puis `RESET_PATH` et inondation (écart volontaire 6) |
| **Network** | Nœuds entendus (nom, type, sauts, SNR, âge), relais en tête. A long = découverte en zéro saut (au plus une fois par 5 s). Temps d'antenne de l'heure et duty cycle |
| **My card** | QR `meshcore://contact/add?name=<nom>&public_key=<64 hexa>&type=1`, scannable par l'app officielle ; nom et clé abrégée dessous |
| **Pairing** | Plein écran, PIN en très grands chiffres, dès que l'appairage commence ; disparaît à la fin |
| **Settings** | Région (préréglages de `config/regions.toml`), Bluetooth, GPS, buzzer, durée d'éclairage, envoyer une annonce (zéro saut ou inondation), hibernation |

Navigation : Home → (A court) Inbox → Network → My card → Settings → Home ; A long ouvre l'élément choisi ; B revient.

## 6. Notifications

- **LED bleue** : clignote brièvement toutes les 5 s tant qu'il reste un non-lu.
- **Buzzer** : petite mélodie pour un DM, une autre pour un canal, un bip pour un accusé ; seulement quand aucune app n'est connectée (comme le C++) ; coupé par réglage.
- Un nouveau DM affiche l'aperçu sur Home ; on ne change pas d'écran à la place de l'utilisateur pendant qu'il navigue.

## 7. Historique local et non-lus

- Les 64 derniers messages (DM et canaux, reçus et envoyés depuis l'écran) sont gardés en flash, indépendamment de la file d'attente du protocole.
- Un message devient lu quand on l'ouvre sur l'appareil **ou** quand l'app le récupère par `SYNC_NEXT_MESSAGE`.

## 8. Rafraîchissement e-paper

- Rafraîchissement partiel pour la navigation et les mises à jour ; complet tous les 30 partiels, toutes les heures, et à chaque retour sur Home après plus de 10 min.
- Horloge à la minute ; pas de compteur de secondes ; les âges s'affichent en minutes.
- Pas de redessin si l'image n'a pas changé (hash du tampon).
- Batterie : moyenne glissante, mise à jour au plus une fois par minute.

## 9. Vérification

- `mc ui --out <dir>` rend chaque écran, dans des états scriptés, en PNG ; ces images de référence sont comparées en test.
- Sur l'appareil (MC7) : lecture d'un DM, réponse rapide avec coche de livraison, QR ajouté dans l'app officielle, éclairage maintenu et réglé au potentiomètre, pourcentage de batterie juste à ±5 %, aucune trame perdue pendant un rafraîchissement.
