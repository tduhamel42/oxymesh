# ADR-0002 — Décoder les commandes de l'app comme le C++, pas canoniquement

- Statut : accepté
- Date : 2026-11-05

## Contexte

La règle des parseurs durcis (loranet) exige un encodage canonique : `decode → encode` doit redonner les mêmes octets, sinon rejet. Or le firmware C++ accepte des commandes avec des champs finaux optionnels (`SET_RADIO_PARAMS` sans l'octet `repeat`, `GET_CONTACTS` sans `since`…), ignore les octets en trop, et répond `ERR 1` (et non `ERR 6`) quand une précondition de longueur échoue. Les apps existantes comptent sur ces tolérances.

## Décision

- `mc-proto` a deux modes de décodage :
  - **`Strict`** (côté app, outils, tests) : canonique, comme dans loranet ;
  - **`Device`** (côté appareil) : accepte exactement ce qu'accepte `MyMesh::handleCmdFrame` de v1.18, avec les mêmes codes d'erreur. Chaque tolérance est listée dans `docs/spec/companion.md` avec sa ligne C++.
- Les trames **sur l'air** restent décodées canoniquement : on garde les octets bruts quand le C++ tolère une queue (ACK de 4 à 6 octets), pour que la réécriture soit identique.
- Les règles de sûreté ne changent pas : zéro-copie, jamais de panic, jamais d'allocation.

## Conséquences

- La cible de fuzz `cmd_decode` vérifie, en mode `Device`, que toute commande acceptée se réécrit en une forme canonique décodable en mode `Strict` avec le même sens.
- Écarté : rejeter ce que le C++ accepte (casserait des apps) ; accepter plus que le C++ (comportement non testable contre la référence).
