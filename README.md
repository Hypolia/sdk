# Hypolia SDK

Contrats partagés par tous les services Hypolia.

| Crate | Rôle |
|---|---|
| `hypolia-protocol` | Contrat JSON entre lobby, jeux et orchestrateur (file, heartbeat, phases, retours au hub) |
| `hypolia-progression` | Domaine de la progression joueur (niveaux, XP, coins, stats) |

## Utilisation

Les autres repos attendent ce repo cloné à côté d'eux :

```
hypolia/
├── sdk/
├── platform/
├── lobby/
├── sheepwars/
└── towerwars/
```

Ils déclarent `hypolia-protocol = { path = "../sdk/crates/hypolia-protocol" }`. Leur CI clone `hypolia/sdk` au commit écrit dans leur fichier `sdk.ref`.

Changer un contrat :

1. Merger le changement ici.
2. Mettre le nouveau commit dans `sdk.ref` de chaque repo qui en dépend.

## Vérifier

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
