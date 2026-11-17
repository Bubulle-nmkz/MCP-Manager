#![allow(dead_code, unused_imports)]
use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Path as AxumPath, State,
    },
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    convert::Infallible,
    net::{SocketAddr, TcpListener as StdTcpListener},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{broadcast, oneshot, watch, Mutex},
};
use tokio_stream::StreamExt as _;
use tower_http::cors::CorsLayer;
use walkdir::WalkDir;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Server {
    id: String,
    name: String,
    slug: String,
    kind: String, // "stdio" | "http_managed" | "remote"
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default = "yes")]
    auto_start: bool,
    #[serde(default = "yes")]
    auto_restart: bool,
    #[serde(default = "tries")]
    max_retries: u32,
    #[serde(default)]
    local_port: Option<u16>,
    #[serde(default)]
    mcp_path: Option<String>,
    #[serde(default)]
    token_env_key: Option<String>,
    #[serde(default)]
    workspace_env_key: Option<String>,
    #[serde(default)]
    connection_token: Option<String>,
}

fn yes() -> bool {
    true
}
fn tries() -> u32 {
    5
}

impl Default for Server {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            slug: String::new(),
            kind: "stdio".into(),
            command: String::new(),
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            auto_start: true,
            auto_restart: true,
            max_retries: 5,
            local_port: None,
            mcp_path: None,
            token_env_key: None,
            workspace_env_key: None,
            connection_token: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Config {
    port: u16,
    servers: Vec<Server>,
    start_with_windows: bool,
    auto_start_all_mcps: bool,
    public_base_url: String,
    enable_tunnel: bool,
    tunnel_provider: String, // "tailscale" | "ngrok" | "cloudflare" | "none"
    ngrok_auth_token: String,
    ngrok_domain: String,
    cloudflare_tunnel_token: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 7300,
            servers: vec![],
            start_with_windows: false,
            auto_start_all_mcps: false,
            public_base_url: String::new(),
            enable_tunnel: true,
            tunnel_provider: "tailscale".into(),
            ngrok_auth_token: String::new(),
            ngrok_domain: String::new(),
            cloudflare_tunnel_token: String::new(),
        }
    }
}

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct Runtime {
    id: String,
    running: bool,
    restarting: bool,
    installing: bool,
    restart_attempt: u32,
    cpu: f32,
    memory_mb: u64,
    active_token: String,
    #[serde(skip)]
    process_token: String,
    local_endpoint: Option<String>,
    public_url: Option<String>,
    #[serde(skip)]
    logs: VecDeque<String>,
}

struct IoState {
    input: Mutex<tokio::process::ChildStdin>,
    pending: Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>,
    events: broadcast::Sender<String>,
}

struct App {
    config: Mutex<Config>,
    runtime: Mutex<HashMap<String, Runtime>>,
    io: Mutex<HashMap<String, Arc<IoState>>>,
    supervisors: Mutex<HashMap<String, watch::Sender<bool>>>,
    pids: Mutex<HashMap<String, u32>>,
    token: String,
    file: PathBuf,
    client: reqwest::Client,
    sessions: Mutex<HashMap<String, String>>,
    tunnel_url: Mutex<Option<String>>,
}

type S = Arc<App>;

fn config_path() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config")
        })
        .join("McpHub")
}

fn managed_servers_dir() -> PathBuf {
    config_path().join("servers")
}

fn default_workspace_dir() -> String {
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let docs = PathBuf::from(&profile).join("Documents");
        if docs.is_dir() {
            return docs.to_string_lossy().to_string();
        }
        return profile;
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .to_string_lossy()
        .to_string()
}

fn slug(s: &str) -> String {
    let out = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if out.is_empty() {
        "mcp-server".into()
    } else {
        out
    }
}

fn generate_mcp_token() -> String {
    // 32 caractères hexadécimaux aléatoires (compatible MCP_TOKEN >= 20 chars et OPUS_SECRET [A-Za-z0-9_-]{16,128})
    hex::encode(rand::random::<[u8; 16]>())
}

fn pick_free_port(preferred: u16) -> u16 {
    if preferred > 0 {
        if StdTcpListener::bind(("127.0.0.1", preferred)).is_ok() {
            return preferred;
        }
    }
    StdTcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(8790)
}

fn append_log(r: &mut Runtime, s: String) {
    if r.logs.len() >= 500 {
        r.logs.pop_front();
    }
    r.logs.push_back(s);
}

async fn log_line(app: &S, id: &str, msg: impl Into<String>) {
    let text = msg.into();
    let mut r = app.runtime.lock().await;
    let q = r.entry(id.to_string()).or_insert_with(|| Runtime {
        id: id.to_string(),
        ..Default::default()
    });
    append_log(q, text);
}

async fn auth(h: &HeaderMap, s: &S) -> Result<(), StatusCode> {
    if h.get("x-mcp-hub-token").and_then(|v| v.to_str().ok()) == Some(s.token.as_str()) {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn extract_bearer_or_token(h: &HeaderMap) -> Option<String> {
    if let Some(auth_val) = h.get("authorization").and_then(|v| v.to_str().ok()) {
        let trimmed = auth_val.trim();
        if let Some(rest) = trimmed.strip_prefix("Bearer ").or_else(|| trimmed.strip_prefix("bearer ")) {
            if !rest.trim().is_empty() {
                return Some(rest.trim().to_string());
            }
        }
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(tok) = h.get("x-mcp-token").and_then(|v| v.to_str().ok()) {
        if !tok.trim().is_empty() {
            return Some(tok.trim().to_string());
        }
    }
    None
}

async fn verify_server_token(app: &S, server_id: &str, h: &HeaderMap) -> Result<String, StatusCode> {
    let active = {
        let mut r = app.runtime.lock().await;
        let q = r.entry(server_id.to_string()).or_insert_with(|| Runtime {
            id: server_id.to_string(),
            active_token: generate_mcp_token(),
            ..Default::default()
        });
        if q.active_token.is_empty() {
            q.active_token = generate_mcp_token();
        }
        q.active_token.clone()
    };
    if let Some(provided) = extract_bearer_or_token(h) {
        if provided == active {
            return Ok(active);
        }
    }
    Err(StatusCode::UNAUTHORIZED)
}

#[cfg(windows)]
fn protect(plain: &str) -> Result<String, String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let mut input_bytes = plain.as_bytes().to_vec();
    let input = CRYPT_INTEGER_BLOB {
        cbData: input_bytes.len() as u32,
        pbData: input_bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(&input, PCWSTR::null(), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        .map_err(|e| e.to_string())?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let encoded = B64.encode(bytes);
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(output.pbData as _));
        Ok(format!("dpapi:{encoded}"))
    }
}
#[cfg(not(windows))]
fn protect(_: &str) -> Result<String, String> {
    Err("DPAPI est disponible uniquement sous Windows".into())
}

#[cfg(windows)]
fn unprotect(value: &str) -> Result<String, String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let encoded = value.strip_prefix("dpapi:").ok_or("préfixe DPAPI absent")?;
    let mut bytes = B64.decode(encoded).map_err(|e| e.to_string())?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        .map_err(|e| e.to_string())?;
        let clear = String::from_utf8_lossy(std::slice::from_raw_parts(
            output.pbData,
            output.cbData as usize,
        ))
        .into_owned();
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(output.pbData as _));
        Ok(clear)
    }
}
#[cfg(not(windows))]
fn unprotect(_: &str) -> Result<String, String> {
    Err("DPAPI est disponible uniquement sous Windows".into())
}

fn persisted(c: &Config) -> Result<Config, String> {
    let mut out = c.clone();
    for server in &mut out.servers {
        for v in server.env.values_mut().chain(server.headers.values_mut()) {
            if v.is_empty() || v.starts_with("dpapi:") {
                continue;
            }
            *v = protect(v)?;
        }
    }
    Ok(out)
}

fn loaded(mut c: Config) -> (Config, bool) {
    let mut changed = false;
    for server in &mut c.servers {
        for v in server.env.values_mut().chain(server.headers.values_mut()) {
            if let Some(old) = v.strip_prefix("plain:") {
                *v = old.to_string();
                changed = true;
            }
            if v.starts_with("dpapi:") {
                if let Ok(clear) = unprotect(v) {
                    *v = clear;
                }
            } else if !v.is_empty() {
                changed = true;
            }
        }
    }
    (c, changed)
}

async fn save(s: &S, c: &Config) -> Result<(), String> {
    let enc = persisted(c)?;
    let bytes = serde_json::to_vec_pretty(&enc).map_err(|e| e.to_string())?;
    tokio::fs::write(&s.file, bytes)
        .await
        .map_err(|e| e.to_string())
}

fn public_server(
    x: &Server,
    r: Option<&Runtime>,
    hub_port: u16,
    public_base: &str,
) -> serde_json::Value {
    let masked = x
        .env
        .iter()
        .map(|(k, v)| {
            let is_path_key = k == "PROJET"
                || k == "OPUS_WORKSPACE"
                || k == "WORKSPACE"
                || x.workspace_env_key.as_deref() == Some(k.as_str());
            (
                k.clone(),
                if v.is_empty() {
                    String::new()
                } else if is_path_key {
                    v.clone()
                } else {
                    "••••••••".into()
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let headers = x
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                if v.is_empty() {
                    String::new()
                } else {
                    "••••••••".into()
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let active_tok = r.map(|v| v.active_token.clone()).unwrap_or_default();
    let local_url = format!("http://127.0.0.1:{hub_port}/mcp/{}", x.slug);
    let clean_base = public_base.trim().trim_end_matches('/');
    // Si on a une base publique (tunnel Cloudflare, ngrok fixe, ou domaine perso), on construit l'URL publique :
    // - fixedUrl : https://<tunnel>/mcp/<slug> (avec header Authorization: Bearer <token>)
    // - mammouthUrl : https://<tunnel>/mcp/<slug>/<token>/mcp (secret intégré dans l'URL comme exigé par Mammouth AI !)
    let has_public = !clean_base.is_empty() && !clean_base.contains("127.0.0.1") && !clean_base.contains("localhost");
    let effective_base = if has_public {
        clean_base.to_string()
    } else if let Some(u) = &x.url {
        if !u.trim().is_empty() && !u.contains("127.0.0.1") && !u.contains("localhost") {
            u.trim().trim_end_matches('/').to_string()
        } else {
            format!("http://127.0.0.1:{hub_port}")
        }
    } else {
        format!("http://127.0.0.1:{hub_port}")
    };

    // L'URL est 100 % FIXE et NE CONTIENT JAMAIS LE TOKEN (aucun token dans l'URL pour Mammouth AI ni Claude) !
    // Le token est transmis séparément via l'en-tête standard "Authorization: Bearer <TOKEN>"
    let fixed_url = format!("{effective_base}/mcp/{}", x.slug);
    let mammouth_url = format!("{effective_base}/mcp/{}/mcp", x.slug);

    let workspace_val = x
        .workspace_env_key
        .as_ref()
        .and_then(|k| x.env.get(k))
        .cloned()
        .or_else(|| x.env.get("PROJET").cloned())
        .or_else(|| x.env.get("OPUS_WORKSPACE").cloned());

    serde_json::json!({
        "id": x.id,
        "name": x.name,
        "slug": x.slug,
        "kind": x.kind,
        "command": x.command,
        "args": x.args,
        "cwd": x.cwd,
        "env": masked,
        "url": x.url,
        "headers": headers,
        "autoStart": x.auto_start,
        "autoRestart": x.auto_restart,
        "maxRetries": x.max_retries,
        "localPort": x.local_port,
        "mcpPath": x.mcp_path,
        "tokenEnvKey": x.token_env_key,
        "workspaceEnvKey": x.workspace_env_key,
        "workspacePath": workspace_val,
        "fixedUrl": fixed_url,
        "localUrl": local_url,
        "mammouthUrl": mammouth_url,
        "publicUrl": if has_public { Some(mammouth_url.clone()) } else { Some(fixed_url.clone()) },
        "connectionToken": active_tok,
        "activeToken": active_tok,
        "localEndpoint": r.and_then(|v| v.local_endpoint.clone()),
        "installing": r.map(|v| v.installing).unwrap_or(false),
        "restarting": r.map(|v| v.restarting).unwrap_or(false),
        "restartAttempt": r.map(|v| v.restart_attempt).unwrap_or(0),
        "running": r.map(|v| v.running).unwrap_or(false),
        "cpu": r.map(|v| v.cpu).unwrap_or(0.),
        "memoryMb": r.map(|v| v.memory_mb).unwrap_or(0),
    })
}
async fn supervise(app: S, id: String, mut stop_rx: watch::Receiver<bool>) {
    let mut attempt = 0u32;
    loop {
        if *stop_rx.borrow() {
            break;
        }
        let cfg = app.config.lock().await.clone();
        let Some(x) = cfg.servers.iter().find(|x| x.id == id).cloned() else {
            break;
        };
        if x.kind == "remote" && x.command.trim().is_empty() {
            break;
        }

        // Conserver le token existant/persistant du serveur (sans l'écraser au redémarrage du PC ou du MCP)
        let (server_token, proc_token) = {
            let mut r = app.runtime.lock().await;
            let q = r.entry(id.clone()).or_insert_with(|| Runtime {
                id: id.clone(),
                ..Default::default()
            });
            let tok = if !q.active_token.is_empty() {
                q.active_token.clone()
            } else if let Some(ct) = x.connection_token.clone().filter(|s| !s.trim().is_empty()) {
                ct
            } else if let Some(et) = x
                .env
                .get("MCP_TOKEN")
                .or_else(|| x.env.get("OPUS_BEARER"))
                .or_else(|| x.env.get("OPUS_SECRET"))
                .cloned()
                .filter(|s| !s.trim().is_empty() && s != "••••••••")
            {
                et
            } else {
                generate_mcp_token()
            };
            q.active_token = tok.clone();
            q.process_token = tok.clone();
            append_log(
                q,
                format!(
                    "🔑 Token de session actif prêt (URL fixe /mcp/{})",
                    x.slug
                ),
            );
            (tok.clone(), tok)
        };

        // Sauvegarder le token dans la config s'il n'était pas encore persisté
        if x.connection_token.as_deref() != Some(server_token.as_str()) {
            let mut cfg_guard = app.config.lock().await;
            if let Some(srv) = cfg_guard.servers.iter_mut().find(|s| s.id == id) {
                srv.connection_token = Some(server_token.clone());
                srv.env.insert("MCP_TOKEN".into(), server_token.clone());
                srv.env.insert("OPUS_BEARER".into(), server_token.clone());
                srv.env.insert("OPUS_SECRET".into(), server_token.clone());
            }
            let _ = save(&app, &cfg_guard).await;
        }

        // Auto-installation des dépendances si nécessaire
        ensure_dependencies(&app, &id, &x).await;

        // Si le serveur utilise un port HTTP local (ex: 3333 ou 8765) et qu'un processus zombie l'occupe après un redémarrage, on le libère
        if let Some(lp) = x.local_port {
            free_local_port(lp).await;
        }

        let mut env_vars = x.env.clone();
        env_vars.insert("MCP_TOKEN".into(), proc_token.clone());
        env_vars.insert("OPUS_BEARER".into(), proc_token.clone());
        env_vars.insert("OPUS_SECRET".into(), proc_token.clone());
        if let Some(ref key) = x.token_env_key {
            env_vars.insert(key.clone(), proc_token.clone());
        }
        if let Some(p) = x.local_port {
            env_vars
                .entry("PORT".into())
                .or_insert_with(|| p.to_string());
            env_vars
                .entry("OPUS_PORT".into())
                .or_insert_with(|| p.to_string());
        }
        // S'assurer que l'exécutable Python utilise bien le .venv fraîchement créé s'il existe
        let mut actual_command = x.command.clone();
        if let Some(cwd) = &x.cwd {
            let venv_py = Path::new(cwd).join(".venv").join("Scripts").join("python.exe");
            if (actual_command == "python" || actual_command == "python.exe") && venv_py.exists() {
                actual_command = venv_py.to_string_lossy().to_string();
            }
            // S'assurer que PROJET / OPUS_WORKSPACE ont une valeur valide et existante sur ce PC
            let fallback_ws = Path::new(cwd)
                .parent()
                .unwrap_or_else(|| Path::new(cwd))
                .to_string_lossy()
                .to_string();
            let proj_invalid = env_vars
                .get("PROJET")
                .map(|s| s.trim().is_empty() || !Path::new(s.trim()).exists())
                .unwrap_or(false);
            if proj_invalid {
                env_vars.insert("PROJET".into(), fallback_ws.clone());
            }
            let opus_invalid = env_vars
                .get("OPUS_WORKSPACE")
                .map(|s| s.trim().is_empty() || !Path::new(s.trim()).exists())
                .unwrap_or(true);
            if opus_invalid {
                env_vars.insert("OPUS_WORKSPACE".into(), fallback_ws);
            }
        }

        let mut cmd = Command::new(&actual_command);
        cmd.args(&x.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(cwd) = &x.cwd {
            if !cwd.is_empty() {
                cmd.current_dir(cwd);
            }
        }
        cmd.envs(env_vars);

        let spawn = cmd.spawn();
        let mut child = match spawn {
            Ok(ch) => ch,
            Err(e) => {
                let unexpected = !*stop_rx.borrow();
                let retry = unexpected && x.auto_restart && attempt < x.max_retries;
                let mut r = app.runtime.lock().await;
                let q = r.entry(id.clone()).or_insert_with(|| Runtime {
                    id: id.clone(),
                    ..Default::default()
                });
                q.running = false;
                append_log(q, format!("Échec de lancement ({}) : {e}", x.command));
                if !retry {
                    q.restarting = false;
                    q.restart_attempt = attempt;
                    drop(r);
                    app.supervisors.lock().await.remove(&id);
                    return;
                }
                attempt += 1;
                let backoff = 2u64.pow((attempt - 1).min(5)).min(30);
                q.restarting = true;
                q.restart_attempt = attempt;
                append_log(
                    q,
                    format!(
                        "Échec de lancement; redémarrage {attempt}/{} dans {backoff}s",
                        x.max_retries
                    ),
                );
                drop(r);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(backoff)) => {},
                    changed = stop_rx.changed() => {
                        if changed.is_err() || *stop_rx.borrow() {
                            if let Some(q) = app.runtime.lock().await.get_mut(&id) {
                                q.restarting = false;
                            }
                            app.supervisors.lock().await.remove(&id);
                            return;
                        }
                    }
                }
                continue;
            }
        };

        let pid = child.id().unwrap_or(0);
        let input = child.stdin.take().expect("stdin piped");
        let output = child.stdout.take().expect("stdout piped");
        let err_output = child.stderr.take().expect("stderr piped");
        let (events_tx, _) = broadcast::channel(128);
        let io = Arc::new(IoState {
            input: Mutex::new(input),
            pending: Mutex::new(HashMap::new()),
            events: events_tx,
        });
        app.io.lock().await.insert(id.clone(), io.clone());
        app.pids.lock().await.insert(id.clone(), pid);
        {
            let mut r = app.runtime.lock().await;
            let q = r.entry(id.clone()).or_insert_with(|| Runtime {
                id: id.clone(),
                ..Default::default()
            });
            q.running = true;
            q.restarting = false;
            q.restart_attempt = attempt;
            append_log(
                q,
                format!("Processus démarré (PID {pid}) — {} {}", x.command, x.args.join(" ")),
            );
        }
        tokio::spawn(stdout_reader(id.clone(), output, io, app.clone()));
        tokio::spawn(stderr_reader(id.clone(), err_output, app.clone()));

        let started = tokio::time::Instant::now();
        let mut stable = false;
        let status = loop {
            tokio::select! {
                res = child.wait() => break Some(res),
                _ = tokio::time::sleep_until(started + Duration::from_secs(60)), if !stable => {
                    stable = true;
                    attempt = 0;
                    let mut r = app.runtime.lock().await;
                    if let Some(q) = r.get_mut(&id) {
                        q.restart_attempt = 0;
                    }
                },
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        break None;
                    }
                }
            }
        };

        if status.is_none() {
            if pid > 0 {
                #[cfg(windows)]
                {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/F", "/T", "/PID", &pid.to_string()])
                        .output();
                }
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
            app.io.lock().await.remove(&id);
            app.pids.lock().await.remove(&id);
            if let Some(q) = app.runtime.lock().await.get_mut(&id) {
                q.running = false;
                q.restarting = false;
                append_log(q, "Arrêt demandé".into());
            }
            app.supervisors.lock().await.remove(&id);
            return;
        }

        let status = status.unwrap();
        if let Some(io) = app.io.lock().await.remove(&id) {
            io.pending.lock().await.clear();
        }
        app.pids.lock().await.remove(&id);
        if let Some(q) = app.runtime.lock().await.get_mut(&id) {
            q.running = false;
        }
        let unexpected = !*stop_rx.borrow();
        let Ok(status) = status else {
            break;
        };
        if !unexpected || !x.auto_restart || attempt >= x.max_retries {
            let mut r = app.runtime.lock().await;
            if let Some(q) = r.get_mut(&id) {
                q.restarting = false;
                q.restart_attempt = attempt;
                append_log(
                    q,
                    format!("Processus terminé ({status}); redémarrage désactivé ou limite atteinte"),
                );
            }
            break;
        }
        attempt += 1;
        let backoff = 2u64.pow((attempt - 1).min(5)).min(30);
        {
            let mut r = app.runtime.lock().await;
            if let Some(q) = r.get_mut(&id) {
                q.restarting = true;
                q.restart_attempt = attempt;
                append_log(
                    q,
                    format!(
                        "Sortie inattendue; redémarrage {attempt}/{} dans {backoff}s",
                        x.max_retries
                    ),
                );
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {},
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    break;
                }
            }
        }
    }
    if let Some(q) = app.runtime.lock().await.get_mut(&id) {
        q.running = false;
        q.restarting = false;
    }
    app.supervisors.lock().await.remove(&id);
}

async fn start(app: &S, id: &str) -> Result<(), String> {
    let cfg = app.config.lock().await.clone();
    let x = cfg
        .servers
        .iter()
        .find(|x| x.id == id)
        .ok_or("Serveur inconnu")?;
    if x.kind == "remote" && x.command.trim().is_empty() {
        let mut r = app.runtime.lock().await;
        let q = r.entry(id.into()).or_insert_with(|| Runtime {
            id: id.into(),
            ..Default::default()
        });
        q.running = true;
        if q.active_token.is_empty() {
            q.active_token = x
                .connection_token
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(generate_mcp_token);
        }
        append_log(
            q,
            format!("🔑 Serveur distant activé (/mcp/{})", x.slug),
        );
        return Ok(());
    }
    let mut supervisors = app.supervisors.lock().await;
    if supervisors.contains_key(id) {
        return Ok(());
    }
    let (tx, rx) = watch::channel(false);
    supervisors.insert(id.into(), tx);
    let app2 = app.clone();
    let id2 = id.to_string();
    tokio::spawn(async move {
        supervise(app2, id2, rx).await;
    });
    Ok(())
}

async fn stop(app: &S, id: &str) {
    if let Some(tx) = app.supervisors.lock().await.get(id) {
        let _ = tx.send(true);
    }
    if let Some(pid) = app.pids.lock().await.get(id).copied() {
        if pid > 0 {
            #[cfg(windows)]
            {
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .output();
            }
        }
    }
    if let Some(q) = app.runtime.lock().await.get_mut(id) {
        q.running = false;
        q.restarting = false;
        append_log(q, "Arrêt demandé".into());
    }
    for _ in 0..100 {
        if !app.supervisors.lock().await.contains_key(id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn admin_shutdown(State(s): State<S>, h: HeaderMap) -> Result<StatusCode, StatusCode> {
    auth(&h, &s).await?;
    let ids: Vec<String> = s
        .config
        .lock()
        .await
        .servers
        .iter()
        .map(|srv| srv.id.clone())
        .collect();
    for id in ids {
        stop(&s, &id).await;
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", "cloudflared.exe"])
            .output();
    }
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        std::process::exit(0);
    });
    Ok(StatusCode::NO_CONTENT)
}

async fn restart(app: &S, id: &str) -> Result<(), String> {
    stop(app, id).await;
    start(app, id).await
}

async fn power(
    State(s): State<S>,
    h: HeaderMap,
    AxumPath((id, a)): AxumPath<(String, String)>,
) -> Result<StatusCode, Response> {
    auth(&h, &s).await.map_err(IntoResponse::into_response)?;
    let r = match a.as_str() {
        "start" => start(&s, &id).await,
        "restart" => restart(&s, &id).await,
        _ => {
            stop(&s, &id).await;
            Ok(())
        }
    };
    r.map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| (StatusCode::BAD_REQUEST, e).into_response())
}

async fn del(
    State(s): State<S>,
    h: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, StatusCode> {
    auth(&h, &s).await?;
    stop(&s, &id).await;
    let mut c = s.config.lock().await;
    c.servers.retain(|x| x.id != id);
    save(&s, &c)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn logs(
    State(s): State<S>,
    h: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Vec<String>>, StatusCode> {
    auth(&h, &s).await?;
    Ok(Json(
        s.runtime
            .lock()
            .await
            .get(&id)
            .map(|r| r.logs.iter().cloned().collect())
            .unwrap_or_default(),
    ))
}

async fn secret(
    State(s): State<S>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    h: HeaderMap,
    Json(v): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    auth(&h, &s).await?;
    if !peer.ip().is_loopback() {
        return Err(StatusCode::FORBIDDEN);
    }
    let id = v["id"].as_str().ok_or(StatusCode::BAD_REQUEST)?;
    let field = v["field"].as_str().unwrap_or("activeToken");
    if field == "activeToken" {
        let mut r = s.runtime.lock().await;
        let q = r.entry(id.to_string()).or_insert_with(|| Runtime {
            id: id.to_string(),
            active_token: generate_mcp_token(),
            ..Default::default()
        });
        if q.active_token.is_empty() {
            q.active_token = generate_mcp_token();
        }
        return Ok(Json(serde_json::json!({ "value": q.active_token })));
    }
    let key = v["key"].as_str().ok_or(StatusCode::BAD_REQUEST)?;
    if field != "env" && field != "headers" {
        return Err(StatusCode::BAD_REQUEST);
    }
    let c = s.config.lock().await;
    let x = c
        .servers
        .iter()
        .find(|x| x.id == id)
        .ok_or(StatusCode::NOT_FOUND)?;
    let map = if field == "env" { &x.env } else { &x.headers };
    let value = map.get(key).ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(serde_json::json!({ "value": value })))
}
async fn settings(
    State(s): State<S>,
    h: HeaderMap,
    Json(v): Json<serde_json::Value>,
) -> Result<StatusCode, Response> {
    auth(&h, &s).await.map_err(IntoResponse::into_response)?;
    let mut c = s.config.lock().await;
    if let Some(p) = v["port"].as_u64().filter(|p| *p > 0 && *p < 65536) {
        c.port = p as u16;
    }
    if let Some(pub_url) = v["publicBaseUrl"].as_str() {
        let trimmed = pub_url.trim().trim_end_matches('/').to_string();
        if !trimmed.contains("127.0.0.1") && !trimmed.contains("localhost") {
            c.public_base_url = trimmed;
        } else if trimmed.is_empty() {
            c.public_base_url.clear();
        }
    }
    if let Some(en) = v["enableTunnel"].as_bool() {
        c.enable_tunnel = en;
    }
    if let Some(tp) = v["tunnelProvider"].as_str() {
        c.tunnel_provider = tp.trim().to_string();
    }
    if let Some(nd) = v["ngrokDomain"].as_str() {
        let clean = nd
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        c.ngrok_domain = clean.clone();
        if !clean.is_empty() && c.tunnel_provider == "ngrok" {
            c.public_base_url = format!("https://{clean}");
        }
    }
    if let Some(nt) = v["ngrokAuthToken"].as_str() {
        let trimmed = nt.trim();
        if !trimmed.is_empty() && trimmed != "••••••••" {
            c.ngrok_auth_token = trimmed.to_string();
        }
    }
    if let Some(ct) = v["cloudflareTunnelToken"].as_str() {
        let trimmed = ct.trim();
        if !trimmed.is_empty() && trimmed != "••••••••" {
            c.cloudflare_tunnel_token = trimmed.to_string();
        }
    }
    if let Some(b) = v["startWithWindows"].as_bool() {
        if b != c.start_with_windows {
            let app_path = v["appPath"].as_str().unwrap_or("");
            #[cfg(windows)]
            set_startup(b, app_path)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e).into_response())?;
            #[cfg(not(windows))]
            if b {
                return Err(
                    (StatusCode::INTERNAL_SERVER_ERROR, "Windows uniquement".to_string())
                        .into_response(),
                );
            }
            c.start_with_windows = b;
        }
    }
    save(&s, &c)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e).into_response())?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(windows)]
fn set_startup(enabled: bool, app_path: &str) -> Result<(), String> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            winreg::enums::KEY_WRITE,
        )
        .or_else(|_| {
            hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
                .map(|x| x.0)
        })
        .map_err(|e| e.to_string())?;
    let name = "MCP Manager";
    if enabled {
        let exe = if app_path.is_empty() {
            std::env::current_exe()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .to_string()
        } else {
            app_path.to_string()
        };
        key.set_value(name, &format!("\"{}\" --minimized", exe))
            .map_err(|e| e.to_string())?;
    } else {
        let _ = key.delete_value(name);
    }
    Ok(())
}

#[cfg(not(windows))]
fn set_startup(_: bool, _: &str) -> Result<(), String> {
    Err("Le démarrage Windows n’est disponible que sous Windows".into())
}

fn parse_env_file(path: &Path) -> HashMap<String, String> {
    let mut env = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text
            .lines()
            .map(str::trim)
            .filter(|x| !x.is_empty() && !x.starts_with('#'))
        {
            if let Some((key, val)) = line.split_once('=') {
                let k = key.trim();
                let v = val.trim().trim_matches('"').trim_matches('\'');
                if !k.is_empty() {
                    env.insert(k.to_string(), v.to_string());
                }
            }
        }
    }
    env
}

fn example_env(dir: &Path) -> HashMap<String, String> {
    let mut env = parse_env_file(&dir.join(".env.example"));
    for (k, v) in parse_env_file(&dir.join(".env")) {
        if !v.is_empty() {
            env.insert(k, v);
        } else {
            env.entry(k).or_insert_with(String::new);
        }
    }
    env
}

// Analyse statique d'un fichier ou dossier pour détecter le port HTTP, le chemin MCP et les variables d'env
fn analyze_source_text(text: &str, default_workspace: &str) -> (Option<u16>, Option<String>, HashMap<String, String>) {
    let mut env = HashMap::new();
    let mut local_port: Option<u16> = None;
    let mut local_path: Option<String> = None;

    // Détection de process.env.XYZ ou os.environ.get("XYZ")
    if text.contains("process.env.PROJET") {
        env.insert("PROJET".into(), default_workspace.to_string());
    }
    if text.contains("OPUS_WORKSPACE") {
        env.insert("OPUS_WORKSPACE".into(), default_workspace.to_string());
    }

    // Détection d'un serveur HTTP MCP (ex: Express app.listen(3333) ou uvicorn port 8765)
    if text.contains("StreamableHTTPServerTransport")
        || text.contains("app.listen(")
        || text.contains("uvicorn")
        || text.contains("FastMCP")
    {
        // Chercher app.listen(3333 ou port: int = 8765
        for line in text.lines() {
            if local_port.is_none() {
                if let Some(idx) = line.find("app.listen(") {
                    let rest = &line[idx + 11..];
                    let digits: String = rest
                        .chars()
                        .skip_while(|c| c.is_whitespace())
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    if let Ok(p) = digits.parse::<u16>() {
                        local_port = Some(p);
                    }
                } else if line.contains("port") && line.contains("8765") {
                    local_port = Some(8765);
                } else if line.contains("PORT") && line.contains("3333") {
                    local_port = Some(3333);
                }
            }
            if local_path.is_none() {
                if line.contains("\"/mcp\"") || line.contains("'/mcp'") {
                    local_path = Some("/mcp".into());
                }
            }
        }
    }
    (local_port, local_path, env)
}

fn inspect_directory(dir: &Path) -> Option<Server> {
    let default_ws = dir
        .parent()
        .unwrap_or(dir)
        .to_string_lossy()
        .to_string();

    // 1. Cas Node.js (package.json ou server.js / index.js)
    let pkg_path = dir.join("package.json");
    if pkg_path.exists() || dir.join("server.js").exists() || dir.join("index.js").exists() {
        let mut name = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let mut candidate_entry: Option<String> = None;

        if let Ok(raw) = std::fs::read_to_string(&pkg_path) {
            if let Ok(j) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(n) = j["name"].as_str() {
                    if !n.is_empty() {
                        name = n.to_string();
                    }
                }
                if let Some(b) = j["bin"].as_str() {
                    if dir.join(b).exists() {
                        candidate_entry = Some(b.to_string());
                    }
                }
                if candidate_entry.is_none() {
                    if let Some(m) = j["main"].as_str() {
                        if dir.join(m).exists() {
                            candidate_entry = Some(m.to_string());
                        }
                    }
                }
            }
        }

        // Fallback intelligent si "main": "index.js" n'existe pas (ex: mcp-montage a server.js !)
        if candidate_entry.is_none() {
            for fallback in [
                "server.js",
                "index.js",
                "main.js",
                "server.mjs",
                "index.mjs",
                "dist/index.js",
                "build/index.js",
                "src/index.js",
            ] {
                if dir.join(fallback).exists() {
                    candidate_entry = Some(fallback.to_string());
                    break;
                }
            }
        }

        if let Some(entry) = candidate_entry {
            let code = std::fs::read_to_string(dir.join(&entry)).unwrap_or_default();
            let (local_port, mut local_path, mut env) =
                analyze_source_text(&code, &default_ws);
            for (k, v) in example_env(dir) {
                env.entry(k).or_insert(v);
            }
            if local_port.is_some() && local_path.is_none() {
                local_path = Some("/mcp".into());
            }
            let detected_kind = if local_port.is_some() {
                "local_http"
            } else {
                "stdio"
            };
            return Some(Server {
                id: String::new(),
                name: name.clone(),
                slug: slug(&name),
                kind: detected_kind.into(),
                command: "node".into(),
                args: vec![entry],
                cwd: Some(dir.to_string_lossy().into()),
                env,
                local_port,
                mcp_path: local_path,
                auto_start: true,
                auto_restart: true,
                max_retries: 5,
                ..Default::default()
            });
        }
    }

    // 2. Cas Python (package avec __main__.py comme opus-flutter-mcp, ou server.py / main.py)
    // Attention : si `dir` est lui-même le sous-dossier package Python (contenant __init__.py et __main__.py)
    // d'un projet parent qui contient requirements.txt / start.bat, on ignore ce sous-dossier pour éviter les doublons !
    if dir.join("__init__.py").exists() && dir.join("__main__.py").exists() {
        if let Some(parent) = dir.parent() {
            if parent.join("requirements.txt").exists()
                || parent.join("pyproject.toml").exists()
                || parent.join("start.bat").exists()
            {
                return None;
            }
        }
    }

    let mut py_module: Option<String> = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join("__main__.py").exists() {
                if let Some(mod_name) = p.file_name().and_then(|x| x.to_str()) {
                    if !mod_name.starts_with('.') && mod_name != "tests" && mod_name != "data" {
                        py_module = Some(mod_name.to_string());
                        break;
                    }
                }
            }
        }
    }

    let venv_py = dir.join(".venv").join("Scripts").join("python.exe");
    let py_cmd = if venv_py.exists() {
        venv_py.to_string_lossy().to_string()
    } else {
        "python".to_string()
    };

    if let Some(module) = py_module {
        let n = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let mut env = example_env(dir);
        let main_code = std::fs::read_to_string(dir.join(&module).join("__main__.py"))
            .unwrap_or_default();
        let cfg_code = std::fs::read_to_string(dir.join(&module).join("config.py"))
            .unwrap_or_default();
        let combined = format!("{main_code}\n{cfg_code}");
        let (mut local_port, mut local_path, extra_env) =
            analyze_source_text(&combined, &default_ws);
        for (k, v) in extra_env {
            env.entry(k).or_insert(v);
        }
        // S'assurer que OPUS_WORKSPACE pointe toujours vers un dossier existant (sinon opus-flutter-mcp refuse de démarrer)
        let ws_ok = env
            .get("OPUS_WORKSPACE")
            .map(|w| !w.trim().is_empty() && Path::new(w.trim()).exists())
            .unwrap_or(false);
        if !ws_ok {
            env.insert("OPUS_WORKSPACE".into(), default_ws.clone());
        }
        // Cas opus-flutter-mcp : utilise secret.txt ou OPUS_SECRET pour /{secret}/mcp sur port 8765
        if combined.contains("OPUS_SECRET") || combined.contains("secret.txt") {
            let secret_val = env
                .get("OPUS_SECRET")
                .cloned()
                .filter(|s| s.len() >= 16)
                .or_else(|| {
                    std::fs::read_to_string(dir.join("data").join("secret.txt"))
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| s.len() >= 16)
                })
                .unwrap_or_else(|| "opus_hub_fixed_secret_key".to_string());
            env.insert("OPUS_SECRET".into(), secret_val.clone());
            local_port = Some(local_port.unwrap_or(8765));
            local_path = Some(format!("/{secret_val}/mcp"));
        }
        let detected_kind = if local_port.is_some() {
            "local_http"
        } else {
            "stdio"
        };
        return Some(Server {
            id: String::new(),
            name: n.clone(),
            slug: slug(&n),
            kind: detected_kind.into(),
            command: py_cmd,
            args: vec!["-m".into(), module],
            cwd: Some(dir.to_string_lossy().into()),
            env,
            local_port,
            mcp_path: local_path,
            auto_start: false,
            auto_restart: true,
            max_retries: 5,
            ..Default::default()
        });
    }

    for py_entry in ["server.py", "main.py", "mcp_server.py"] {
        if dir.join(py_entry).exists() {
            let n = dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let code = std::fs::read_to_string(dir.join(py_entry)).unwrap_or_default();
            let (local_port, local_path, mut env) = analyze_source_text(&code, &default_ws);
            for (k, v) in example_env(dir) {
                env.entry(k).or_insert(v);
            }
            let detected_kind = if local_port.is_some() {
                "local_http"
            } else {
                "stdio"
            };
            return Some(Server {
                id: String::new(),
                name: n.clone(),
                slug: slug(&n),
                kind: detected_kind.into(),
                command: py_cmd,
                args: vec![py_entry.into()],
                cwd: Some(dir.to_string_lossy().into()),
                env,
                local_port,
                mcp_path: local_path,
                auto_start: true,
                auto_restart: true,
                max_retries: 5,
                ..Default::default()
            });
        }
    }

    None
}
fn scan(root: &str) -> Vec<Server> {
    let root_path = Path::new(root);
    let mut out = Vec::new();
    let mut seen_dirs = std::collections::HashSet::new();

    // Si c'est un fichier unique (.js, .py, .json)
    if root_path.is_file() {
        if let Some(parent) = root_path.parent() {
            if let Some(s) = inspect_directory(parent) {
                out.push(s);
                return out;
            }
        }
    }

    // D'abord inspecter la racine directement : si la racine est déjà un serveur MCP complet
    // (ex: mcp-montage ou opus-flutter-mcp), on retourne uniquement ce serveur sans scanner ses sous-dossiers internes !
    if root_path.is_dir() {
        if let Some(s) = inspect_directory(root_path) {
            out.push(s);
            return out;
        }
    }

    for e in WalkDir::new(root)
        .max_depth(3)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_str().unwrap_or("");
            name != "node_modules"
                && name != ".venv"
                && name != "target"
                && name != ".git"
                && name != ".agents"
                && name != "build"
        })
        .filter_map(Result::ok)
    {
        let p = e.path();
        if p.is_dir() && !seen_dirs.contains(p) {
            if let Some(s) = inspect_directory(p) {
                seen_dirs.insert(p.to_path_buf());
                out.push(s);
                continue;
            }
        }
        let file = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
        if file == "claude_desktop_config.json" || file == "mcp.json" {
            if let Some(j) = std::fs::read_to_string(p)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            {
                if let Some(m) = j
                    .pointer("/mcpServers")
                    .or_else(|| j.get("servers"))
                    .and_then(|x| x.as_object())
                {
                    for (n, v) in m {
                        out.push(Server {
                            id: String::new(),
                            name: n.clone(),
                            slug: slug(n),
                            kind: if v.get("url").is_some() {
                                "remote"
                            } else {
                                "stdio"
                            }
                            .into(),
                            command: v["command"].as_str().unwrap_or("").into(),
                            args: v["args"]
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|x| x.as_str().map(str::to_string))
                                        .collect()
                                })
                                .unwrap_or_default(),
                            env: v["env"]
                                .as_object()
                                .map(|m| {
                                    m.iter()
                                        .map(|(k, v)| {
                                            (k.clone(), v.as_str().unwrap_or("").to_string())
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                            url: v["url"].as_str().map(str::to_string),
                            ..Default::default()
                        });
                    }
                }
            }
        }
    }
    out
}

async fn discover(
    State(s): State<S>,
    h: HeaderMap,
    Json(v): Json<serde_json::Value>,
) -> Result<Json<Vec<Server>>, StatusCode> {
    auth(&h, &s).await?;
    Ok(Json(scan(
        v["path"].as_str().ok_or(StatusCode::BAD_REQUEST)?,
    )))
}

// Endpoint d'import universel : accepte du code brut collé OU un fichier/dossier, génère le projet et l'enregistre
async fn import_code(
    State(s): State<S>,
    h: HeaderMap,
    Json(v): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, Response> {
    auth(&h, &s).await.map_err(IntoResponse::into_response)?;
    let raw_name = v["name"].as_str().unwrap_or("Nouveau MCP").trim();
    let name = if raw_name.is_empty() {
        "Nouveau MCP"
    } else {
        raw_name
    };
    let sl = slug(name);
    let code = v["code"].as_str().unwrap_or("");
    let filename = v["filename"].as_str().unwrap_or("");
    let workspace = v["workspace"].as_str().unwrap_or("");

    if code.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Le code du MCP est vide").into_response());
    }

    let dir = managed_servers_dir().join(&sl);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    let is_python = filename.ends_with(".py")
        || code.contains("from mcp")
        || code.contains("import mcp")
        || code.contains("FastMCP")
        || code.contains("def ") && code.contains("import ");

    if is_python {
        let entry = if filename.ends_with(".py") && !filename.is_empty() {
            filename.to_string()
        } else {
            "server.py".to_string()
        };
        tokio::fs::write(dir.join(&entry), code)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
        if !dir.join("requirements.txt").exists() {
            let mut reqs = vec!["mcp>=1.0.0"];
            if code.contains("fastapi") {
                reqs.push("fastapi");
            }
            if code.contains("uvicorn") {
                reqs.push("uvicorn");
            }
            if code.contains("pydantic") {
                reqs.push("pydantic");
            }
            if code.contains("httpx") {
                reqs.push("httpx");
            }
            let _ = tokio::fs::write(dir.join("requirements.txt"), reqs.join("\n")).await;
        }
    } else {
        let entry = if (filename.ends_with(".js") || filename.ends_with(".mjs"))
            && !filename.is_empty()
        {
            filename.to_string()
        } else {
            "server.js".to_string()
        };
        tokio::fs::write(dir.join(&entry), code)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

        // Générer automatiquement un package.json adapté aux imports détectés
        let mut deps = serde_json::Map::new();
        deps.insert(
            "@modelcontextprotocol/sdk".into(),
            serde_json::Value::String("^1.0.0".into()),
        );
        if code.contains("from \"express\"") || code.contains("from 'express'") || code.contains("require('express')") {
            deps.insert("express".into(), serde_json::Value::String("^4.19.2".into()));
        }
        if code.contains("from \"zod\"") || code.contains("from 'zod'") || code.contains("require('zod')") {
            deps.insert("zod".into(), serde_json::Value::String("^3.23.8".into()));
        }
        if code.contains("from \"cors\"") || code.contains("from 'cors'") {
            deps.insert("cors".into(), serde_json::Value::String("^2.8.5".into()));
        }
        let pkg = serde_json::json!({
            "name": sl,
            "version": "1.0.0",
            "type": "module",
            "main": entry,
            "dependencies": deps
        });
        let _ = tokio::fs::write(
            dir.join("package.json"),
            serde_json::to_vec_pretty(&pkg).unwrap_or_default(),
        )
        .await;
    }

    let mut srv = inspect_directory(&dir).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Impossible de détecter le point d'entrée du MCP",
        )
            .into_response()
    })?;
    srv.name = name.to_string();
    srv.slug = sl;
    if !workspace.is_empty() {
        if srv.env.contains_key("PROJET") {
            srv.env.insert("PROJET".into(), workspace.to_string());
        }
        if srv.env.contains_key("OPUS_WORKSPACE") {
            srv.env.insert("OPUS_WORKSPACE".into(), workspace.to_string());
        }
    }

    // Sauvegarder et démarrer immédiatement le MCP importé !
    srv.id = format!("{:016x}", rand::random::<u64>());
    let id = srv.id.clone();
    {
        let mut c = s.config.lock().await;
        if let Some(pos) = c.servers.iter().position(|x| x.slug == srv.slug) {
            srv.id = c.servers[pos].id.clone();
            c.servers[pos] = srv.clone();
        } else {
            c.servers.push(srv.clone());
        }
        save(&s, &c)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e).into_response())?;
    }
    let _ = start(&s, &srv.id).await;
    let r = s.runtime.lock().await;
    let c = s.config.lock().await;
    let pub_base = effective_public_base(&s, &c).await;
    Ok(Json(public_server(&srv, r.get(&id), c.port, &pub_base)))
}

async fn effective_public_base(s: &S, c: &Config) -> String {
    if !c.public_base_url.trim().is_empty()
        && !c.public_base_url.contains("127.0.0.1")
        && !c.public_base_url.contains("localhost")
    {
        return c.public_base_url.trim().trim_end_matches('/').to_string();
    }
    if let Some(t) = s.tunnel_url.lock().await.clone() {
        return t.trim_end_matches('/').to_string();
    }
    format!("http://127.0.0.1:{}", c.port)
}

// Vérifie le Bearer token du MCP OU le token présent dans l'URL (pour Mammouth AI)
async fn verify_mcp_bearer(s: &S, server: &Server, h: &HeaderMap, path_token: Option<&str>) -> bool {
    let mut r = s.runtime.lock().await;
    let fallback_tok = server
        .connection_token
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| server.env.get("MCP_TOKEN").cloned().filter(|t| !t.trim().is_empty() && t != "••••••••"))
        .unwrap_or_else(generate_mcp_token);
    let q = r.entry(server.id.clone()).or_insert_with(|| Runtime {
        id: server.id.clone(),
        active_token: fallback_tok.clone(),
        process_token: fallback_tok.clone(),
        ..Default::default()
    });
    if q.active_token.is_empty() {
        q.active_token = fallback_tok;
    }
    let expected = q.active_token.clone();
    drop(r);

    if let Some(pt) = path_token {
        let trimmed = pt.trim();
        // Si l'URL appelée est /mcp/:slug/mcp, axum capture "mcp" comme second segment :
        // on l'ignore en tant que token de chemin et on vérifie les headers Authorization / x-mcp-token.
        if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("mcp") && trimmed == expected {
            return true;
        }
    }
    // Si le client fournit Authorization: Bearer <token> ou x-mcp-token
    if let Some(auth_val) = h.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(tok) = auth_val
            .strip_prefix("Bearer ")
            .or_else(|| auth_val.strip_prefix("bearer "))
        {
            return tok.trim() == expected;
        }
        if auth_val.trim() == expected {
            return true;
        }
    }
    if let Some(tok) = h.get("x-mcp-token").and_then(|v| v.to_str().ok()) {
        return tok.trim() == expected;
    }
    false
}

async fn free_local_port(port: u16) {
    if StdTcpListener::bind(("127.0.0.1", port)).is_ok() {
        return;
    }
    #[cfg(windows)]
    {
        let mut cmd = Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-Command",
            &format!(
                "Get-NetTCPConnection -LocalPort {port} -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique | ForEach-Object {{ Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue }}"
            ),
        ]);
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
        let _ = cmd.output().await;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn gateway(
    State(s): State<S>,
    method: axum::http::Method,
    AxumPath(sl): AxumPath<String>,
    h: HeaderMap,
    b: Bytes,
) -> Response {
    gateway_inner(s, method, sl, None, h, b).await
}

async fn gateway_with_token(
    State(s): State<S>,
    method: axum::http::Method,
    AxumPath((sl, tok)): AxumPath<(String, String)>,
    h: HeaderMap,
    b: Bytes,
) -> Response {
    gateway_inner(s, method, sl, Some(tok), h, b).await
}

async fn gateway_inner(
    s: S,
    method: axum::http::Method,
    sl: String,
    path_tok: Option<String>,
    h: HeaderMap,
    b: Bytes,
) -> Response {
    let cfg = s
        .config
        .lock()
        .await
        .servers
        .iter()
        .find(|x| x.slug == sl)
        .cloned();
    let Some(x) = cfg else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // Vérifier d'abord le token de connexion (Header Authorization OU chemin URL Mammouth)
    if !verify_mcp_bearer(&s, &x, &h, path_tok.as_deref()).await {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Token MCP invalide. Vérifiez le token ou l'URL copiée depuis MCP Manager."
            })),
        )
            .into_response();
    }

    // Si le serveur est arrêté, mais configuré en auto_start, s'assurer qu'il est bien en ligne
    let is_running = s
        .runtime
        .lock()
        .await
        .get(&x.id)
        .map(|r| r.running)
        .unwrap_or(false);
    if !is_running {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": format!("Le serveur MCP '{}' est arrêté dans MCP Manager.", x.name)
            })),
        )
            .into_response();
    }

    // 1. Cas MCP local qui écoute sur son propre port HTTP (ex: mcp-montage sur 3333 ou opus-flutter-mcp sur 8765)
    // Prioritaire si local_port est défini, même si kind == "remote" (exposé via le tunnel de la passerelle) !
    if let Some(local_port) = x.local_port {
        let internal_token = {
            let r = s.runtime.lock().await;
            r.get(&x.id)
                .map(|rt| {
                    if !rt.process_token.is_empty() {
                        rt.process_token.clone()
                    } else {
                        rt.active_token.clone()
                    }
                })
                .unwrap_or_default()
        };
        // Pour opus-flutter-mcp, le chemin interne utilise le token du processus démarré ; sinon /mcp
        let path = if x.slug.contains("opus") || x.args.iter().any(|a| a.contains("opus_mcp")) {
            format!("/{internal_token}/mcp")
        } else {
            x.mcp_path.clone().unwrap_or_else(|| "/mcp".into())
        };
        let target_url = format!("http://127.0.0.1:{local_port}{path}");
        let req_method = reqwest::Method::from_bytes(method.as_str().as_bytes())
            .unwrap_or(reqwest::Method::POST);
        let mut q = s
            .client
            .request(req_method, &target_url)
            .header("Authorization", format!("Bearer {internal_token}"))
            .header("x-mcp-token", &internal_token);
        if !b.is_empty() {
            q = q.body(b);
        }
        let mut has_accept = false;
        for (k, v) in &h {
            let name = k.as_str().to_lowercase();
            if name == "accept" {
                has_accept = true;
            }
            if name != "host"
                && name != "x-mcp-hub-token"
                && name != "authorization"
                && name != "content-length"
            {
                q = q.header(k, v);
            }
        }
        if !has_accept {
            q = q.header("Accept", "application/json, text/event-stream");
        } else if let Some(acc) = h.get("accept").and_then(|v| v.to_str().ok()) {
            if !acc.contains("text/event-stream") || !acc.contains("application/json") {
                q = q.header("Accept", "application/json, text/event-stream");
            }
        }
        return match q.send().await {
            Ok(r) => {
                let mut builder = Response::builder().status(r.status());
                if let Some(h) = builder.headers_mut() {
                    for (k, v) in r.headers() {
                        h.insert(k.clone(), v.clone());
                    }
                }
                builder
                    .body(axum::body::Body::from_stream(r.bytes_stream()))
                    .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
            }
            Err(_) => StatusCode::BAD_GATEWAY.into_response(),
        };
    }

    // 2. Cas MCP purement distant (URL externe sans processus local)
    if x.kind == "remote" && x.command.trim().is_empty() {
        let Some(url) = x.url else {
            return StatusCode::BAD_GATEWAY.into_response();
        };
        let req_method = reqwest::Method::from_bytes(method.as_str().as_bytes())
            .unwrap_or(reqwest::Method::POST);
        let mut q = s.client.request(req_method, url);
        if !b.is_empty() {
            q = q.body(b);
        }
        for (k, v) in x.headers {
            if let Ok(v) = unprotect_if_encoded(&v) {
                q = q.header(k, v);
            }
        }
        for (k, v) in &h {
            let name = k.as_str().to_lowercase();
            if name != "host"
                && name != "x-mcp-hub-token"
                && name != "authorization"
                && name != "content-length"
            {
                q = q.header(k, v);
            }
        }
        return match q.send().await {
            Ok(r) => {
                let mut builder = Response::builder().status(r.status());
                if let Some(h) = builder.headers_mut() {
                    for (k, v) in r.headers() {
                        h.insert(k.clone(), v.clone());
                    }
                }
                builder
                    .body(axum::body::Body::from_stream(r.bytes_stream()))
                    .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
            }
            Err(_) => StatusCode::BAD_GATEWAY.into_response(),
        };
    }

    // 3. Cas MCP stdio standard
    let io = s.io.lock().await.get(&x.id).cloned();
    let Some(io) = io else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let msg: serde_json::Value = match serde_json::from_slice(&b) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let Some(id) = id_key(&msg) else {
        let mut input = io.input.lock().await;
        if input.write_all(&b).await.is_err() || input.write_all(b"\n").await.is_err() {
            return StatusCode::BAD_GATEWAY.into_response();
        }
        return StatusCode::ACCEPTED.into_response();
    };
    if msg.get("method").is_none() {
        let mut input = io.input.lock().await;
        if input.write_all(&b).await.is_err() || input.write_all(b"\n").await.is_err() {
            return StatusCode::BAD_GATEWAY.into_response();
        }
        return StatusCode::ACCEPTED.into_response();
    }
    let (tx, rx) = oneshot::channel();
    io.pending.lock().await.insert(id.clone(), tx);
    {
        let mut input = io.input.lock().await;
        if input.write_all(&b).await.is_err() || input.write_all(b"\n").await.is_err() {
            return StatusCode::BAD_GATEWAY.into_response();
        }
    }
    match tokio::time::timeout(Duration::from_secs(60), rx).await {
        Ok(Ok(reply)) => {
            if msg["method"] == "initialize" {
                let session = hex::encode(rand::random::<[u8; 16]>());
                s.sessions.lock().await.insert(session.clone(), x.id.clone());
                Response::builder()
                    .header("content-type", "application/json")
                    .header("Mcp-Session-Id", session)
                    .body(reply.to_string().into())
                    .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
            } else {
                Response::builder()
                    .header("content-type", "application/json")
                    .body(reply.to_string().into())
                    .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
            }
        }
        _ => {
            io.pending.lock().await.remove(&id);
            StatusCode::GATEWAY_TIMEOUT.into_response()
        }
    }
}


fn id_key(v: &serde_json::Value) -> Option<String> {
    v.get("id").map(|id| match id {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        _ => id.to_string(),
    })
}

fn unprotect_if_encoded(v: &str) -> Result<String, String> {
    if v.starts_with("dpapi:") {
        unprotect(v)
    } else {
        Ok(v.to_string())
    }
}

async fn ensure_dependencies(app: &S, id: &str, srv: &Server) {
    let Some(cwd) = &srv.cwd else { return };
    let cwd_path = Path::new(cwd);

    if cwd_path.join("package.json").exists() && !cwd_path.join("node_modules").exists() {
        {
            let mut r = app.runtime.lock().await;
            if let Some(q) = r.get_mut(id) {
                append_log(q, "📦 Dépendances npm absentes, exécution de npm install...".into());
            }
        }
        let _ = Command::new("npm.cmd")
            .arg("install")
            .current_dir(cwd_path)
            .output()
            .await;
    }

    if cwd_path.join("requirements.txt").exists() && !cwd_path.join(".venv").exists() {
        {
            let mut r = app.runtime.lock().await;
            if let Some(q) = r.get_mut(id) {
                append_log(q, "📦 Environnement .venv absent, création et pip install...".into());
            }
        }
        let _ = Command::new("python")
            .args(["-m", "venv", ".venv"])
            .current_dir(cwd_path)
            .output()
            .await;
        let pip_exe = cwd_path.join(".venv").join("Scripts").join("pip.exe");
        let _ = Command::new(pip_exe)
            .args(["install", "-r", "requirements.txt"])
            .current_dir(cwd_path)
            .output()
            .await;
    }
}

async fn stdout_reader(
    id: String,
    stdout: tokio::process::ChildStdout,
    io: Arc<IoState>,
    app: S,
) {
    let mut reader = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        {
            let mut r = app.runtime.lock().await;
            if let Some(q) = r.get_mut(&id) {
                append_log(q, format!("[out] {trimmed}"));
            }
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(msg_id) = id_key(&val) {
                if let Some(sender) = io.pending.lock().await.remove(&msg_id) {
                    let _ = sender.send(val.clone());
                }
            }
            let _ = io.events.send(trimmed.to_string());
        }
    }
}

async fn stderr_reader(
    id: String,
    stderr: tokio::process::ChildStderr,
    app: S,
) {
    let mut reader = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut r = app.runtime.lock().await;
        if let Some(q) = r.get_mut(&id) {
            append_log(q, format!("[err] {trimmed}"));
        }
    }
}

async fn admin_state(
    State(s): State<S>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    auth(&h, &s).await?;
    let c = s.config.lock().await.clone();
    let pub_base = effective_public_base(&s, &c).await;
    let tunnel_active = s.tunnel_url.lock().await.clone();
    let mut r = s.runtime.lock().await;
    let servers: Vec<serde_json::Value> = c
        .servers
        .iter()
        .map(|srv| {
            let tok = srv
                .connection_token
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(generate_mcp_token);
            let q = r.entry(srv.id.clone()).or_insert_with(|| Runtime {
                id: srv.id.clone(),
                active_token: tok.clone(),
                process_token: tok.clone(),
                ..Default::default()
            });
            if q.active_token.is_empty() {
                q.active_token = tok;
            }
            public_server(srv, Some(q), c.port, &pub_base)
        })
        .collect();
    Ok(Json(serde_json::json!({
        "port": c.port,
        "publicBaseUrl": pub_base,
        "customPublicUrl": c.public_base_url,
        "tunnelUrl": tunnel_active,
        "tunnelProvider": if c.tunnel_provider.is_empty() { "tailscale" } else { c.tunnel_provider.as_str() },
        "enableTunnel": c.enable_tunnel,
        "ngrokDomain": c.ngrok_domain,
        "hasNgrokToken": !c.ngrok_auth_token.is_empty(),
        "hasCloudflareToken": !c.cloudflare_tunnel_token.is_empty(),
        "startWithWindows": c.start_with_windows,
        "servers": servers,
    })))
}

async fn save_server(
    State(s): State<S>,
    h: HeaderMap,
    Json(v): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, Response> {
    auth(&h, &s).await.map_err(IntoResponse::into_response)?;
    let mut srv: Server = serde_json::from_value(v.clone())
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()).into_response())?;
    if srv.name.trim().is_empty() {
        srv.name = "Serveur MCP".into();
    }
    if srv.slug.trim().is_empty() {
        srv.slug = slug(&srv.name);
    }
    if srv.id.trim().is_empty() {
        srv.id = format!("{:016x}", rand::random::<u64>());
    }
    let id = srv.id.clone();
    let (hub_port, pub_base) = {
        let mut c = s.config.lock().await;
        if let Some(pos) = c.servers.iter().position(|x| x.id == srv.id || x.slug == srv.slug) {
            srv.id = c.servers[pos].id.clone();
            if srv.connection_token.is_none() {
                srv.connection_token = c.servers[pos].connection_token.clone();
            }
            // Conserver les secrets existants si "••••••••" est renvoyé par l'UI
            for (k, val) in &mut srv.env {
                if val == "••••••••" {
                    if let Some(old) = c.servers[pos].env.get(k) {
                        *val = old.clone();
                    }
                }
            }
            for (k, val) in &mut srv.headers {
                if val == "••••••••" {
                    if let Some(old) = c.servers[pos].headers.get(k) {
                        *val = old.clone();
                    }
                }
            }
            c.servers[pos] = srv.clone();
        } else {
            if srv.connection_token.is_none() {
                srv.connection_token = Some(generate_mcp_token());
            }
            c.servers.push(srv.clone());
        }
        save(&s, &c)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e).into_response())?;
        let pb = effective_public_base(&s, &c).await;
        (c.port, pb)
    };
    let mut r = s.runtime.lock().await;
    let tok = srv.connection_token.clone().unwrap_or_else(generate_mcp_token);
    let q = r.entry(id.clone()).or_insert_with(|| Runtime {
        id: id.clone(),
        active_token: tok,
        ..Default::default()
    });
    Ok(Json(public_server(&srv, Some(q), hub_port, &pub_base)))
}

async fn rotate_token_handler(
    State(s): State<S>,
    h: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    auth(&h, &s).await?;
    let new_tok = generate_mcp_token();

    // 1. Sauvegarde dans la configuration du serveur (connection_token + variables d'environnement pour le prochain lancement)
    {
        let mut cfg = s.config.lock().await;
        if let Some(srv) = cfg.servers.iter_mut().find(|x| x.id == id) {
            srv.connection_token = Some(new_tok.clone());
            srv.env.insert("MCP_TOKEN".into(), new_tok.clone());
            srv.env.insert("OPUS_BEARER".into(), new_tok.clone());
            srv.env.insert("OPUS_SECRET".into(), new_tok.clone());
            if let Some(ref key) = srv.token_env_key {
                srv.env.insert(key.clone(), new_tok.clone());
            }
        }
        let _ = save(&s, &cfg).await;
    }

    // 2. Mise à jour immédiate du token externe (`active_token`) SANS écraser `process_token`
    // Le processus en cours continue d'être contacté avec son `process_token` de lancement,
    // ce qui permet d'utiliser immédiatement le nouveau token sans redémarrer le MCP !
    let mut r = s.runtime.lock().await;
    let q = r.entry(id.clone()).or_insert_with(|| Runtime {
        id: id.clone(),
        ..Default::default()
    });
    if q.process_token.is_empty() && !q.active_token.is_empty() {
        q.process_token = q.active_token.clone();
    }
    q.active_token = new_tok.clone();
    append_log(
        q,
        format!("🔑 Nouveau token de connexion actif immédiatement (sans redémarrage requis) : {new_tok}"),
    );

    Ok(Json(serde_json::json!({
        "connectionToken": new_tok,
        "activeToken": new_tok,
        "status": "ok"
    })))
}

async fn import_auto(
    State(s): State<S>,
    h: HeaderMap,
    Json(v): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, Response> {
    auth(&h, &s).await.map_err(IntoResponse::into_response)?;
    let raw_path = v["path"].as_str().unwrap_or("").trim();
    let workspace = v["workspace"].as_str().unwrap_or("").trim();
    let auto_start = v["autoStart"].as_bool().unwrap_or(true);
    let requested_kind = v["kind"].as_str().unwrap_or("auto").trim();
    let remote_url = v["remoteUrl"].as_str().unwrap_or("").trim();

    if raw_path.is_empty() && requested_kind != "remote" {
        return Err((StatusCode::BAD_REQUEST, "Chemin vide").into_response());
    }
    let src_path = Path::new(raw_path);
    if requested_kind != "remote" && !src_path.exists() {
        return Err((StatusCode::BAD_REQUEST, format!("Le chemin n'existe pas : {raw_path}")).into_response());
    }

    // Si c'est une archive .zip, on l'extrait dans managed_servers_dir()
    let target_path: PathBuf = if src_path.is_file()
        && src_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("zip"))
            .unwrap_or(false)
    {
        let stem = src_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("imported-mcp");
        let dest = managed_servers_dir().join(slug(stem));
        let _ = std::fs::create_dir_all(&dest);
        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                    src_path.display(),
                    dest.display()
                ),
            ])
            .status()
            .await;
        if status.map(|st| !st.success()).unwrap_or(true) {
            return Err((StatusCode::BAD_REQUEST, "Échec de l'extraction de l'archive .zip").into_response());
        }
        dest
    } else if src_path.is_file() {
        let ext = src_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        // Si c'est un fichier isolé (.py, .js, .mjs, .ts) hors d'un dossier projet complet, ou si inspect_directory sur le parent échoue
        let parent = src_path.parent().unwrap_or(src_path);
        if (ext == "py" || ext == "js" || ext == "mjs" || ext == "ts")
            && inspect_directory(parent).is_none()
        {
            let code = tokio::fs::read_to_string(src_path)
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()).into_response())?;
            let filename = src_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("server.js");
            let name = src_path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("Custom MCP");
            return import_code(
                State(s),
                h,
                Json(serde_json::json!({
                    "name": name,
                    "code": code,
                    "filename": filename,
                    "workspace": workspace,
                    "autoStart": auto_start,
                })),
            )
            .await;
        }
        parent.to_path_buf()
    } else {
        src_path.to_path_buf()
    };

    let mut found = scan(&target_path.to_string_lossy());
    if found.is_empty() {
        if let Some(srv) = inspect_directory(&target_path) {
            found.push(srv);
        }
    }
    if found.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Aucun MCP détecté dans '{}'. Vérifiez qu'il contient server.js, package.json, requirements.txt ou un module Python.",
                target_path.display()
            ),
        )
            .into_response());
    }

    let mut imported_ids = Vec::new();
    let hub_port = {
        let mut c = s.config.lock().await;
        // Nettoyer automatiquement l'éventuel doublon `opus_mcp` ou `opus-mcp` créé par l'ancien scan récursif
        c.servers.retain(|x| x.name != "opus_mcp" && x.slug != "opus-mcp");

        for mut srv in found {
            // Appliquer le choix de mode demandé par l'utilisateur lors de l'import
            match requested_kind {
                "stdio" => {
                    srv.kind = "stdio".into();
                    srv.local_port = None;
                }
                "local_http" => {
                    srv.kind = "local_http".into();
                    if srv.local_port.is_none() {
                        srv.local_port = Some(3333);
                    }
                    if srv.mcp_path.is_none() {
                        srv.mcp_path = Some("/mcp".into());
                    }
                }
                "remote" => {
                    srv.kind = "remote".into();
                    if !remote_url.is_empty() {
                        srv.url = Some(remote_url.to_string());
                    } else if let Some(p) = srv.local_port {
                        let mp = srv.mcp_path.clone().unwrap_or_else(|| "/mcp".into());
                        srv.url = Some(format!("http://127.0.0.1:{p}{mp}"));
                    }
                }
                _ => {
                    // "auto" : garde local_http si un port HTTP est détecté, sinon stdio
                }
            }

            if !workspace.is_empty() {
                srv.env.insert("PROJET".into(), workspace.to_string());
                srv.env.insert("OPUS_WORKSPACE".into(), workspace.to_string());
                if let Some(wk) = srv.workspace_env_key.clone() {
                    srv.env.insert(wk, workspace.to_string());
                }
            }
            if srv.id.is_empty() {
                srv.id = format!("{:016x}", rand::random::<u64>());
            }
            if let Some(pos) = c.servers.iter().position(|x| x.slug == srv.slug) {
                srv.id = c.servers[pos].id.clone();
                c.servers[pos] = srv.clone();
            } else {
                c.servers.push(srv.clone());
            }
            imported_ids.push(srv.id.clone());
        }
        save(&s, &c)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e).into_response())?;
        c.port
    };

    for id in &imported_ids {
        {
            let mut r = s.runtime.lock().await;
            let q = r.entry(id.clone()).or_insert_with(|| Runtime {
                id: id.clone(),
                active_token: generate_mcp_token(),
                ..Default::default()
            });
            if q.active_token.is_empty() {
                q.active_token = generate_mcp_token();
            }
        }
        if auto_start {
            let _ = restart(&s, id).await;
        }
    }

    let c = s.config.lock().await;
    let pub_base = effective_public_base(&s, &c).await;
    let r = s.runtime.lock().await;
    let first = c
        .servers
        .iter()
        .find(|x| imported_ids.contains(&x.id))
        .map(|srv| public_server(srv, r.get(&srv.id), hub_port, &pub_base))
        .unwrap_or_else(|| serde_json::json!({ "ok": true }));

    Ok(Json(first))
}

fn find_cloudflared_bin() -> Option<String> {
    if let Ok(p) = std::env::var("CLOUDFLARED_BIN") {
        if Path::new(&p).exists() {
            return Some(p);
        }
    }
    for candidate in [
        "cloudflared.exe",
        "cloudflared",
        r"C:\Program Files (x86)\cloudflared\cloudflared.exe",
        r"C:\Program Files\cloudflared\cloudflared.exe",
    ] {
        if candidate.contains('\\') {
            if Path::new(candidate).exists() {
                return Some(candidate.to_string());
            }
        } else if std::process::Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok()
        {
            return Some(candidate.to_string());
        }
    }
    None
}

fn find_tailscale_bin() -> Option<String> {
    if let Ok(p) = std::env::var("TAILSCALE_BIN") {
        if Path::new(&p).exists() {
            return Some(p);
        }
    }
    for candidate in [
        r"C:\Program Files\Tailscale\tailscale.exe",
        r"C:\Program Files (x86)\Tailscale\tailscale.exe",
        "tailscale.exe",
        "tailscale",
    ] {
        if candidate.contains('\\') {
            if Path::new(candidate).exists() {
                return Some(candidate.to_string());
            }
        } else if std::process::Command::new(candidate)
            .arg("version")
            .output()
            .is_ok()
        {
            return Some(candidate.to_string());
        }
    }
    None
}

fn find_ngrok_bin() -> Option<String> {
    if let Ok(p) = std::env::var("NGROK_BIN") {
        if Path::new(&p).exists() {
            return Some(p);
        }
    }
    for candidate in [
        "ngrok.exe",
        "ngrok",
        r"C:\Program Files\ngrok\ngrok.exe",
    ] {
        if candidate.contains('\\') {
            if Path::new(candidate).exists() {
                return Some(candidate.to_string());
            }
        } else if std::process::Command::new(candidate)
            .arg("version")
            .output()
            .is_ok()
        {
            return Some(candidate.to_string());
        }
    }
    None
}

async fn detect_tailscale_dns(bin: &str) -> Option<String> {
    // 1. Essayer via `tailscale status --json`
    let mut cmd = Command::new(bin);
    cmd.args(["status", "--json"]);
    #[cfg(windows)]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    if let Ok(out) = cmd.output().await {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
            if let Some(dns) = v
                .pointer("/Self/DNSName")
                .and_then(|d| d.as_str())
                .map(|s| s.trim().trim_end_matches('.'))
                .filter(|s| !s.is_empty())
            {
                return Some(format!("https://{dns}"));
            }
            if let Some(suffix) = v
                .pointer("/MagicDNSSuffix")
                .and_then(|d| d.as_str())
                .map(|s| s.trim().trim_end_matches('.'))
                .filter(|s| !s.is_empty())
            {
                if let Some(host) = v
                    .pointer("/Self/HostName")
                    .and_then(|h| h.as_str())
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                {
                    return Some(format!("https://{host}.{suffix}"));
                }
            }
        }
    }

    // 2. Fallback via `tailscale funnel status` ou `tailscale serve status`
    for sub in ["funnel", "serve"] {
        let mut fcmd = Command::new(bin);
        fcmd.args([sub, "status"]);
        #[cfg(windows)]
        {
            #[allow(unused_imports)]
            use std::os::windows::process::CommandExt;
            fcmd.creation_flags(0x08000000);
        }
        if let Ok(out) = fcmd.output().await {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if let Some(idx) = line.find("https://") {
                    let rest = &line[idx..];
                    let end = rest
                        .find(|c: char| c.is_whitespace() || c == '/' || c == ')')
                        .unwrap_or(rest.len());
                    let url = rest[..end].trim().trim_end_matches('/');
                    if url.contains(".ts.net") {
                        return Some(url.to_string());
                    }
                }
            }
        }
    }
    None
}

// Lance et maintient le tunnel HTTPS permanent vers la passerelle locale (port 7300).
// Priorité par défaut : Tailscale Funnel (URL permanente https://<machine>.<tailnet>.ts.net qui ne change jamais, même après extinction du PC).
// Dès que le domaine permanent est détecté, il est sauvegardé dans config.json (`public_base_url`) pour rester 100% fixe à vie !
async fn supervise_hub_tunnel(app: S, port: u16) {
    loop {
        let (enabled, provider, ngrok_tok, ngrok_dom, cf_tok, current_pub) = {
            let c = app.config.lock().await;
            let prov = if c.tunnel_provider.trim().is_empty() {
                "tailscale".to_string()
            } else {
                c.tunnel_provider.trim().to_lowercase()
            };
            (
                c.enable_tunnel,
                prov,
                c.ngrok_auth_token.clone(),
                c.ngrok_domain.clone(),
                c.cloudflare_tunnel_token.clone(),
                c.public_base_url.clone(),
            )
        };

        if !enabled || provider == "none" || provider == "local" {
            *app.tunnel_url.lock().await = None;
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        // 1. Mode TAILSCALE FUNNEL (Recommandé : gratuit, permanent à vie sur chaque PC, illimité)
        if provider == "tailscale" {
            if let Some(ts_bin) = find_tailscale_bin() {
                if let Some(permanent_url) = detect_tailscale_dns(&ts_bin).await {
                    *app.tunnel_url.lock().await = Some(permanent_url.clone());
                    // Sauvegarder définitivement l'URL permanente dans config.json pour qu'elle ne bouge JAMAIS
                    if current_pub != permanent_url {
                        let mut c = app.config.lock().await;
                        c.public_base_url = permanent_url.clone();
                        let _ = save(&app, &c).await;
                    }
                }
                // Activer Tailscale Funnel en arrière-plan (`tailscale funnel --bg <port>`)
                let mut bg_cmd = Command::new(&ts_bin);
                bg_cmd.args(["funnel", "--bg", "--yes", &port.to_string()]);
                #[cfg(windows)]
                {
                    #[allow(unused_imports)]
                    use std::os::windows::process::CommandExt;
                    bg_cmd.creation_flags(0x08000000);
                }
                let bg_res = bg_cmd.output().await;
                let bg_ok = bg_res.as_ref().map(|o| o.status.success()).unwrap_or(false);
                if !bg_ok {
                    // Fallback sans --yes pour les versions CLI Tailscale qui ne supportent pas --yes
                    let mut bg_cmd2 = Command::new(&ts_bin);
                    bg_cmd2.args(["funnel", "--bg", &port.to_string()]);
                    #[cfg(windows)]
                    {
                        #[allow(unused_imports)]
                        use std::os::windows::process::CommandExt;
                        bg_cmd2.creation_flags(0x08000000);
                    }
                    let _ = bg_cmd2.output().await;
                }
                tokio::time::sleep(Duration::from_secs(15)).await;
                continue;
            }
        }

        // 2. Mode NGROK DOMAINE STATIQUE PERMANENT
        if provider == "ngrok" {
            if let Some(ng_bin) = find_ngrok_bin() {
                if !ngrok_tok.is_empty() {
                    let mut auth_cmd = Command::new(&ng_bin);
                    auth_cmd.args(["config", "add-authtoken", &ngrok_tok]);
                    #[cfg(windows)]
                    {
                        #[allow(unused_imports)]
                        use std::os::windows::process::CommandExt;
                        auth_cmd.creation_flags(0x08000000);
                    }
                    let _ = auth_cmd.output().await;
                }
                let mut cmd = Command::new(&ng_bin);
                if !ngrok_dom.is_empty() {
                    let full_url = format!("https://{ngrok_dom}");
                    *app.tunnel_url.lock().await = Some(full_url.clone());
                    if current_pub != full_url {
                        let mut c = app.config.lock().await;
                        c.public_base_url = full_url;
                        let _ = save(&app, &c).await;
                    }
                    cmd.args(["http", &format!("--url={ngrok_dom}"), &port.to_string()]);
                } else {
                    cmd.args(["http", &port.to_string()]);
                }
                cmd.stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                #[cfg(windows)]
                {
                    #[allow(unused_imports)]
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x08000000);
                }
                if let Ok(mut child) = cmd.spawn() {
                    let _ = child.wait().await;
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }

        // 3. Mode CLOUDFLARE NAMED TUNNEL (avec Token fixe) uniquement si un token fixe ou provider cloudflare est explicitement choisi
        // IMPORTANT : Si `public_base_url` est déjà enregistrée, on ne l'écrase JAMAIS par une URL `.trycloudflare.com` aléatoire !
        if provider == "cloudflare" {
            if let Some(bin) = find_cloudflared_bin() {
                let mut cmd = Command::new(&bin);
                if !cf_tok.is_empty() {
                    cmd.args(["tunnel", "--no-autoupdate", "run", "--token", &cf_tok]);
                } else {
                    cmd.args([
                        "tunnel",
                        "--no-autoupdate",
                        "--url",
                        &format!("http://127.0.0.1:{port}"),
                    ]);
                }
                cmd.stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());

                #[cfg(windows)]
                {
                    #[allow(unused_imports)]
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x08000000);
                }

                if let Ok(mut child) = cmd.spawn() {
                    if cf_tok.is_empty() {
                        if let Some(stderr) = child.stderr.take() {
                            let app2 = app.clone();
                            tokio::spawn(async move {
                                let mut lines = BufReader::new(stderr).lines();
                                while let Ok(Some(line)) = lines.next_line().await {
                                    if let Some(pos) = line.find("https://") {
                                        let rest = &line[pos..];
                                        let end = rest
                                            .find(|c: char| c.is_whitespace() || c == '|' || c == '"')
                                            .unwrap_or(rest.len());
                                        let url = &rest[..end];
                                        if url.ends_with(".trycloudflare.com") {
                                            *app2.tunnel_url.lock().await = Some(url.to_string());
                                        }
                                    }
                                }
                            });
                        }
                    }
                    let _ = child.wait().await;
                    *app.tunnel_url.lock().await = None;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg_dir = config_path();
    std::fs::create_dir_all(&cfg_dir).ok();
    let cfg_file = cfg_dir.join("config.json");

    let raw_cfg: Config = if cfg_file.exists() {
        std::fs::read_to_string(&cfg_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        Config::default()
    };
    let (mut initial_config, _) = loaded(raw_cfg);
    initial_config
        .servers
        .retain(|x| x.name != "opus_mcp" && x.slug != "opus-mcp");

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(initial_config.port);

    let token = std::env::var("MCP_HUB_TOKEN").unwrap_or_else(|_| generate_mcp_token());

    let mut initial_runtime = HashMap::new();
    let mut config_updated = false;
    for srv in &mut initial_config.servers {
        let persistent_tok = srv
            .connection_token
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| {
                srv.env
                    .get("MCP_TOKEN")
                    .or_else(|| srv.env.get("OPUS_BEARER"))
                    .or_else(|| srv.env.get("OPUS_SECRET"))
                    .cloned()
                    .filter(|t| !t.trim().is_empty() && t != "••••••••")
            })
            .unwrap_or_else(|| {
                config_updated = true;
                generate_mcp_token()
            });
        if srv.connection_token.as_deref() != Some(persistent_tok.as_str()) {
            srv.connection_token = Some(persistent_tok.clone());
            config_updated = true;
        }
        initial_runtime.insert(
            srv.id.clone(),
            Runtime {
                id: srv.id.clone(),
                active_token: persistent_tok.clone(),
                process_token: persistent_tok,
                ..Default::default()
            },
        );
    }

    let app_state: S = Arc::new(App {
        config: Mutex::new(initial_config.clone()),
        runtime: Mutex::new(initial_runtime),
        io: Mutex::new(HashMap::new()),
        supervisors: Mutex::new(HashMap::new()),
        pids: Mutex::new(HashMap::new()),
        token: token.clone(),
        file: cfg_file,
        client: reqwest::Client::new(),
        sessions: Mutex::new(HashMap::new()),
        tunnel_url: Mutex::new(None),
    });

    if config_updated {
        let _ = save(&app_state, &initial_config).await;
    }

    // Démarrer automatiquement le tunnel HTTPS global vers la passerelle locale (port 7300)
    tokio::spawn(supervise_hub_tunnel(app_state.clone(), port));

    // Ne démarrer automatiquement les serveurs que si l'option globale `auto_start_all_mcps` est activée
    if initial_config.auto_start_all_mcps {
        let auto_start_ids: Vec<String> = initial_config
            .servers
            .iter()
            .map(|s| s.id.clone())
            .collect();
        for id in auto_start_ids {
            let _ = start(&app_state, &id).await;
        }
    }

    let router = Router::new()
        // Routes attendues par l'application Flutter (/admin/*)
        .route("/admin/state", get(admin_state))
        .route("/admin/shutdown", post(admin_shutdown))
        .route("/admin/servers", post(save_server))
        .route("/admin/servers/:id", axum::routing::delete(del))
        .route("/admin/servers/:id/rotate-token", post(rotate_token_handler))
        .route("/admin/servers/:id/:action", post(power))
        .route("/admin/logs/:id", get(logs))
        .route("/admin/secret", post(secret))
        .route("/admin/settings", post(settings))
        .route("/admin/discover", post(discover))
        .route("/admin/import-auto", post(import_auto))
        .route("/admin/import-code", post(import_code))
        // Alias /api/* pour compatibilité
        .route("/api/state", get(admin_state))
        .route("/api/servers", post(save_server))
        .route("/api/servers/:id", axum::routing::delete(del))
        .route("/api/servers/:id/rotate-token", post(rotate_token_handler))
        .route("/api/servers/:id/:action", post(power))
        .route("/api/servers/:id/logs", get(logs))
        .route("/api/secret", post(secret))
        .route("/api/settings", post(settings))
        .route("/api/import-auto", post(import_auto))
        .route("/api/import-code", post(import_code))
        // Passerelle MCP à URL fixe (/mcp/:slug, /mcp/:slug/mcp, /mcp/:slug/:token, /mcp/:slug/:token/mcp)
        .route("/mcp/:slug", post(gateway).get(gateway).delete(gateway))
        .route(
            "/mcp/:slug/:token",
            post(gateway_with_token)
                .get(gateway_with_token)
                .delete(gateway_with_token),
        )
        .route(
            "/mcp/:slug/:token/mcp",
            post(gateway_with_token)
                .get(gateway_with_token)
                .delete(gateway_with_token),
        )
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(app_state.clone());

    free_local_port(port).await;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // Ligne lue par le processus Flutter pour récupérer le token et le port du Hub !
    println!("MCP_HUB_TOKEN={};MCP_HUB_PORT={}", token, port);

    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}