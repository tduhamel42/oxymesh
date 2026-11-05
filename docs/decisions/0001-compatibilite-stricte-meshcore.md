# ADR-0001 — Compatibilité stricte avec MeshCore v1.18

- Statut : accepté
- Date : 2026-11-05

## Contexte

Tanguy veut un firmware Rust pour son ThinkNode M5 qui remplace le firmware MeshCore sans rien casser : il doit parler aux nœuds MeshCore d'origine sur l'air (le réseau de relais français) et rester utilisable avec l'app MeshCore officielle, meshcore.js et l'app-web de loranet. Le seul gain visible attendu est l'interface de l'appareil.

## Décision

- **Référence** : MeshCore `companion-v1.18.0`, commit 81dbae5 (2026-10-15). Les specs `docs/spec/air.md` et `docs/spec/companion.md` citent les lignes de ce commit ; le code C++ prime sur la doc de MeshCore quand ils divergent.
- **Sur l'air** : mêmes octets émis que le firmware C++ dans les mêmes conditions, mêmes trames acceptées (y compris les variantes anciennes encore tolérées par v1.18).
- **Protocole compagnon** : mêmes réponses, octet pour octet, aux mêmes commandes. Les seules différences permises sont ces **écarts volontaires**, invisibles pour l'app :
  1. la file des messages en attente survit au redémarrage (gardée en flash) ;
  2. la fréquence est rapportée exacte (`869618` kHz au lieu de `869617`, effet d'une troncature flottante en C++) ;
  3. le nom d'un canal dans `CHANNEL_INFO` est complété par des zéros (le C++ y laisse des octets résiduels) ;
  4. pas de troncature des trames de 174 à 176 octets en Bluetooth (MTU d'au moins 179 négociée) ;
  5. le PIN aléatoire est tiré à chaque appairage plutôt qu'à chaque démarrage ;
  6. un message envoyé depuis l'écran de l'appareil est renvoyé par le firmware s'il n'est pas accusé (aucune app n'est là pour le faire) ; un message envoyé par l'app garde la règle C++ (aucun renvoi par le firmware).
- Tout nouvel écart passe par un ADR qui amende celui-ci.

## Conséquences

- On hérite des faiblesses cryptographiques de MeshCore (AES-128-ECB, MAC de 2 octets, clés statiques) : c'est le prix de la compatibilité. Le chiffrement solide reste l'affaire de l'app (ln-msg dans loranet).
- `mc conform` compare nos réponses à celles du Heltec en C++ : toute différence hors de la liste ci-dessus est un bug.
- Écarté : un mode « amélioré » négocié entre nœuds Rust. Il fragmenterait le réseau pour un gain que personne n'a demandé.
