# Graph Report - flutter  (2026-11-13)

## Corpus Check
- 2 files · ~641 words
- Verdict: corpus is large enough that graph structure adds value.
- Unclassified: 2 file(s) not represented in the graph (top: (none) 1, .ico 1)

## Summary
- 74 nodes · 76 edges · 5 communities (4 shown, 1 thin omitted)
- Extraction: 100% EXTRACTED · 0% INFERRED · 0% AMBIGUOUS
- Token cost: 0 input · 0 output

## Community Hubs (Navigation)
- main.dart
- pubspec.yaml
- _HubHomeState
- dependencies
- HubApp

## God Nodes (most connected - your core abstractions)
1. `dependencies` - 6 edges
2. `_HubHomeState` - 5 edges
3. `HubHome` - 3 edges
4. `ServerDialog` - 3 edges
5. `_ServerDialogState` - 3 edges
6. `dev_dependencies` - 3 edges
7. `flutter` - 3 edges
8. `HubApp` - 2 edges
9. `environment` - 2 edges
10. `flutter` - 2 edges

## Surprising Connections (you probably didn't know these)
- None detected - all connections are within the same source files.

## Import Cycles
- None detected.

## Communities (5 total, 1 thin omitted)

### Community 0 - "main.dart"
Cohesion: 0.05
Nodes (30): _button, ClipboardHelper, _connect, _copy, _copySecret, core, _dashboard, _delete (+22 more)

### Community 1 - "pubspec.yaml"
Cohesion: 0.14
Nodes (13): description, dev_dependencies, flutter_lints, flutter_test, environment, sdk, flutter, assets (+5 more)

### Community 2 - "_HubHomeState"
Cohesion: 0.29
Nodes (4): HubHome, _HubHomeState, ServerDialog, _ServerDialogState

### Community 3 - "dependencies"
Cohesion: 0.29
Nodes (7): dependencies, file_picker, flutter, http, tray_manager, window_manager, sdk

## Knowledge Gaps
- **30 isolated node(s):** `ClipboardHelper`, `startMinimized`, `ink`, `core`, `headers` (+25 more)
  These have ≤1 connection - possible missing edges. (Counts symbols only; 59 node(s) total have ≤1 connection when file, concept and rationale nodes are included.)
- **1 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `_HubHomeState` connect `_HubHomeState` to `main.dart`?**
  _High betweenness centrality (0.048) - this node is a cross-community bridge._
- **What connects `ClipboardHelper`, `startMinimized`, `ink` to the rest of the system?**
  _30 weakly-connected nodes found - possible missing edges._
- **Should `main.dart` be split into smaller, more focused modules?**
  _Cohesion score 0.046511627906976744 - nodes in this community are weakly interconnected._
- **Why does `dependencies` connect `dependencies` to `pubspec.yaml`?**
  _High betweenness centrality (0.037) - this node is a cross-community bridge._
- **Should `pubspec.yaml` be split into smaller, more focused modules?**
  _Cohesion score 0.14285714285714285 - nodes in this community are weakly interconnected._