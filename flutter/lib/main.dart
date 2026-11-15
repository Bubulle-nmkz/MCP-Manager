import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:http/http.dart' as http;
import 'package:tray_manager/tray_manager.dart';
import 'package:window_manager/window_manager.dart';

bool startMinimized = false;
const ink = Color(0xFF0E1514),
    panel = Color(0xFF16211E),
    panelAlt = Color(0xFF1D2B27),
    line = Color(0xFF283833),
    mint = Color(0xFFB9F36B),
    cyan = Color(0xFF5EEAD4),
    muted = Color(0xFF91A19A);

void main(List<String> args) async {
  startMinimized = args.contains('--minimized');
  WidgetsFlutterBinding.ensureInitialized();
  if (Platform.isWindows) {
    await windowManager.ensureInitialized();
    await windowManager.setPreventClose(true);
  }
  runApp(const HubApp());
}

class HubApp extends StatelessWidget {
  const HubApp({super.key});
  @override
  Widget build(BuildContext context) => MaterialApp(
        debugShowCheckedModeBanner: false,
        title: 'MCP Manager v3',
        theme: ThemeData(
          useMaterial3: true,
          brightness: Brightness.dark,
          scaffoldBackgroundColor: ink,
          colorScheme: const ColorScheme.dark(
            primary: mint,
            secondary: cyan,
            surface: panel,
          ),
          fontFamily: 'Segoe UI',
          inputDecorationTheme: InputDecorationTheme(
            filled: true,
            fillColor: ink,
            contentPadding:
                const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
            border: OutlineInputBorder(
              borderRadius: BorderRadius.circular(12),
              borderSide: const BorderSide(color: line),
            ),
            enabledBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(12),
              borderSide: const BorderSide(color: line),
            ),
            focusedBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(12),
              borderSide: const BorderSide(color: mint, width: 1.4),
            ),
          ),
        ),
        home: const HubHome(),
      );
}

class HubHome extends StatefulWidget {
  const HubHome({super.key});
  @override
  State<HubHome> createState() => _HubHomeState();
}

class _HubHomeState extends State<HubHome> with WindowListener, TrayListener {
  Process? core;
  String token = '';
  String base = 'http://127.0.0.1:7300';
  String publicBaseUrl = '';
  int port = 7300;
  List<Map<String, dynamic>> servers = [];
  String page = 'overview', query = '';
  bool busy = true, connected = false, startWindows = false, enableTunnel = true;
  bool autoStartAllServers = false;
  bool hasTriggeredAutoStart = false;
  bool isCheckingUpdates = false;
  static const String appVersion = 'v1.0.2';
  static const String githubRepo = 'Bubulle-nmkz/MCP-Manager';
  String tunnelProvider = 'tailscale';
  final TextEditingController portController = TextEditingController();
  final TextEditingController publicUrlController = TextEditingController();
  final TextEditingController ngrokDomainController = TextEditingController();
  final TextEditingController ngrokTokenController = TextEditingController();
  Timer? timer;

  @override
  void initState() {
    super.initState();
    if (Platform.isWindows) {
      windowManager.addListener(this);
      trayManager.addListener(this);
      _setupTray();
    }
    _loadAutoStartSetting();
    _connect();
    Future.delayed(const Duration(seconds: 3), () => _checkForUpdates(manual: false));
  }

  File _getAutoStartConfigFile() {
    final appData = Platform.environment['APPDATA'] ?? Directory.systemTemp.path;
    final dir = Directory('$appData${Platform.pathSeparator}MCPManager');
    if (!dir.existsSync()) {
      dir.createSync(recursive: true);
    }
    return File('${dir.path}${Platform.pathSeparator}auto_start_servers.json');
  }

  Future<void> _loadAutoStartSetting() async {
    try {
      final f = _getAutoStartConfigFile();
      if (await f.exists()) {
        final j = jsonDecode(await f.readAsString());
        if (mounted) {
          setState(() => autoStartAllServers = j['autoStartAllServers'] == true);
        }
      }
    } catch (_) {}
  }

  @override
  void onWindowClose() async {
    await windowManager.hide();
  }

  Future<void> _setupTray() async {
    try {
      final data = await rootBundle.load('assets/mcp-manager.ico');
      final dir =
          await Directory.systemTemp.createTemp('mcp-manager-tray-');
      final icon =
          File('${dir.path}${Platform.pathSeparator}mcp-manager.ico');
      await icon.writeAsBytes(data.buffer.asUint8List());
      await trayManager.setIcon(icon.path);
      await trayManager.setToolTip('MCP Manager');
      await trayManager.setContextMenu(Menu(items: [
        MenuItem(key: 'show', label: 'Afficher'),
        MenuItem.separator(),
        MenuItem(key: 'quit', label: 'Quitter'),
      ]));
      if (startMinimized) await windowManager.hide();
    } catch (_) {}
  }

  @override
  void onTrayIconMouseDown() async {
    await windowManager.show();
    await windowManager.focus();
  }

  @override
  void onTrayIconRightMouseDown() async {
    await trayManager.popUpContextMenu();
  }

  @override
  void onTrayMenuItemClick(MenuItem item) async {
    if (item.key == 'show') {
      await windowManager.show();
      await windowManager.focus();
    } else if (item.key == 'quit') {
      await _quit();
    }
  }

  Future<void> _quit() async {
    timer?.cancel();
    try {
      await http
          .post(Uri.parse('$base/admin/shutdown'), headers: headers)
          .timeout(const Duration(seconds: 3));
    } catch (_) {}
    for (final s in servers) {
      try {
        await http
            .post(Uri.parse('$base/admin/servers/${s['id']}/stop'),
                headers: headers)
            .timeout(const Duration(seconds: 2));
      } catch (_) {}
    }
    try {
      await trayManager.destroy();
    } catch (_) {}
    if (core != null) {
      final pid = core!.pid;
      if (Platform.isWindows && pid > 0) {
        try {
          await Process.run('taskkill', ['/F', '/T', '/PID', '$pid']);
        } catch (_) {}
      }
      core!.kill(ProcessSignal.sigterm);
    }
    if (Platform.isWindows) {
      try {
        await Process.run('taskkill', ['/F', '/T', '/IM', 'mcp-hub-core.exe']);
      } catch (_) {}
    }
    await windowManager.setPreventClose(false);
    exit(0);
  }

  Future<void> _connect() async {
    try {
      final p = Platform.resolvedExecutable;
      final exe = File(
          '${File(p).parent.path}${Platform.pathSeparator}mcp-hub-core.exe');
      if (await exe.exists()) {
        core = await Process.start(exe.path, [], runInShell: false);
        core!.stdout
            .transform(utf8.decoder)
            .transform(const LineSplitter())
            .listen((l) {
          final i = l.indexOf('MCP_HUB_TOKEN=');
          if (i >= 0) {
            final payload = l.substring(i + 14).trim();
            final parts = payload.split(';');
            token = parts.first;
            if (parts.length > 1) {
              final m = RegExp(r'MCP_HUB_PORT=(\d+)').firstMatch(parts[1]);
              if (m != null) {
                port = int.parse(m.group(1)!);
                base = 'http://127.0.0.1:$port';
              }
            }
            _load();
          }
        });
      } else {
        token = Platform.environment['MCP_HUB_TOKEN'] ?? '';
        await _load();
      }
      timer = Timer.periodic(const Duration(seconds: 2), (_) => _load());
    } catch (_) {
      if (mounted) setState(() => busy = false);
    }
  }

  Map<String, String> get headers =>
      {'content-type': 'application/json', 'x-mcp-hub-token': token};

  Future<void> _load() async {
    if (token.isEmpty) return;
    try {
      final r = await http
          .get(Uri.parse('$base/admin/state'), headers: headers)
          .timeout(const Duration(seconds: 2));
      if (r.statusCode == 200) {
        final j = jsonDecode(r.body);
        if (mounted) {
          setState(() {
            port = j['port'] ?? 7300;
            base = 'http://127.0.0.1:$port';
            publicBaseUrl = (j['publicBaseUrl'] ?? '').toString();
            if (portController.text.isEmpty) {
              portController.text = '$port';
            }
            if (publicUrlController.text.isEmpty && publicBaseUrl.isNotEmpty) {
              publicUrlController.text = publicBaseUrl;
            }
            tunnelProvider = (j['tunnelProvider'] ?? 'tailscale').toString();
            enableTunnel = j['enableTunnel'] ?? true;
            if (ngrokDomainController.text.isEmpty &&
                (j['ngrokDomain'] ?? '').toString().isNotEmpty) {
              ngrokDomainController.text = j['ngrokDomain'].toString();
            }
            startWindows = j['startWithWindows'] ?? false;
            servers = List<Map<String, dynamic>>.from(j['servers'] ?? []);
            busy = false;
            connected = true;
          });
          if (autoStartAllServers && !hasTriggeredAutoStart && servers.isNotEmpty) {
            hasTriggeredAutoStart = true;
            _triggerAutoStartAll();
          }
        }
      }
    } catch (_) {
      if (mounted) setState(() => connected = false);
    }
  }

  Future<void> _power(Map<String, dynamic> s, bool on) async {
    await http.post(
      Uri.parse('$base/admin/servers/${s['id']}/${on ? 'start' : 'stop'}'),
      headers: headers,
    );
    await _load();
    if (on && mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          backgroundColor: panelAlt,
          content: Text(
            '${s['name']} démarré — nouveau token de session généré (URL fixe inchangée).',
            style: const TextStyle(color: Colors.white),
          ),
        ),
      );
    }
  }

  Future<void> _restart(Map<String, dynamic> s) async {
    await http.post(
      Uri.parse('$base/admin/servers/${s['id']}/restart'),
      headers: headers,
    );
    await _load();
    if (mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          backgroundColor: panelAlt,
          content: Text(
            '${s['name']} redémarré — token de connexion régénéré !',
            style: const TextStyle(color: mint),
          ),
        ),
      );
    }
  }

  Future<void> _rotateToken(Map<String, dynamic> s) async {
    final r = await http.post(
      Uri.parse('$base/admin/servers/${s['id']}/rotate-token'),
      headers: headers,
    );
    await _load();
    if (r.statusCode == 200 && mounted) {
      final tok = jsonDecode(r.body)['connectionToken'] ?? '';
      _copy(tok.toString(),
          label: 'Nouveau token généré et copié dans le presse-papiers');
    }
  }

  Future<void> _delete(Map<String, dynamic> s) async {
    await http.delete(
      Uri.parse('$base/admin/servers/${s['id']}'),
      headers: headers,
    );
    await _load();
  }

  Future<void> _save(Map<String, dynamic> s, {bool startNow = false}) async {
    final r = await http.post(
      Uri.parse('$base/admin/servers'),
      headers: headers,
      body: jsonEncode(s),
    );
    if (r.statusCode == 200 && startNow) {
      final saved = jsonDecode(r.body);
      final id = saved['id'];
      if (id != null && id.toString().isNotEmpty) {
        await http.post(
          Uri.parse('$base/admin/servers/$id/start'),
          headers: headers,
        );
      }
    }
    await _load();
  }

  Future<void> _openUniversalImport() async {
    await showDialog<void>(
      context: context,
      builder: (_) => UniversalImportDialog(
        base: base,
        headers: headers,
        onImported: () async {
          await _load();
        },
      ),
    );
  }

  Future<void> _edit([Map<String, dynamic>? data]) async {
    final result = await showDialog<Map<String, dynamic>>(
      context: context,
      builder: (_) => ServerDialog(initial: data, base: base, token: token),
    );
    if (result != null) await _save(result);
  }

  Future<void> _export(bool claude) async {
    final config = claude
        ? {
            'mcpServers': {
              for (final s in servers)
                s['name']: {
                  'command': 'npx',
                  'args': [
                    '-y',
                    'mcp-remote',
                    (s['localUrl'] ?? '$base/mcp/${s['slug']}').toString(),
                    '--header',
                    'Authorization: Bearer ${s['connectionToken'] ?? ''}',
                  ]
                }
            }
          }
        : {
            'mammouth_ai_urls': {
              for (final s in servers)
                s['name']: (s['mammouthUrl'] ??
                        '$publicBaseUrl/mcp/${s['slug']}/${s['connectionToken'] ?? ''}/mcp')
                    .toString(),
            },
            'streamable_http_servers': servers
                .map((s) => {
                      'name': s['name'],
                      'mammouth_url_direct': s['mammouthUrl'],
                      'url': s['fixedUrl'] ?? '$base/mcp/${s['slug']}',
                      'headers': {
                        'Authorization': 'Bearer ${s['connectionToken'] ?? ''}'
                      }
                    })
                .toList()
          };
    final text = const JsonEncoder.withIndent('  ').convert(config);
    if (!mounted) return;
    await showDialog<void>(
      context: context,
      builder: (_) => AlertDialog(
        backgroundColor: panel,
        title: Text(
            claude ? 'Export Claude Desktop' : 'Export client HTTP / Mammouth'),
        content: SizedBox(
          width: 560,
          child: SelectableText(
            text,
            style: const TextStyle(fontFamily: 'Consolas', fontSize: 12),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Fermer'),
          ),
          FilledButton.icon(
            onPressed: () {
              _copy(text);
              Navigator.pop(context);
            },
            icon: const Icon(Icons.copy),
            label: const Text('Copier'),
          ),
        ],
      ),
    );
  }

  void _copy(String s, {String label = 'Copié dans le presse-papiers'}) {
    Clipboard.setData(ClipboardData(text: s));
    if (mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(label)));
    }
  }

  @override
  void dispose() {
    timer?.cancel();
    portController.dispose();
    publicUrlController.dispose();
    ngrokDomainController.dispose();
    ngrokTokenController.dispose();
    if (Platform.isWindows) {
      windowManager.removeListener(this);
      trayManager.removeListener(this);
    }
    super.dispose();
  }
  @override
  Widget build(BuildContext context) {
    final visible = servers
        .where((s) => (s['name'] ?? '')
            .toString()
            .toLowerCase()
            .contains(query.toLowerCase()))
        .toList();
    return Scaffold(
      body: Row(
        children: [
          NavigationRail(
            backgroundColor: const Color(0xFF111A18),
            selectedIndex: ['overview', 'logs', 'settings']
                .indexOf(page)
                .clamp(0, 2),
            onDestinationSelected: (i) =>
                setState(() => page = ['overview', 'logs', 'settings'][i]),
            labelType: NavigationRailLabelType.all,
            leading: Padding(
              padding: const EdgeInsets.fromLTRB(0, 20, 0, 28),
              child: ClipRRect(
                borderRadius: BorderRadius.circular(13),
                child: Image.asset(
                  'assets/mcp-manager.png',
                  width: 44,
                  height: 44,
                  fit: BoxFit.cover,
                  errorBuilder: (_, __, ___) => Container(
                    width: 44,
                    height: 44,
                    decoration: BoxDecoration(
                      color: mint,
                      borderRadius: BorderRadius.circular(13),
                    ),
                    child: const Icon(Icons.hub_rounded, color: ink),
                  ),
                ),
              ),
            ),
            destinations: const [
              NavigationRailDestination(
                icon: Icon(Icons.grid_view_rounded),
                label: Text('Serveurs'),
              ),
              NavigationRailDestination(
                icon: Icon(Icons.subject_rounded),
                label: Text('Journaux'),
              ),
              NavigationRailDestination(
                icon: Icon(Icons.tune_rounded),
                label: Text('Réglages'),
              ),
            ],
          ),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(32, 26, 32, 22),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _topbar(),
                  const SizedBox(height: 24),
                  Expanded(
                    child: page == 'overview'
                        ? _dashboard(visible)
                        : page == 'logs'
                            ? _logs()
                            : _settings(),
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _topbar() => Row(
        children: [
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                page == 'overview'
                    ? 'MCP Manager v3'
                    : page == 'logs'
                        ? 'Journaux en direct'
                        : 'Paramètres de la passerelle',
                style: const TextStyle(
                  fontSize: 26,
                  fontWeight: FontWeight.w700,
                  letterSpacing: -.7,
                ),
              ),
              const SizedBox(height: 4),
              const Text(
                'Importez n’importe quel code/fichier MCP · URL fixe permanente · Token persistant & régénérable à chaud',
                style: TextStyle(color: muted, fontSize: 12.5),
              ),
            ],
          ),
          const Spacer(),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
            decoration: BoxDecoration(
              color: panel,
              borderRadius: BorderRadius.circular(30),
              border: Border.all(color: line),
            ),
            child: Row(
              children: [
                Container(
                  width: 8,
                  height: 8,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: connected ? mint : Colors.orange,
                  ),
                ),
                const SizedBox(width: 8),
                Text(
                  connected ? 'PASSERELLE ACTIVE' : 'CONNEXION…',
                  style: const TextStyle(
                    fontSize: 10,
                    letterSpacing: 1,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          FilledButton.icon(
            onPressed: _openUniversalImport,
            icon: const Icon(Icons.bolt_rounded, size: 18),
            label: const Text('Ajout / Import Auto MCP'),
            style: FilledButton.styleFrom(
              backgroundColor: mint,
              foregroundColor: ink,
              padding:
                  const EdgeInsets.symmetric(horizontal: 18, vertical: 16),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(11),
              ),
            ),
          ),
          const SizedBox(width: 10),
          OutlinedButton.icon(
            onPressed: () => _edit(),
            icon: const Icon(Icons.add, size: 17),
            label: const Text('Manuel'),
            style: OutlinedButton.styleFrom(
              foregroundColor: mint,
              side: const BorderSide(color: line),
              padding:
                  const EdgeInsets.symmetric(horizontal: 14, vertical: 16),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(11),
              ),
            ),
          ),
        ],
      );

  Widget _dashboard(List<Map<String, dynamic>> list) {
    final running = servers.where((s) => s['running'] == true).length;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            _metric('SERVEURS MCP', servers.length.toString(),
                Icons.hub_outlined),
            const SizedBox(width: 14),
            _metric('EN LIGNE', running.toString(), Icons.bolt_rounded,
                accent: true),
            const SizedBox(width: 14),
            _metric('URL PASSERELLE FIXE', '127.0.0.1:$port',
                Icons.lock_outline_rounded),
          ],
        ),
        const SizedBox(height: 22),
        Row(
          children: [
            const Text(
              'Vos serveurs MCP',
              style: TextStyle(fontSize: 18, fontWeight: FontWeight.w700),
            ),
            const Spacer(),
            SizedBox(
              width: 240,
              height: 40,
              child: TextField(
                onChanged: (v) => setState(() => query = v),
                decoration: const InputDecoration(
                  hintText: 'Rechercher un MCP…',
                  prefixIcon: Icon(Icons.search, size: 18),
                  contentPadding: EdgeInsets.zero,
                ),
              ),
            ),
            const SizedBox(width: 10),
            PopupMenuButton<bool>(
              tooltip: 'Exporter la configuration',
              onSelected: _export,
              itemBuilder: (_) => const [
                PopupMenuItem(
                  value: true,
                  child: Text('Claude Desktop (URL fixe + Bearer Token)'),
                ),
                PopupMenuItem(
                  value: false,
                  child: Text('Client HTTP / Mammouth (URL fixe + Header)'),
                ),
              ],
              child: Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 13, vertical: 10),
                decoration: BoxDecoration(
                  border: Border.all(color: line),
                  borderRadius: BorderRadius.circular(10),
                ),
                child: const Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(Icons.ios_share, size: 16, color: mint),
                    SizedBox(width: 7),
                    Text('Exporter', style: TextStyle(color: mint)),
                  ],
                ),
              ),
            ),
          ],
        ),
        const SizedBox(height: 14),
        Expanded(
          child: busy
              ? const Center(child: CircularProgressIndicator(color: mint))
              : list.isEmpty
                  ? _empty()
                  : GridView.builder(
                      gridDelegate:
                          const SliverGridDelegateWithMaxCrossAxisExtent(
                        maxCrossAxisExtent: 470,
                        mainAxisExtent: 268,
                        crossAxisSpacing: 15,
                        mainAxisSpacing: 15,
                      ),
                      itemCount: list.length,
                      itemBuilder: (_, i) => _serverCard(list[i]),
                    ),
        ),
      ],
    );
  }

  Widget _metric(String title, String value, IconData icon,
          {bool accent = false}) =>
      Expanded(
        child: Container(
          height: 82,
          padding: const EdgeInsets.symmetric(horizontal: 18),
          decoration: BoxDecoration(
            color: panel,
            borderRadius: BorderRadius.circular(15),
            border: Border.all(color: line),
          ),
          child: Row(
            children: [
              Container(
                width: 40,
                height: 40,
                decoration: BoxDecoration(
                  color: accent ? mint.withValues(alpha: .13) : ink,
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Icon(icon, color: accent ? mint : muted, size: 19),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      title,
                      style: const TextStyle(
                        color: muted,
                        fontSize: 10,
                        letterSpacing: 1.2,
                        fontWeight: FontWeight.bold,
                      ),
                    ),
                    const SizedBox(height: 4),
                    Text(
                      value,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        fontSize: 19,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      );

  Widget _serverCard(Map<String, dynamic> s) {
    final on = s['running'] == true;
    final fixedUrl = (s['fixedUrl'] ?? '$base/mcp/${s['slug']}').toString();
    final mammouthUrl =
        (s['mammouthUrl'] ?? '$fixedUrl/${s['connectionToken'] ?? ''}/mcp')
            .toString();
    final connToken =
        (s['connectionToken'] ?? s['activeToken'] ?? '').toString();
    final kind = (s['kind'] ?? 'stdio').toString();
    final displayUrl = kind == 'remote' ? mammouthUrl : fixedUrl;
    final urlLabel = kind == 'remote' ? 'URL MAMMOUTH :' : 'URL FIXE :';
    final kindLabel = kind == 'local_http'
        ? 'HTTP SERVEUR'
        : kind == 'remote'
            ? 'DISTANT HTTPS'
            : 'LOCAL STDIO';
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: panel,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: on ? mint.withValues(alpha: .38) : line),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: .15),
            blurRadius: 16,
            offset: const Offset(0, 6),
          )
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Container(
                width: 38,
                height: 38,
                decoration: BoxDecoration(
                  color: on ? mint.withValues(alpha: .12) : ink,
                  borderRadius: BorderRadius.circular(11),
                ),
                child: Icon(
                  kind == 'remote'
                      ? Icons.public_rounded
                      : kind == 'local_http'
                          ? Icons.dns_rounded
                          : Icons.terminal_rounded,
                  color: on ? mint : muted,
                  size: 19,
                ),
              ),
              const SizedBox(width: 11),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Flexible(
                          child: Text(
                            (s['name'] ?? 'Serveur').toString(),
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(
                              fontWeight: FontWeight.w700,
                              fontSize: 15,
                            ),
                          ),
                        ),
                        const SizedBox(width: 8),
                        Container(
                          padding: const EdgeInsets.symmetric(
                              horizontal: 7, vertical: 2),
                          decoration: BoxDecoration(
                            color: ink,
                            borderRadius: BorderRadius.circular(6),
                            border: Border.all(color: line),
                          ),
                          child: Text(
                            kindLabel,
                            style: const TextStyle(
                                fontSize: 9,
                                color: cyan,
                                fontWeight: FontWeight.w700),
                          ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 3),
                    Row(
                      children: [
                        Container(
                          width: 6,
                          height: 6,
                          decoration: BoxDecoration(
                            shape: BoxShape.circle,
                            color: on ? mint : muted,
                          ),
                        ),
                        const SizedBox(width: 6),
                        Text(
                          s['restarting'] == true
                              ? 'REDÉMARRAGE (${s['restartAttempt'] ?? 0}/${s['maxRetries'] ?? 5})'
                              : on
                                  ? 'EN LIGNE · PRÊT'
                                  : 'À L’ARRÊT',
                          style: TextStyle(
                            fontSize: 9.5,
                            color: on ? mint : muted,
                            letterSpacing: 1.0,
                            fontWeight: FontWeight.bold,
                          ),
                        ),
                      ],
                    ),
                  ],
                ),
              ),
              IconButton(
                tooltip: 'Redémarrer (génère un nouveau token)',
                onPressed: () => _restart(s),
                icon: const Icon(Icons.restart_alt_rounded,
                    size: 19, color: cyan),
              ),
              Switch.adaptive(
                value: on,
                activeThumbColor: mint,
                onChanged: (v) => _power(s, v),
              ),
            ],
          ),
          const SizedBox(height: 12),
          // Fixed URL Row (Never changes!)
          Container(
            width: double.infinity,
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
            decoration: BoxDecoration(
              color: ink,
              borderRadius: BorderRadius.circular(9),
              border: Border.all(color: line.withValues(alpha: .7)),
            ),
            child: Row(
              children: [
                const Icon(Icons.link_rounded, size: 14, color: mint),
                const SizedBox(width: 7),
                Text(
                  urlLabel,
                  style: const TextStyle(
                    fontSize: 9.5,
                    fontWeight: FontWeight.w800,
                    color: mint,
                  ),
                ),
                const SizedBox(width: 7),
                Expanded(
                  child: Text(
                    displayUrl,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      fontSize: 11,
                      color: Colors.white,
                      fontFamily: 'Consolas',
                    ),
                  ),
                ),
                InkWell(
                  onTap: () => _copy(mammouthUrl,
                      label:
                          'URL Mammouth AI complète (avec secret /mcp) copiée !'),
                  child: const Tooltip(
                    message:
                        'Copier l’URL complète pour Mammouth AI (inclut le token + /mcp)',
                    child: Padding(
                      padding: EdgeInsets.symmetric(horizontal: 4),
                      child: Icon(Icons.cloud_done_rounded,
                          size: 15, color: cyan),
                    ),
                  ),
                ),
                InkWell(
                  onTap: () =>
                      _copy(displayUrl, label: 'URL copiée dans le presse-papiers !'),
                  child: const Padding(
                    padding: EdgeInsets.symmetric(horizontal: 4),
                    child: Icon(Icons.copy_rounded, size: 14, color: mint),
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(height: 8),
          // Rotating Token Row (Changes on every restart!)
          Container(
            width: double.infinity,
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
            decoration: BoxDecoration(
              color: ink,
              borderRadius: BorderRadius.circular(9),
              border: Border.all(color: cyan.withValues(alpha: .35)),
            ),
            child: Row(
              children: [
                const Icon(Icons.vpn_key_rounded, size: 13, color: cyan),
                const SizedBox(width: 7),
                const Text(
                  'TOKEN :',
                  style: TextStyle(
                    fontSize: 9.5,
                    fontWeight: FontWeight.w800,
                    color: cyan,
                  ),
                ),
                const SizedBox(width: 7),
                Expanded(
                  child: Text(
                    connToken.isEmpty ? 'Génération au démarrage…' : connToken,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      fontSize: 11,
                      color: cyan,
                      fontFamily: 'Consolas',
                    ),
                  ),
                ),
                InkWell(
                  onTap: () => _rotateToken(s),
                  child: const Tooltip(
                    message: 'Régénérer le token maintenant',
                    child: Padding(
                      padding: EdgeInsets.symmetric(horizontal: 4),
                      child: Icon(Icons.refresh_rounded, size: 15, color: cyan),
                    ),
                  ),
                ),
                InkWell(
                  onTap: () => _copy(connToken,
                      label: 'Token de connexion actuel copié !'),
                  child: const Tooltip(
                    message: 'Copier le token',
                    child: Padding(
                      padding: EdgeInsets.symmetric(horizontal: 4),
                      child: Icon(Icons.copy_rounded, size: 14, color: cyan),
                    ),
                  ),
                ),
              ],
            ),
          ),
          const Spacer(),
          Row(
            children: [
              _tiny(Icons.memory, '${s['cpu'] ?? 0}%'),
              const SizedBox(width: 12),
              _tiny(Icons.storage_rounded, '${s['memoryMb'] ?? 0} MB'),
              if ((s['cwd'] ?? '').toString().isNotEmpty) ...[
                const SizedBox(width: 12),
                Expanded(
                  child: Text(
                    (s['cwd'] ?? '').toString(),
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      fontSize: 10,
                      color: muted,
                      fontFamily: 'Consolas',
                    ),
                  ),
                ),
              ] else
                const Spacer(),
              IconButton(
                tooltip: 'Modifier',
                visualDensity: VisualDensity.compact,
                onPressed: () => _edit(s),
                icon: const Icon(Icons.edit_outlined, size: 17, color: muted),
              ),
              IconButton(
                tooltip: 'Supprimer',
                visualDensity: VisualDensity.compact,
                onPressed: () => _delete(s),
                icon: const Icon(Icons.delete_outline, size: 17, color: muted),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _tiny(IconData i, String t) => Row(
        children: [
          Icon(i, size: 13, color: muted),
          const SizedBox(width: 4),
          Text(t, style: const TextStyle(fontSize: 11, color: muted)),
        ],
      );

  Widget _empty() => Center(
        child: Container(
          constraints: const BoxConstraints(maxWidth: 520),
          padding: const EdgeInsets.all(28),
          decoration: BoxDecoration(
            color: panel,
            borderRadius: BorderRadius.circular(18),
            border: Border.all(color: line),
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.auto_awesome_motion_rounded,
                  size: 48, color: mint.withValues(alpha: .85)),
              const SizedBox(height: 14),
              const Text(
                'Aucun MCP pré-installé — Prêt pour vos imports',
                textAlign: TextAlign.center,
                style: TextStyle(fontWeight: FontWeight.w700, fontSize: 18),
              ),
              const SizedBox(height: 8),
              const Text(
                'Envoyez un dossier MCP, une archive .zip, un fichier isolé (.js, .py, .ts) ou collez directement le code entier d’un MCP : il sera configuré, installé et lancé automatiquement avec une URL fixe permanente et un token dynamique.',
                textAlign: TextAlign.center,
                style: TextStyle(color: muted, fontSize: 13, height: 1.4),
              ),
              const SizedBox(height: 20),
              FilledButton.icon(
                onPressed: _openUniversalImport,
                icon: const Icon(Icons.bolt_rounded),
                label: const Text('Importer / Coller un MCP maintenant'),
                style: FilledButton.styleFrom(
                  backgroundColor: mint,
                  foregroundColor: ink,
                  padding:
                      const EdgeInsets.symmetric(horizontal: 20, vertical: 16),
                ),
              ),
            ],
          ),
        ),
      );

  Widget _logs() => ListView(
        children: servers
            .map(
              (s) => FutureBuilder<http.Response>(
                future: http.get(
                    Uri.parse('$base/admin/logs/${s['id']}'),
                    headers: headers),
                builder: (_, snap) {
                  String text = 'Chargement des journaux…';
                  if (snap.hasData) {
                    try {
                      final list =
                          List<String>.from(jsonDecode(snap.data!.body));
                      text = list.isEmpty
                          ? 'Aucun journal pour le moment.'
                          : list.join('\n');
                    } catch (_) {
                      text = snap.data!.body;
                    }
                  }
                  return Container(
                    margin: const EdgeInsets.only(bottom: 14),
                    padding: const EdgeInsets.all(17),
                    decoration: BoxDecoration(
                      color: panel,
                      borderRadius: BorderRadius.circular(14),
                      border: Border.all(color: line),
                    ),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            Text(
                              (s['name'] ?? 'Serveur').toString(),
                              style: const TextStyle(
                                  color: mint, fontWeight: FontWeight.bold),
                            ),
                            const Spacer(),
                            Text(
                              'URL fixe : ${s['fixedUrl'] ?? ''}',
                              style: const TextStyle(
                                  fontFamily: 'Consolas',
                                  fontSize: 11,
                                  color: muted),
                            ),
                          ],
                        ),
                        const SizedBox(height: 10),
                        SelectableText(
                          text,
                          style: const TextStyle(
                              fontFamily: 'Consolas',
                              fontSize: 12,
                              color: muted),
                        ),
                      ],
                    ),
                  );
                },
              ),
            )
            .toList(),
      );

  Widget _settings() => Container(
        padding: const EdgeInsets.all(24),
        decoration: BoxDecoration(
          color: panel,
          borderRadius: BorderRadius.circular(16),
          border: Border.all(color: line),
        ),
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                'Passerelle locale & URL fixe permanente',
                style: TextStyle(fontSize: 18, fontWeight: FontWeight.bold),
              ),
              const SizedBox(height: 8),
              const Text(
                'Chaque MCP dispose d’une URL fixe permanente (/mcp/<slug>) qui ne change jamais, même après avoir éteint le PC. Seul le token de connexion change au redémarrage d’un MCP.',
                style: TextStyle(color: muted, fontSize: 13),
              ),
              const SizedBox(height: 18),
              DropdownButtonFormField<String>(
                key: ValueKey(tunnelProvider),
                initialValue: ['tailscale', 'ngrok', 'local', 'cloudflare']
                        .contains(tunnelProvider)
                    ? tunnelProvider
                    : 'tailscale',
                decoration: const InputDecoration(
                  labelText: 'Mode d’URL permanente (survit à l’extinction du PC)',
                ),
                items: const [
                  DropdownMenuItem(
                    value: 'tailscale',
                    child: Text(
                      'Tailscale Funnel (Recommandé — HTTPS fixe .ts.net gratuit & illimité)',
                    ),
                  ),
                  DropdownMenuItem(
                    value: 'ngrok',
                    child: Text(
                      'Ngrok Domaine Statique (HTTPS fixe .ngrok-free.app)',
                    ),
                  ),
                  DropdownMenuItem(
                    value: 'local',
                    child: Text(
                      'Local uniquement (100% fixe : http://127.0.0.1:7300/mcp/<slug>)',
                    ),
                  ),
                  DropdownMenuItem(
                    value: 'cloudflare',
                    child: Text(
                      'Cloudflare Tunnel (Domaine personnalisé)',
                    ),
                  ),
                ],
                onChanged: (v) {
                  if (v != null) setState(() => tunnelProvider = v);
                },
              ),
              const SizedBox(height: 14),
              if (tunnelProvider == 'tailscale')
                Container(
                  padding: const EdgeInsets.all(12),
                  decoration: BoxDecoration(
                    color: panelAlt,
                    borderRadius: BorderRadius.circular(10),
                    border: Border.all(color: cyan.withValues(alpha: 0.4)),
                  ),
                  child: const Text(
                    '⚡ Tailscale Funnel active automatiquement votre domaine permanent https://<votre-pc>.<tailnet>.ts.net sur le port 7300.\n'
                    '• Aucune configuration ni achat de domaine requis : installez simplement Tailscale et connectez-vous.\n'
                    '• Toutes vos adresses MCP (/mcp/<slug>) resteront strictement identiques à vie, même après redémarrage du PC.',
                    style: TextStyle(color: cyan, fontSize: 12.5),
                  ),
                ),
              if (tunnelProvider == 'ngrok') ...[
                Row(
                  children: [
                    Expanded(
                      child: TextField(
                        controller: ngrokDomainController,
                        decoration: const InputDecoration(
                          labelText:
                              'Domaine statique Ngrok (ex: mon-nom.ngrok-free.app)',
                        ),
                      ),
                    ),
                    const SizedBox(width: 14),
                    Expanded(
                      child: TextField(
                        controller: ngrokTokenController,
                        obscureText: true,
                        decoration: const InputDecoration(
                          labelText: 'Authtoken Ngrok (optionnel si déjà configuré)',
                        ),
                      ),
                    ),
                  ],
                ),
              ],
              const SizedBox(height: 14),
              Row(
                children: [
                  SizedBox(
                    width: 160,
                    child: TextField(
                      controller: portController,
                      keyboardType: TextInputType.number,
                      decoration:
                          const InputDecoration(labelText: 'Port local fixe'),
                    ),
                  ),
                  const SizedBox(width: 14),
                  Expanded(
                    child: TextField(
                      controller: publicUrlController,
                      decoration: const InputDecoration(
                        labelText:
                            'URL publique permanente détectée ou personnalisée (sauvegardée sur disque)',
                      ),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 12),
              SwitchListTile(
                contentPadding: EdgeInsets.zero,
                title: const Text('Démarrer avec Windows'),
                subtitle: const Text(
                  'Ajoute ou retire MCP Manager du registre HKCU\\…\\Run.',
                  style: TextStyle(color: muted, fontSize: 12),
                ),
                value: startWindows,
                onChanged: (v) => setState(() => startWindows = v),
              ),
              const SizedBox(height: 8),
              SwitchListTile(
                contentPadding: EdgeInsets.zero,
                title: const Text('Démarrer tous les MCP à l’ouverture'),
                subtitle: const Text(
                  'Lance automatiquement tous les serveurs MCP configurés dès l’ouverture de l’application (désactivé par défaut).',
                  style: TextStyle(color: muted, fontSize: 12),
                ),
                value: autoStartAllServers,
                onChanged: (v) async {
                  setState(() => autoStartAllServers = v);
                  await _saveAutoStartSetting(v);
                },
              ),
              const SizedBox(height: 12),
              Row(
                children: [
                  FilledButton.icon(
                    onPressed: () async {
                      final p = int.tryParse(portController.text);
                      if (p == null || p < 1 || p > 65535) return;
                      await http.post(
                        Uri.parse('$base/admin/settings'),
                        headers: headers,
                        body: jsonEncode({
                          'port': p,
                          'tunnelProvider': tunnelProvider,
                          'enableTunnel': tunnelProvider != 'local',
                          'ngrokDomain': ngrokDomainController.text.trim(),
                          'ngrokAuthToken': ngrokTokenController.text.trim(),
                          'publicBaseUrl': tunnelProvider == 'local'
                              ? ''
                              : publicUrlController.text.trim(),
                          'startWithWindows': startWindows,
                          'appPath': Platform.resolvedExecutable,
                        }),
                      );
                      await _load();
                      if (mounted) {
                        ScaffoldMessenger.of(context).showSnackBar(
                          const SnackBar(
                            content: Text('Paramètres d’URL permanente enregistrés avec succès.'),
                          ),
                        );
                      }
                    },
                    icon: const Icon(Icons.save_outlined),
                    label: const Text('Enregistrer'),
                  ),
                  const SizedBox(width: 14),
                  OutlinedButton.icon(
                    onPressed: isCheckingUpdates ? null : () => _checkForUpdates(manual: true),
                    icon: isCheckingUpdates
                        ? const SizedBox(
                            width: 16,
                            height: 16,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.refresh_rounded),
                    label: Text(isCheckingUpdates ? 'Recherche...' : 'Recherche de mise à jour'),
                  ),
                ],
              ),
              const SizedBox(height: 16),
              Text(
                'Passerelle locale active : $base/mcp/<slug>',
                style: const TextStyle(color: mint, fontFamily: 'Consolas'),
              ),
            ],
          ),
        ),
      );

  Future<void> _saveAutoStartSetting(bool val) async {
    setState(() => autoStartAllServers = val);
    try {
      final f = _getAutoStartConfigFile();
      await f.writeAsString(jsonEncode({'autoStartAllServers': val}));
    } catch (_) {}
  }

  Future<void> _triggerAutoStartAll() async {
    for (final s in servers) {
      final running = s['running'] == true || s['status'] == 'running';
      if (!running && s['id'] != null) {
        try {
          await http.post(
            Uri.parse('$base/admin/servers/${s['id']}/start'),
            headers: headers,
          );
        } catch (_) {}
      }
    }
    await _load();
  }

  Future<void> _checkForUpdates({bool manual = false}) async {
    if (isCheckingUpdates) return;
    setState(() => isCheckingUpdates = true);
    try {
      String tag = '';
      String htmlUrl = 'https://github.com/$githubRepo/releases';
      String body = '';
      bool reachable = false;
      String errorMsg = '';

      // 1. Requête principale : API GitHub Releases
      try {
        final releaseUrl = Uri.parse('https://api.github.com/repos/$githubRepo/releases/latest');
        final resp = await http.get(releaseUrl, headers: {
          'Accept': 'application/vnd.github.v3+json',
          'User-Agent': 'MCP-Manager-Client',
        }).timeout(const Duration(seconds: 10));

        if (resp.statusCode == 200) {
          final data = jsonDecode(resp.body);
          final rawTag = (data['tag_name'] ?? '').toString();
          tag = rawTag.replaceFirst(RegExp(r'^v', caseSensitive: false), '').trim();
          htmlUrl = (data['html_url'] ?? htmlUrl).toString();
          body = (data['body'] ?? '').toString();
          if (tag.isNotEmpty) reachable = true;
        } else {
          errorMsg = 'GitHub API code: ${resp.statusCode}';
        }
      } catch (e) {
        errorMsg = 'Erreur GitHub API: $e';
      }

      // 2. Si l'API échoue (rate-limit ou autre), fallback sur version.json de la branche main
      if (!reachable) {
        try {
          final rawUrl = Uri.parse('https://raw.githubusercontent.com/$githubRepo/main/version.json');
          final rawResp = await http.get(rawUrl, headers: {
            'User-Agent': 'MCP-Manager-Client',
          }).timeout(const Duration(seconds: 10));

          if (rawResp.statusCode == 200) {
            final data = jsonDecode(rawResp.body);
            final rawVersion = (data['version'] ?? '').toString();
            tag = rawVersion.replaceFirst(RegExp(r'^v', caseSensitive: false), '').trim();
            htmlUrl = (data['release_url'] ?? htmlUrl).toString();
            body = (data['notes'] ?? '').toString();
            if (tag.isNotEmpty) reachable = true;
          } else {
            errorMsg = 'GitHub Raw code: ${rawResp.statusCode}';
          }
        } catch (e) {
          errorMsg = 'Erreur GitHub Raw: $e';
        }
      }

      if (reachable && tag.isNotEmpty) {
        final cleanCurrent = appVersion.replaceFirst(RegExp(r'^v', caseSensitive: false), '').trim();
        final cleanTag = tag;

        // Comparaison sémantique propre (X.Y.Z)
        bool isNewer = false;
        try {
          final curParts = cleanCurrent.split('.').map((e) => int.tryParse(e) ?? 0).toList();
          final newParts = cleanTag.split('.').map((e) => int.tryParse(e) ?? 0).toList();
          while (curParts.length < 3) {
            curParts.add(0);
          }
          while (newParts.length < 3) {
            newParts.add(0);
          }

          for (int i = 0; i < 3; i++) {
            if (newParts[i] > curParts[i]) {
              isNewer = true;
              break;
            } else if (newParts[i] < curParts[i]) {
              break;
            }
          }
        } catch (_) {
          isNewer = cleanTag.isNotEmpty && cleanTag != cleanCurrent;
        }

        if (isNewer) {
          if (mounted) {
            _showUpdateDialog(cleanTag, htmlUrl, body);
          }
        } else if (manual && mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(
              content: Text('Vous utilisez déjà la dernière version disponible ($appVersion). Dernière version en ligne : v$cleanTag.'),
              backgroundColor: mint,
            ),
          );
        }
      } else if (manual && mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Impossible de vérifier les mises à jour ($errorMsg). Vérifiez votre connexion Internet.'),
            backgroundColor: Colors.orangeAccent,
          ),
        );
      }
    } catch (e) {
      if (manual && mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Erreur réseau lors de la vérification : $e'),
            backgroundColor: Colors.redAccent,
          ),
        );
      }
    } finally {
      if (mounted) setState(() => isCheckingUpdates = false);
    }
  }

  void _showUpdateDialog(String newVersion, String releaseUrl, String notes) {
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: panel,
        title: Row(
          children: [
            const Icon(Icons.system_update_alt_rounded, color: mint),
            const SizedBox(width: 10),
            Text('Mise à jour disponible : $newVersion'),
          ],
        ),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Une nouvelle version ($newVersion) est disponible sur GitHub !'),
            const SizedBox(height: 10),
            const Text('Version actuelle installée : $appVersion',
                style: TextStyle(color: muted, fontSize: 13)),
            if (notes.isNotEmpty) ...[
              const SizedBox(height: 12),
              const Text('Nouveautés :', style: TextStyle(fontWeight: FontWeight.bold)),
              const SizedBox(height: 6),
              Container(
                constraints: const BoxConstraints(maxHeight: 180),
                padding: const EdgeInsets.all(8),
                decoration: BoxDecoration(
                  color: ink,
                  borderRadius: BorderRadius.circular(6),
                  border: Border.all(color: line),
                ),
                child: SingleChildScrollView(
                  child: Text(notes, style: const TextStyle(fontSize: 12)),
                ),
              ),
            ],
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('Plus tard'),
          ),
          FilledButton.icon(
            icon: const Icon(Icons.open_in_browser_rounded, size: 16),
            onPressed: () async {
              Navigator.pop(ctx);
              final target = releaseUrl.isNotEmpty
                  ? releaseUrl
                  : 'https://github.com/$githubRepo/releases/latest';
              if (Platform.isWindows) {
                await Process.run('cmd', ['/c', 'start', '', target]);
              }
            },
            label: const Text('Mettre à jour'),
          ),
        ],
      ),
    );
  }
}
class UniversalImportDialog extends StatefulWidget {
  final String base;
  final Map<String, String> headers;
  final Future<void> Function() onImported;

  const UniversalImportDialog({
    super.key,
    required this.base,
    required this.headers,
    required this.onImported,
  });

  @override
  State<UniversalImportDialog> createState() => _UniversalImportDialogState();
}

class _UniversalImportDialogState extends State<UniversalImportDialog>
    with SingleTickerProviderStateMixin {
  late final TabController _tabController;
  final TextEditingController _nameCtrl = TextEditingController();
  final TextEditingController _codeCtrl = TextEditingController();
  final TextEditingController _workspaceCtrl = TextEditingController();
  final TextEditingController _pathCtrl = TextEditingController();
  final TextEditingController _remoteUrlCtrl = TextEditingController();
  String _lang = 'auto';
  String _importKind = 'auto';
  bool _autoStart = true;
  bool _loading = false;
  String? _statusMessage;

  @override
  void initState() {
    super.initState();
    _tabController = TabController(length: 2, vsync: this);
  }

  @override
  void dispose() {
    _tabController.dispose();
    _nameCtrl.dispose();
    _codeCtrl.dispose();
    _workspaceCtrl.dispose();
    _pathCtrl.dispose();
    _remoteUrlCtrl.dispose();
    super.dispose();
  }

  Future<void> _pickFolder() async {
    final d = await FilePicker.platform.getDirectoryPath();
    if (d != null) {
      setState(() => _pathCtrl.text = d);
    }
  }

  Future<void> _pickFile() async {
    final res = await FilePicker.platform.pickFiles(
      allowMultiple: false,
      type: FileType.any,
    );
    if (res != null && res.files.single.path != null) {
      setState(() => _pathCtrl.text = res.files.single.path!);
    }
  }

  Future<void> _pickWorkspace() async {
    final d = await FilePicker.platform.getDirectoryPath();
    if (d != null) {
      setState(() => _workspaceCtrl.text = d);
    }
  }

  Future<void> _submitPathImport() async {
    final p = _pathCtrl.text.trim();
    if (p.isEmpty) return;
    setState(() {
      _loading = true;
      _statusMessage =
          'Analyse du MCP, installation des dépendances et démarrage automatique…';
    });
    try {
      final r = await http.post(
        Uri.parse('${widget.base}/admin/import-auto'),
        headers: widget.headers,
        body: jsonEncode({
          'path': p,
          'workspace': _workspaceCtrl.text.trim(),
          'autoStart': _autoStart,
          'kind': _importKind,
          'remoteUrl': _remoteUrlCtrl.text.trim(),
        }),
      );
      if (r.statusCode == 200) {
        await widget.onImported();
        if (mounted) Navigator.pop(context);
      } else {
        setState(() {
          _statusMessage = 'Erreur (${r.statusCode}) : ${r.body}';
          _loading = false;
        });
      }
    } catch (e) {
      setState(() {
        _statusMessage = 'Erreur : $e';
        _loading = false;
      });
    }
  }

  Future<void> _submitCodeImport() async {
    final code = _codeCtrl.text.trim();
    if (code.isEmpty) return;
    setState(() {
      _loading = true;
      _statusMessage =
          'Création du serveur MCP, génération des dépendances et lancement…';
    });
    try {
      final r = await http.post(
        Uri.parse('${widget.base}/admin/import-code'),
        headers: widget.headers,
        body: jsonEncode({
          'name': _nameCtrl.text.trim().isEmpty
              ? 'Custom MCP'
              : _nameCtrl.text.trim(),
          'code': code,
          'language': _lang,
          'workspace': _workspaceCtrl.text.trim(),
          'autoStart': _autoStart,
        }),
      );
      if (r.statusCode == 200) {
        await widget.onImported();
        if (mounted) Navigator.pop(context);
      } else {
        setState(() {
          _statusMessage = 'Erreur (${r.statusCode}) : ${r.body}';
          _loading = false;
        });
      }
    } catch (e) {
      setState(() {
        _statusMessage = 'Erreur : $e';
        _loading = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      backgroundColor: panel,
      title: const Row(
        children: [
          Icon(Icons.bolt_rounded, color: mint),
          SizedBox(width: 10),
          Text('Import & Lancement Automatique de MCP'),
        ],
      ),
      content: SizedBox(
        width: 660,
        height: 530,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TabBar(
              controller: _tabController,
              indicatorColor: mint,
              labelColor: mint,
              unselectedLabelColor: muted,
              tabs: const [
                Tab(
                  icon: Icon(Icons.folder_zip_outlined),
                  text: 'Dossier, Archive .ZIP ou Fichier MCP',
                ),
                Tab(
                  icon: Icon(Icons.code_rounded),
                  text: 'Coller le code entier d’un MCP',
                ),
              ],
            ),
            const SizedBox(height: 16),
            Expanded(
              child: TabBarView(
                controller: _tabController,
                children: [
                  // Tab 1: Folder / ZIP / Any MCP file
                  SingleChildScrollView(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        const Text(
                          'Sélectionnez n’importe quel dossier MCP (ex: mcp-montage, opus-flutter-mcp), une archive .zip ou un fichier serveur (.js, .py, .ts, package.json, etc.). L’application configure et lance le MCP sans modifier vos fichiers d’origine.',
                          style: TextStyle(color: muted, fontSize: 12.5),
                        ),
                        const SizedBox(height: 12),
                        Row(
                          children: [
                            Expanded(
                              child: TextField(
                                controller: _pathCtrl,
                                decoration: const InputDecoration(
                                  labelText:
                                      'Chemin du dossier, .zip ou fichier MCP',
                                  hintText:
                                      r'C:\Users\...\Mon-MCP ou serveur.js / archive.zip',
                                ),
                              ),
                            ),
                            const SizedBox(width: 8),
                            OutlinedButton.icon(
                              onPressed: _pickFolder,
                              icon: const Icon(Icons.folder_open, size: 17),
                              label: const Text('Dossier'),
                            ),
                            const SizedBox(width: 6),
                            OutlinedButton.icon(
                              onPressed: _pickFile,
                              icon: const Icon(Icons.insert_drive_file_outlined,
                                  size: 17),
                              label: const Text('Fichier / ZIP'),
                            ),
                          ],
                        ),
                        const SizedBox(height: 12),
                        DropdownButtonFormField<String>(
                          initialValue: _importKind,
                          dropdownColor: panel,
                          decoration: const InputDecoration(
                            labelText:
                                'Mode de fonctionnement du MCP (Local ou Serveur HTTP / Distant)',
                          ),
                          items: const [
                            DropdownMenuItem(
                              value: 'auto',
                              child: Text(
                                  'Auto-détection intelligente (Serveur HTTP ou Local STDIO)'),
                            ),
                            DropdownMenuItem(
                              value: 'local_http',
                              child: Text(
                                  'Serveur HTTP (non-STDIO — ex: Express / FastMCP / Uvicorn)'),
                            ),
                            DropdownMenuItem(
                              value: 'stdio',
                              child: Text(
                                  'MCP Local STDIO uniquement (processus standard)'),
                            ),
                            DropdownMenuItem(
                              value: 'remote',
                              child: Text(
                                  'MCP Distant / Externe (URL HTTP externe ou Tunnel)'),
                            ),
                          ],
                          onChanged: (v) =>
                              setState(() => _importKind = v ?? 'auto'),
                        ),
                        if (_importKind == 'remote') ...[
                          const SizedBox(height: 12),
                          TextField(
                            controller: _remoteUrlCtrl,
                            decoration: const InputDecoration(
                              labelText:
                                  'URL distante du MCP (optionnel si détectée automatiquement)',
                              hintText: 'https://mon-mcp.mondomaine.com/mcp',
                            ),
                          ),
                        ],
                        const SizedBox(height: 12),
                        Row(
                          children: [
                            Expanded(
                              child: TextField(
                                controller: _workspaceCtrl,
                                decoration: const InputDecoration(
                                  labelText:
                                      'Dossier PROJET / WORKSPACE cible (optionnel, auto-détecté)',
                                  hintText:
                                      'Dossier de travail passé à PROJET / OPUS_WORKSPACE',
                                ),
                              ),
                            ),
                            const SizedBox(width: 8),
                            OutlinedButton.icon(
                              onPressed: _pickWorkspace,
                              icon: const Icon(Icons.workspaces_outline,
                                  size: 17),
                              label: const Text('Choisir'),
                            ),
                          ],
                        ),
                        const SizedBox(height: 10),
                        SwitchListTile(
                          contentPadding: EdgeInsets.zero,
                          title: const Text(
                              'Installer les dépendances et lancer immédiatement'),
                          subtitle: const Text(
                            'Crée une URL fixe /mcp/<slug> et génère un token unique à chaque redémarrage.',
                            style: TextStyle(color: muted, fontSize: 12),
                          ),
                          value: _autoStart,
                          onChanged: (v) => setState(() => _autoStart = v),
                        ),
                      ],
                    ),
                  ),
                  // Tab 2: Paste full MCP code
                  Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          Expanded(
                            flex: 2,
                            child: TextField(
                              controller: _nameCtrl,
                              decoration: const InputDecoration(
                                labelText: 'Nom du MCP',
                                hintText: 'Ex: Mon MCP Custom',
                              ),
                            ),
                          ),
                          const SizedBox(width: 10),
                          Expanded(
                            child: DropdownButtonFormField<String>(
                              initialValue: _lang,
                              dropdownColor: panel,
                              decoration: const InputDecoration(
                                  labelText: 'Langage'),
                              items: const [
                                DropdownMenuItem(
                                    value: 'auto',
                                    child: Text('Auto-détection')),
                                DropdownMenuItem(
                                    value: 'javascript',
                                    child: Text('Node.js / JS')),
                                DropdownMenuItem(
                                    value: 'typescript',
                                    child: Text('TypeScript')),
                                DropdownMenuItem(
                                    value: 'python', child: Text('Python')),
                              ],
                              onChanged: (v) =>
                                  setState(() => _lang = v ?? 'auto'),
                            ),
                          ),
                        ],
                      ),
                      const SizedBox(height: 10),
                      Expanded(
                        child: TextField(
                          controller: _codeCtrl,
                          maxLines: null,
                          expands: true,
                          textAlignVertical: TextAlignVertical.top,
                          style: const TextStyle(
                              fontFamily: 'Consolas', fontSize: 12),
                          decoration: const InputDecoration(
                            labelText:
                                'Collez le code complet de votre serveur MCP ici',
                            hintText:
                                'import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";\n// ou from mcp.server.fastmcp import FastMCP ...',
                          ),
                        ),
                      ),
                      const SizedBox(height: 8),
                      SwitchListTile(
                        contentPadding: EdgeInsets.zero,
                        title: const Text(
                            'Générer le projet, installer les paquets et démarrer le MCP'),
                        value: _autoStart,
                        onChanged: (v) => setState(() => _autoStart = v),
                      ),
                    ],
                  ),
                ],
              ),
            ),
            if (_statusMessage != null) ...[
              const SizedBox(height: 8),
              Container(
                width: double.infinity,
                padding: const EdgeInsets.all(10),
                decoration: BoxDecoration(
                  color: ink,
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: line),
                ),
                child: Row(
                  children: [
                    if (_loading) ...[
                      const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(
                            strokeWidth: 2, color: mint),
                      ),
                      const SizedBox(width: 10),
                    ],
                    Expanded(
                      child: Text(
                        _statusMessage!,
                        style:
                            const TextStyle(fontSize: 12, color: Colors.white),
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: _loading ? null : () => Navigator.pop(context),
          child: const Text('Annuler'),
        ),
        FilledButton.icon(
          onPressed: _loading
              ? null
              : () {
                  if (_tabController.index == 0) {
                    _submitPathImport();
                  } else {
                    _submitCodeImport();
                  }
                },
          icon: const Icon(Icons.rocket_launch_rounded, size: 17),
          label: Text(_loading
              ? 'Configuration en cours…'
              : 'Ajouter et Lancer le MCP'),
        ),
      ],
    );
  }
}

class ServerDialog extends StatefulWidget {
  final Map<String, dynamic>? initial;
  final String base, token;
  const ServerDialog(
      {super.key, this.initial, required this.base, required this.token});
  @override
  State<ServerDialog> createState() => _ServerDialogState();
}

class _ServerDialogState extends State<ServerDialog> {
  final name = TextEditingController(),
      command = TextEditingController(),
      args = TextEditingController(),
      cwd = TextEditingController(),
      url = TextEditingController(),
      localPort = TextEditingController(),
      localPath = TextEditingController(),
      secretKey = TextEditingController(),
      secretValue = TextEditingController(),
      retryCount = TextEditingController();
  String kind = 'stdio';
  bool autoRestart = true, autoInstall = true, showSecret = false;

  @override
  void initState() {
    super.initState();
    final x = widget.initial;
    if (x != null) {
      name.text = (x['name'] ?? '').toString();
      command.text = (x['command'] ?? '').toString();
      args.text = (x['args'] as List? ?? []).join(' ');
      cwd.text = (x['cwd'] ?? '').toString();
      url.text = (x['url'] ?? '').toString();
      kind = (x['kind'] ?? 'stdio').toString();
      localPort.text = '${x['localPort'] ?? ''}';
      localPath.text = (x['localPath'] ?? '/mcp').toString();
      autoRestart = x['autoRestart'] ?? true;
      autoInstall = x['autoInstall'] ?? true;
      retryCount.text = '${x['maxRetries'] ?? 5}';
      final secrets = Map<String, dynamic>.from(
          x[kind == 'remote' ? 'headers' : 'env'] ?? {});
      if (secrets.isNotEmpty) {
        secretKey.text = secrets.keys.first;
        secretValue.text = '••••••••';
      }
    } else {
      retryCount.text = '5';
      localPath.text = '/mcp';
    }
  }

  Future<void> _reveal() async {
    if (widget.initial == null || secretKey.text.isEmpty) return;
    try {
      final r = await http.post(
        Uri.parse('${widget.base}/admin/secret'),
        headers: {
          'content-type': 'application/json',
          'x-mcp-hub-token': widget.token
        },
        body: jsonEncode({
          'id': widget.initial!['id'],
          'field': kind == 'remote' ? 'headers' : 'env',
          'key': secretKey.text
        }),
      );
      if (r.statusCode == 200 && mounted) {
        secretValue.text = (jsonDecode(r.body)['value'] ?? '').toString();
        setState(() => showSecret = true);
      }
    } catch (_) {}
  }

  @override
  void dispose() {
    name.dispose();
    command.dispose();
    args.dispose();
    cwd.dispose();
    url.dispose();
    localPort.dispose();
    localPath.dispose();
    secretKey.dispose();
    secretValue.dispose();
    retryCount.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext c) => AlertDialog(
        backgroundColor: panel,
        title: Text(widget.initial == null
            ? 'Configurer un serveur MCP'
            : 'Modifier le serveur MCP'),
        content: SizedBox(
          width: 480,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                    controller: name,
                    decoration: const InputDecoration(labelText: 'Nom')),
                const SizedBox(height: 10),
                DropdownButtonFormField<String>(
                  initialValue: kind,
                  dropdownColor: panel,
                  decoration:
                      const InputDecoration(labelText: 'Type de serveur MCP'),
                  items: const [
                    DropdownMenuItem(
                        value: 'stdio',
                        child: Text('Processus local STDIO (Node, Python…)')),
                    DropdownMenuItem(
                        value: 'local_http',
                        child: Text(
                            'Processus local HTTP (Serveur HTTP interne proxifié)')),
                    DropdownMenuItem(
                        value: 'remote',
                        child: Text('Serveur distant HTTP externe')),
                  ],
                  onChanged: (v) => setState(() => kind = v ?? 'stdio'),
                ),
                const SizedBox(height: 10),
                if (kind == 'remote')
                  TextField(
                    controller: url,
                    decoration:
                        const InputDecoration(labelText: 'URL du serveur'),
                  ),
                if (kind != 'remote') ...[
                  TextField(
                    controller: command,
                    decoration: const InputDecoration(
                        labelText: 'Commande (node, python, uv…)'),
                  ),
                  const SizedBox(height: 10),
                  TextField(
                    controller: args,
                    decoration: const InputDecoration(
                        labelText: 'Arguments (séparés par espaces)'),
                  ),
                  const SizedBox(height: 10),
                  TextField(
                    controller: cwd,
                    decoration: const InputDecoration(
                        labelText: 'Dossier de travail (cwd)'),
                  ),
                ],
                if (kind == 'local_http') ...[
                  const SizedBox(height: 10),
                  Row(
                    children: [
                      Expanded(
                        child: TextField(
                          controller: localPort,
                          keyboardType: TextInputType.number,
                          decoration: const InputDecoration(
                              labelText: 'Port HTTP interne'),
                        ),
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        child: TextField(
                          controller: localPath,
                          decoration: const InputDecoration(
                              labelText: 'Chemin HTTP (/mcp)'),
                        ),
                      ),
                    ],
                  ),
                ],
                const SizedBox(height: 12),
                Row(
                  children: [
                    Expanded(
                      child: TextField(
                        controller: secretKey,
                        decoration: InputDecoration(
                          labelText: kind == 'remote'
                              ? 'Header secret'
                              : 'Variable env (ex: PROJET)',
                        ),
                      ),
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: TextField(
                        controller: secretValue,
                        obscureText: !showSecret,
                        decoration: InputDecoration(
                          labelText: 'Valeur',
                          suffixIcon: IconButton(
                            tooltip: showSecret ? 'Masquer' : 'Révéler',
                            onPressed: () =>
                                showSecret ? _mask() : _reveal(),
                            icon: Icon(showSecret
                                ? Icons.visibility_off
                                : Icons.visibility),
                          ),
                        ),
                      ),
                    ),
                  ],
                ),
                if (kind != 'remote') ...[
                  SwitchListTile(
                    contentPadding: EdgeInsets.zero,
                    title: const Text('Auto-installation des dépendances'),
                    value: autoInstall,
                    onChanged: (v) => setState(() => autoInstall = v),
                  ),
                  SwitchListTile(
                    contentPadding: EdgeInsets.zero,
                    title: const Text('Redémarrage automatique'),
                    value: autoRestart,
                    onChanged: (v) => setState(() => autoRestart = v),
                  ),
                  TextField(
                    controller: retryCount,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(
                        labelText: 'Nombre maximum de tentatives'),
                  ),
                ],
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(c),
            child: const Text('Annuler'),
          ),
          FilledButton(
            onPressed: () {
              final n = name.text.trim();
              if (n.isEmpty) return;
              final old = widget.initial ?? {};
              final env = Map<String, dynamic>.from(old['env'] ?? {});
              final headers = Map<String, dynamic>.from(old['headers'] ?? {});
              final secrets = kind == 'remote' ? headers : env;
              if (secretKey.text.trim().isNotEmpty &&
                  secretValue.text != '••••••••') {
                secrets[secretKey.text.trim()] = secretValue.text;
              }
              Navigator.pop(c, <String, dynamic>{
                'id': old['id'] ?? '',
                'name': n,
                'slug': old['slug'] ?? '',
                'kind': kind,
                'command': command.text.trim(),
                'args': args.text.trim().isEmpty
                    ? []
                    : args.text.trim().split(RegExp(r'\s+')),
                'url': kind == 'remote' ? url.text.trim() : null,
                'cwd': cwd.text.trim().isEmpty ? null : cwd.text.trim(),
                'localPort': int.tryParse(localPort.text.trim()),
                'localPath': localPath.text.trim().isEmpty
                    ? null
                    : localPath.text.trim(),
                'env': env,
                'headers': headers,
                'autoStart': old['autoStart'] ?? true,
                'autoRestart': autoRestart,
                'autoInstall': autoInstall,
                'maxRetries': int.tryParse(retryCount.text) ?? 5,
              });
            },
            child: const Text('Enregistrer'),
          ),
        ],
      );

  void _mask() {
    setState(() => showSecret = false);
    secretValue.text = '••••••••';
  }
}
