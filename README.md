# 🚀 MCP Manager — Le Guide Complet Pas-à-Pas

Bienvenue sur le dépôt officiel de **MCP Manager** !  
Ce guide a été rédigé pour que **n'importe qui**, même sans aucune connaissance en informatique, puisse installer, configurer et utiliser l'application en quelques minutes.

---

## 📑 Sommaire
1. [Qu'est-ce que MCP Manager ?](#-quest-ce-que-mcp-manager-)
2. [Méthode 1 : Installer facilement avec l'installateur (Recommandé)](#-méthode-1--installer-facilement-avec-linstallateur-recommandé)
3. [Méthode 2 : Compiler soi-même depuis les sources (Développeurs)](#-méthode-2--compiler-soi-même-depuis-les-sources-développeurs)
4. [Tutoriel Débutant : Installer et connecter Tailscale Funnel](#-tutoriel-débutant--installer-et-connecter-tailscale-funnel)
5. [Comment utiliser l'application au quotidien](#-comment-utiliser-lapplication-au-quotidien)
6. [Mises à jour automatiques](#-mises-à-jour-automatiques)
7. [Dépannage & Questions fréquentes (FAQ)](#-dépannage--questions-fréquentes-faq)

---

## 💡 Qu'est-ce que MCP Manager ?

Les **MCP** (*Model Context Protocol*) sont des connecteurs qui permettent aux intelligences artificielles (comme Claude, Cursor, ChatGPT, etc.) de se connecter à vos outils (fichiers, bases de données, Github, recherche web...).

**MCP Manager** transforme votre ordinateur Windows en un centre de contrôle :
- Il lance et surveille vos serveurs MCP en un clic.
- Il permet de les partager de façon sécurisée sur Internet (via Tailscale Funnel) pour les utiliser partout.
- Il gère vos mots de passe et clés secrètes avec le système de chiffrement ultrasécurisé de Windows (DPAPI).
- Il possède un logo officiel moderne et se met à jour automatiquement via GitHub.

---

## 📦 Méthode 1 : Installer facilement avec l'installateur (Recommandé)

C'est la méthode la plus simple pour les utilisateurs.

1. Allez dans l'onglet **[Releases](../../releases)** sur la droite de cette page GitHub.
2. Téléchargez le fichier **`MCP-Manager-Setup.exe`**.
3. Double-cliquez sur le fichier téléchargé.
4. Suivez l'assistant d'installation (cliquez sur *Suivant* puis *Installer*).
5. **C'est tout !** L'application est installée dans `C:\Program Files\MCP Manager`, et son raccourci officiel avec logo bleu apparaît directement sur votre Bureau.

---

## 🛠️ Méthode 2 : Compiler soi-même depuis les sources (Développeurs)

Si vous avez cloné ce dépôt git sur votre PC et souhaitez fabriquer l'exécutable vous-même :

### 1. Prérequis nécessaires
- **Windows 10 ou Windows 11 (64 bits)**
- **Mode Développeur Windows activé** (Paramètres Windows > Système > Espace développeurs > Activer le "Mode développeur" pour autoriser les liens symboliques).
- **Flutter SDK** (avec support Windows Desktop activé : `flutter config --enable-windows-desktop`).
- **Rust & Cargo** (`rustup default stable-x86_64-pc-windows-msvc`).
- **Visual Studio** avec la charge de travail *« Développement Desktop en C++ »*.
- **Inno Setup 6** (pour générer le fichier `Setup.exe`).

### 2. Commande unique de compilation
Ouvrez PowerShell dans le dossier du projet et lancez :
```powershell
# 1. Compiler le cœur Rust et l'interface Flutter
.\scripts\build.ps1

# 2. Générer l'installateur Setup.exe
$iscc = (Get-ChildItem -Path "C:\Program Files*\Inno Setup*", "$env:LOCALAPPDATA\Programs\Inno Setup*" -Filter "ISCC.exe" -Recurse | Select-Object -First 1).FullName
& "$iscc" .\scripts\installer.iss
```
L'installateur prêt à l'emploi sera généré dans `dist\MCP-Manager-Setup.exe`.

---

## 🌐 Tutoriel Débutant : Installer et connecter Tailscale Funnel

> **⚠️ À quoi ça sert ?**  
> Par défaut, vos serveurs MCP ne fonctionnent que sur votre ordinateur (`127.0.0.1`).  
> Si vous voulez y connecter des IA distantes (comme Mammouth AI, Claude sur le web, ou un service cloud), il vous faut une adresse sécurisée sur Internet. **Tailscale Funnel** crée ce pont de manière 100% sécurisée sans ouvrir de ports dangereux sur votre box internet.

### Étape 1 : Créer un compte Tailscale (Gratuit)
1. Rendez-vous sur le site officiel : [https://tailscale.com/](https://tailscale.com/)
2. Cliquez sur **« Get Started »** ou **« Try for free »**.
3. Connectez-vous avec votre compte Google, Microsoft, Apple ou GitHub.

### Étape 2 : Télécharger et installer Tailscale sur Windows
1. Téléchargez l'application officielle : [https://tailscale.com/download/windows](https://tailscale.com/download/windows)
2. Installez le logiciel.
3. Une fois lancé, une petite icône Tailscale apparaît en bas à droite à côté de l'horloge Windows. Cliquez dessus et cliquez sur **« Log in... »**.
4. Validez la connexion dans votre navigateur internet. Votre ordinateur est maintenant connecté !

### Étape 3 : Activer la fonctionnalité "Funnel" sur votre compte
> **⚠️ ATTENTION :** Par défaut, Funnel est désactivé par sécurité. Il faut l'autoriser une seule fois dans votre tableau de bord Tailscale :

1. Ouvrez votre console Tailscale : [https://login.tailscale.com/admin/settings/features](https://login.tailscale.com/admin/settings/features)
2. Dans la section **Access Controls** ou **Features**, assurez-vous que **Funnel** est autorisé pour vos machines.

### Étape 4 : Associer Tailscale dans MCP Manager
1. Ouvrez **MCP Manager** depuis votre Bureau.
2. Allez dans les **Réglages** (icône d'engrenage à gauche).
3. Cliquez sur **Vérifier / Connecter Tailscale**.
4. L'application détecte automatiquement votre nom de machine Tailscale (par ex. `mon-pc.tailnet-xyz.ts.net`) et active l'URL sécurisée HTTPS !

---

## 🖥️ Comment utiliser l'application au quotidien

### 1. Démarrer / Arrêter vos MCP
- Sur l'écran d'accueil, vous voyez la liste de vos serveurs MCP.
- Cliquez sur le bouton **Play (Démarrer)** pour lancer un serveur, ou sur **Stop** pour l'arrêter.
- Vous pouvez suivre la consommation CPU et mémoire de chaque serveur en temps réel.

### 2. Démarrage automatique des serveurs MCP à l'ouverture
- Si vous utilisez toujours les mêmes MCP, vous pouvez demander à MCP Manager de tous les allumer tout seul !
- Rendez-vous dans **Réglages** > cochez l'interrupteur **« Lancer automatiquement tous les MCP au démarrage de l'application »**.

### 3. Réduire dans la barre de notification
- Si vous fermez la fenêtre avec la croix rouge, **MCP Manager ne s'arrête pas** : il reste actif discrètement en bas à droite à côté de votre horloge Windows (avec le logo officiel).
- Faites un clic droit sur l'icône dans la barre des tâches pour :
  - **Ouvrir l'interface**.
  - **Quitter complètement** (cela éteindra proprement tous vos serveurs).

---

## 🔄 Mises à jour automatiques

MCP Manager intègre un système de mise à jour relié directement à GitHub :
1. **À chaque ouverture**, l'application contacte discrètement GitHub en arrière-plan.
2. Si une nouvelle version plus récente existe, **une fenêtre pop-up s'affiche automatiquement** pour vous proposer de la télécharger.
3. Vous pouvez aussi vérifier manuellement à tout moment dans l'onglet **Réglages** en cliquant sur le bouton **« Rechercher une mise à jour »**.

---

## ❓ Dépannage & Questions fréquentes (FAQ)

### 🔴 « Le port 7300 est déjà utilisé »
- Un autre programme utilise ce port, ou une ancienne version est restée ouverte en arrière-plan.
- Fermez MCP Manager, ouvrez le Gestionnaire des tâches Windows (Ctrl + Maj + Échap), fermez `mcp-hub-core.exe` si présent, puis relancez.

### 🔴 « Tailscale Funnel ne répond pas »
- Vérifiez que Tailscale est bien connecté (icône blanche près de l'horloge).
- Vérifiez que vous avez bien suivi l'[Étape 3](#étape-3--activer-la-fonctionnalité-funnel-sur-votre-compte) pour activer Funnel dans votre panneau d'administration Tailscale.

### 🔴 « L'icône sur le bureau n'a pas changé »
- Windows met parfois du temps à vider sa mémoire cache d'icônes. Redémarrez votre PC ou fermez votre session Windows pour que le nouveau logo s'affiche instantanément.

---

## 📜 Licence & Confidentialité
- **100% Local :** Vos configurations et secrets sont stockés localement sur votre disque dur (`%APPDATA%\MCPManager` et `%APPDATA%\McpHub`), protégés par DPAPI Windows.
- Distribué sous licence open-source.
