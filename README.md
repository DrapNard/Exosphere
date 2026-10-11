# Exosphere

Projet expérimental d’émulation Nintendo Switch **exclusivement pour Linux**, développé en Rust.

Exosphere construit progressivement les briques nécessaires à l’exécution des jeux : chargement du contenu, CPU ARM64, services système Horizon, GPU Tegra/Maxwell et rendu Vulkan. L’analyse et la compilation AOT/JIT font partie de l’architecture ; l’exécution diagnostique actuelle utilise aussi un interpréteur ARM64 de référence.

**Le projet est en développement actif : ce n’est pas encore un émulateur prêt à jouer.** Windows et macOS ne sont pas des plateformes cibles.

Maintenu par [DrapNard](https://github.com/DrapNard) · Licence [GPL-3.0-or-later](LICENSE)

## Objectif et état actuel

La priorité est de lancer et stabiliser **Snipperclips Plus** sur Linux, puis de passer à **Super Mario Odyssey**.

- Le contenu de Snipperclips est chargé et ses modules exécutent du code ARM64 dans l’environnement Horizon émulé.
- Le noyau, les services IPC, la mémoire, les fichiers et le pilote GPU sont implémentés progressivement, avec des tests de régression.
- Le backend Vulkan est testé sur des scénarios contrôlés. Ces tests ne prouvent pas le rendu d’un jeu commercial.
- **Aucune image, aucun menu et aucun gameplay de Snipperclips ne sont encore vérifiés.** L’initialisation GPU reste incomplète.
- Le diagnostic normal s’arrête sur ZCullGetCtxSize. Des essais avec des hypothèses GPU temporaires atteignent ensuite un échec de destruction d’événement ; ces hypothèses ne sont pas activées en production.
- La stabilisation de Super Mario Odyssey attend celle de Snipperclips.

La [roadmap Linux](docs/LINUX_GAME_ROADMAP.md) détaille les étapes, les preuves de fonctionnement et les blocages. Reconnaître une instruction ou réussir un test isolé ne signifie pas qu’un jeu est compatible.

## Architecture

| Partie | Rôle |
| --- | --- |
| `exosphere-ingest` | Lecture des conteneurs, vérification du contenu et chargement des exécutables. |
| CPU IR, backends et runtime | Décodage ARM64, représentation intermédiaire, compilation AOT/JIT et interprétation. |
| `exosphere-horizon` / `exosphere-vfs` | Noyau invité, appels système, services IPC, mémoire et accès aux fichiers. |
| GPU IR / Vulkan | Commandes Tegra/Maxwell, shaders, ressources et exécution graphique. |
| Plateforme, audio, média et input Linux | Intégration avec le système hôte Linux. |
| CLI / UI | Outils de développement et de diagnostic ; l’interface graphique reste une ébauche. |

Les comportements non pris en charge doivent rester explicites : une commande ignorée ou un résultat inventé ne constitue pas une implémentation correcte.

## Compiler sur Linux

Prérequis : une chaîne Rust stable récente avec Cargo, un compilateur/linker système et, pour les tests graphiques natifs, un environnement Vulkan compatible. Certains tests nécessitent aussi une session graphique ou des fonctionnalités matérielles spécifiques.

```bash
git clone https://github.com/DrapNard/Exosphere.git
cd Exosphere
cargo build -p exosphere-cli
cargo run -p exosphere-cli -- --help
cargo run -p exosphere-cli -- doctor
```

Les diagnostics de contenu/CPU ne nécessitent pas tous un affichage graphique. Un test Vulkan peut échouer si le pilote ou les capacités nécessaires ne sont pas disponibles.

## Outils de développement

Inspection d’un exécutable et de sa représentation CPU :

```bash
cargo run -p exosphere-cli -- inspect main.nso
cargo run -p exosphere-cli -- cpu-ir main.nso
```

Inspection d’un conteneur NSP personnel et exécution CPU bornée :

```bash
cargo run -p exosphere-cli -- content-inspect \
  "LocalDevStorage/game.nsp" \
  --keys "LocalDevStorage/keys/prod.keys" \
  --cpu-steps 1000000
```

Cette commande est un **diagnostic**, pas une promesse de lancement jouable. Les chemins sont des exemples ; aucun jeu ni fichier de clés n’est fourni.

Quelques tests ciblés :

```bash
cargo run -p exosphere-cli -- horizon-ipc-smoke
cargo run -p exosphere-cli -- fs-smoke
cargo run -p exosphere-cli -- vulkan-triangle-smoke --width 256 --height 256
cargo run -p exosphere-cli -- gpu-capabilities
```

Le test du triangle vérifie un rendu Vulkan hors écran et sa lecture retour, pas l’affichage de Snipperclips.

## Qualité et contributions

Le code doit rester clair et compréhensible. Les commentaires expliquent les choix, les contraintes et les comportements qui ne sont pas évidents à la lecture du code.

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Les tests natifs opt-in et les essais sur de vrais jeux sont des validations distinctes des tests unitaires. La CI du dépôt cible Linux.

Voir [CONTRIBUTING.md](CONTRIBUTING.md), la [roadmap Linux](docs/LINUX_GAME_ROADMAP.md) et la [documentation d’architecture](spec/exosphere-docs-v0.1/docs/ARCHITECTURE.md). Certains documents historiques décrivent des ambitions ou des étapes intermédiaires ; ils ne garantissent pas la compatibilité actuelle.

## Contenu local et publication

Exosphere ne distribue **ni jeux, ni firmware, ni clés de déchiffrement, ni contenu propriétaire du SDK Nintendo**. Les ressources nécessaires aux essais doivent être fournies localement par l’utilisateur, dans le respect du droit applicable.

`LocalDevStorage/` est exclu de Git, ainsi que les principaux formats de dumps et de clés. Ce dossier ne fait pas partie du dépôt public. Ne joignez jamais ces ressources à une contribution, une issue ou un journal de diagnostic.

Les références et règles relatives aux sources sont documentées dans [legal/SOURCES.md](legal/SOURCES.md).
