# MCP Manager v3 — gestionnaire local de serveurs MCP

Application desktop Windows en Flutter et passerelle Rust, interface française. Les points locaux sont liés exclusivement à `127.0.0.1` (port par défaut 7300) : `http://127.0.0.1:7300/mcp/<slug>`. Le relais stdio fournit POST Streamable HTTP avec corrélation JSON-RPC/timeout de 60 s, SSE par GET, et les routes historiques `/mcp/<slug>/sse` et `/messages`.

## Prérequis Windows

- Windows 10/11 x64, Visual Studio Build Tools avec workload C++/MSVC et Windows SDK.
- Rust stable avec cible MSVC (`rustup default stable-x86_64-pc-windows-msvc`).
- Flutter stable configuré pour Windows Desktop (`flutter doctor`).
- Node.js/npm et/ou Python/uv seulement si les MCP gérés en ont besoin.

## Build et tests

Depuis la racine du dépôt, exécuter `./scripts/build.ps1` dans PowerShell. Le script exécute les tests Rust, construit le cœur release, crée au besoin le runner Windows Flutter, récupère les paquets, lance `flutter analyze` et produit `dist/MCP-Hub`. Pour les étapes séparées : `cargo test -p mcp_hub_core`, `cargo build --release -p mcp_hub_core`, puis dans `flutter`: `flutter pub get`, `flutter analyze`, `flutter build windows --release`.

## Fonctionnalités et limites

- Secrets des variables d’environnement et headers chiffrés par DPAPI, portée utilisateur Windows, sérialisés avec le préfixe `dpapi:` et base64. Les anciennes valeurs `plain:` ou sans préfixe sont migrées à la lecture et enregistrées à nouveau chiffrées; une configuration DPAPI ne peut pas être déchiffrée par un autre utilisateur Windows ou sur un autre PC. L’API locale authentifiée `/admin/secret` ne révèle des valeurs qu’aux clients loopback.
- Supervision des MCP stdio: redémarrage après sortie inattendue, délais exponentiels 1/2/4/8/16/30 s, limite configurable (défaut 5), compteur remis à zéro après 60 secondes stables. Les démarrages qui échouent avant création du processus sont consignés et ne font pas de boucle de reprise.
- CPU et mémoire du processus MCP et de son arbre de descendants sont échantillonnés toutes les deux secondes. Les mesures dépendent de l’actualisation `sysinfo` et sont affichées sur les cartes.
- La réponse HTTP d’initialisation inclut un `Mcp-Session-Id` généré. Le protocole d’événements est destiné aux flux stdio MCP JSON-RPC; le support de sessions MCP plus complet (gestion d’état/capacités et négociation complète de versions) n’est pas implémenté.
- Fermeture de la fenêtre = masquage dans le tray. Menu tray « Afficher » / « Quitter »; quitter demande l’arrêt des MCP. Le lancement au démarrage est désactivé par défaut et inscrit l’application via `HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run` avec `--minimized`.
- Mammouth AI n’accepte que des MCP distants; il ne peut pas joindre les MCP locaux. Pour Claude Desktop, utiliser l’export `npx -y mcp-remote` proposé dans l’interface.

La configuration se trouve dans `%APPDATA%\\McpHub\\config.json`. Le port peut être changé dans les paramètres, mais le cœur déjà lancé ne rebinde pas à chaud: redémarrer l’application. Le token admin est émis sur stdout par le cœur et l’API d’administration l’exige. Ne pas exposer le port local au réseau.
