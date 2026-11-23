use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;

const DOCKER_CMD_TIMEOUT: Duration = Duration::from_secs(2);
const REDIS_PROBE_TIMEOUT: Duration = Duration::from_millis(1200);
const MANTICORE_PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedRedisService {
    pub source: String,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub container_name: Option<String>,
    pub container_image: Option<String>,
    pub reachable: bool,
    pub requires_auth: bool,
    pub version: Option<String>,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedManticoreService {
    pub source: String,
    pub label: String,
    pub url: String,
    pub container_name: Option<String>,
    pub container_image: Option<String>,
    pub reachable: bool,
    pub requires_auth: bool,
    pub has_torrents_table: Option<bool>,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDiscoveryReport {
    pub docker_available: bool,
    pub redis: Vec<DetectedRedisService>,
    pub manticore: Vec<DetectedManticoreService>,
}

#[derive(Debug, Clone)]
struct RedisCandidate {
    source: String,
    label: String,
    host: String,
    port: u16,
    container_name: Option<String>,
    container_image: Option<String>,
    keep_if_unreachable: bool,
}

#[derive(Debug, Clone)]
struct ManticoreCandidate {
    source: String,
    label: String,
    url: String,
    container_name: Option<String>,
    container_image: Option<String>,
    keep_if_unreachable: bool,
}

/// 自动探测本机、环境变量与运行中的 Docker 容器里的 Redis 和 ManticoreSearch 服务。
/// 安全约束：绝不返回任何从容器或环境变量中提取的密码明文。
pub async fn detect_services() -> ServiceDiscoveryReport {
    let (docker_available, docker_redis, docker_manticore) = discover_docker_services().await;

    let mut redis_map: HashMap<(String, u16), RedisCandidate> = HashMap::new();
    let mut manticore_map: HashMap<String, ManticoreCandidate> = HashMap::new();

    // 1. Docker 发现的候选项优先写入（保留容器名与镜像元数据）
    for item in docker_redis {
        let key = (normalize_host(&item.host), item.port);
        redis_map.entry(key).or_insert(item);
    }
    for item in docker_manticore {
        let key = normalize_url_key(&item.url);
        manticore_map.entry(key).or_insert(item);
    }

    // 2. 环境变量候选项（仅提取 host/port/url，绝不泄露密钥）
    for item in env_redis_candidates() {
        let key = (normalize_host(&item.host), item.port);
        redis_map.entry(key).or_insert(item);
    }
    for item in env_manticore_candidates() {
        let key = normalize_url_key(&item.url);
        manticore_map.entry(key).or_insert(item);
    }

    // 3. 本机默认端口与容器内常见网络主机名
    for item in default_redis_candidates() {
        let key = (normalize_host(&item.host), item.port);
        redis_map.entry(key).or_insert(item);
    }
    for item in default_manticore_candidates() {
        let key = normalize_url_key(&item.url);
        manticore_map.entry(key).or_insert(item);
    }

    let redis_candidates: Vec<RedisCandidate> = redis_map.into_values().collect();
    let manticore_candidates: Vec<ManticoreCandidate> = manticore_map.into_values().collect();

    let http_client = reqwest::Client::builder()
        .timeout(MANTICORE_PROBE_TIMEOUT)
        .connect_timeout(Duration::from_millis(900))
        .build()
        .ok();

    let redis_futs = redis_candidates.into_iter().map(probe_redis_candidate);
    let manticore_futs = manticore_candidates
        .into_iter()
        .map(|cand| probe_manticore_candidate(cand, http_client.clone()));

    let (redis_probed, manticore_probed) =
        tokio::join!(join_all(redis_futs), join_all(manticore_futs));

    let mut redis: Vec<DetectedRedisService> = redis_probed.into_iter().flatten().collect();
    let mut manticore: Vec<DetectedManticoreService> =
        manticore_probed.into_iter().flatten().collect();

    redis.sort_by_key(|r| {
        (
            !r.reachable,
            r.requires_auth,
            r.source != "docker",
            r.latency_ms.unwrap_or(u64::MAX),
            r.port,
        )
    });
    manticore.sort_by_key(|m| {
        (
            !m.reachable,
            m.requires_auth,
            !m.has_torrents_table.unwrap_or(false),
            m.source != "docker",
            m.latency_ms.unwrap_or(u64::MAX),
        )
    });

    ServiceDiscoveryReport {
        docker_available,
        redis,
        manticore,
    }
}

fn normalize_host(host: &str) -> String {
    let trimmed = host.trim().to_ascii_lowercase();
    if trimmed == "localhost" {
        "127.0.0.1".to_string()
    } else {
        trimmed
    }
}

fn normalize_url_key(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .replace("://localhost:", "://127.0.0.1:")
        .to_ascii_lowercase()
}

fn normalize_docker_host_ip(host_ip: &str) -> String {
    let trimmed = host_ip.trim();
    if trimmed.is_empty() || trimmed == "0.0.0.0" || trimmed == "::" || trimmed == "[::]" {
        "127.0.0.1".to_string()
    } else {
        trimmed.to_string()
    }
}

fn is_inside_container() -> bool {
    Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists()
}

fn default_redis_candidates() -> Vec<RedisCandidate> {
    let mut out = vec![RedisCandidate {
        source: "local".to_string(),
        label: "本机 Redis".to_string(),
        host: "127.0.0.1".to_string(),
        port: 6379,
        container_name: None,
        container_image: None,
        keep_if_unreachable: false,
    }];
    if is_inside_container() {
        for (host, label) in [
            ("redis", "Docker 网络 redis"),
            ("host.docker.internal", "宿主机 Redis"),
        ] {
            out.push(RedisCandidate {
                source: "docker".to_string(),
                label: label.to_string(),
                host: host.to_string(),
                port: 6379,
                container_name: None,
                container_image: None,
                keep_if_unreachable: false,
            });
        }
    }
    out
}

fn default_manticore_candidates() -> Vec<ManticoreCandidate> {
    let mut out = vec![ManticoreCandidate {
        source: "local".to_string(),
        label: "本机 Manticore".to_string(),
        url: "http://127.0.0.1:9308".to_string(),
        container_name: None,
        container_image: None,
        keep_if_unreachable: false,
    }];
    if is_inside_container() {
        for (host, label) in [
            ("manticore", "Docker 网络 manticore"),
            ("manticoresearch", "Docker 网络 manticoresearch"),
            ("host.docker.internal", "宿主机 Manticore"),
        ] {
            out.push(ManticoreCandidate {
                source: "docker".to_string(),
                label: label.to_string(),
                url: format!("http://{host}:9308"),
                container_name: None,
                container_image: None,
                keep_if_unreachable: false,
            });
        }
    }
    out
}

fn env_redis_candidates() -> Vec<RedisCandidate> {
    let mut out = Vec::new();
    if let Ok(raw_url) = std::env::var("REDIS_URL")
        && let Some((host, port)) = parse_redis_host_port(&raw_url)
    {
        out.push(RedisCandidate {
            source: "env".to_string(),
            label: "环境变量 REDIS_URL".to_string(),
            host,
            port,
            container_name: None,
            container_image: None,
            keep_if_unreachable: false,
        });
    }
    if let Ok(host) = std::env::var("REDIS_HOST") {
        let trimmed = host.trim();
        if !trimmed.is_empty() {
            let port = std::env::var("REDIS_PORT")
                .ok()
                .and_then(|v| v.trim().parse::<u16>().ok())
                .unwrap_or(6379);
            out.push(RedisCandidate {
                source: "env".to_string(),
                label: "环境变量 REDIS_HOST".to_string(),
                host: trimmed.to_string(),
                port,
                container_name: None,
                container_image: None,
                keep_if_unreachable: false,
            });
        }
    }
    out
}

fn env_manticore_candidates() -> Vec<ManticoreCandidate> {
    let mut out = Vec::new();
    if let Ok(raw_url) = std::env::var("MANTICORE_URL")
        && let Some(clean_url) = sanitize_http_base_url(&raw_url)
    {
        out.push(ManticoreCandidate {
            source: "env".to_string(),
            label: "环境变量 MANTICORE_URL".to_string(),
            url: clean_url,
            container_name: None,
            container_image: None,
            keep_if_unreachable: false,
        });
    }
    out
}

fn parse_redis_host_port(raw: &str) -> Option<(String, u16)> {
    let trimmed = raw.trim();
    let rest = trimmed
        .strip_prefix("redis://")
        .or_else(|| trimmed.strip_prefix("rediss://"))?;
    let authority = rest.split('/').next()?;
    let host_port = authority.rsplit('@').next()?;
    if host_port.is_empty() {
        return None;
    }
    if let Some(bracket_rest) = host_port.strip_prefix('[') {
        let (ipv6, after) = bracket_rest.split_once(']')?;
        let port = after
            .strip_prefix(':')
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(6379);
        return Some((ipv6.to_string(), port));
    }
    match host_port.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => {
            Some((h.to_string(), p.parse::<u16>().ok().unwrap_or(6379)))
        }
        _ => Some((host_port.to_string(), 6379)),
    }
}

fn sanitize_http_base_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let (scheme, rest) = if let Some(r) = trimmed.strip_prefix("http://") {
        ("http://", r)
    } else if let Some(r) = trimmed.strip_prefix("https://") {
        ("https://", r)
    } else {
        return None;
    };
    let authority = rest.split('/').next()?;
    let host_port = authority.rsplit('@').next()?;
    if host_port.is_empty() {
        return None;
    }
    Some(format!("{scheme}{host_port}"))
}

async fn run_cli_capture(bin: &str, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let output = tokio::time::timeout(DOCKER_CMD_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

async fn discover_docker_services() -> (bool, Vec<RedisCandidate>, Vec<ManticoreCandidate>) {
    let mut engine = None;
    let mut ps_stdout = None;
    for candidate_bin in ["docker", "podman"] {
        if let Some(out) = run_cli_capture(
            candidate_bin,
            &[
                "ps",
                "--no-trunc",
                "--format",
                "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Ports}}",
            ],
        )
        .await
        {
            engine = Some(candidate_bin);
            ps_stdout = Some(out);
            break;
        }
    }

    let (Some(bin), Some(ps_text)) = (engine, ps_stdout) else {
        return (false, Vec::new(), Vec::new());
    };

    let mut matched_ids: Vec<String> = Vec::new();
    for line in ps_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }
        let id = cols[0].trim();
        let name = cols[1].to_ascii_lowercase();
        let image = cols[2].to_ascii_lowercase();
        let ports = cols.get(3).copied().unwrap_or("").to_ascii_lowercase();

        if is_redis_container(&name, &image, &ports)
            || is_manticore_container(&name, &image, &ports)
        {
            matched_ids.push(id.to_string());
        }
    }

    if matched_ids.is_empty() {
        return (true, Vec::new(), Vec::new());
    }

    let mut inspect_args: Vec<&str> = Vec::with_capacity(matched_ids.len() + 1);
    inspect_args.push("inspect");
    for id in &matched_ids {
        inspect_args.push(id.as_str());
    }

    let Some(inspect_json) = run_cli_capture(bin, &inspect_args).await else {
        return (true, Vec::new(), Vec::new());
    };

    let (redis, manticore) = parse_docker_inspect_candidates(&inspect_json);
    (true, redis, manticore)
}

fn is_redis_container(name_lc: &str, image_lc: &str, ports_lc: &str) -> bool {
    let tokens = ["redis", "valkey", "dragonfly", "keydb"];
    tokens
        .iter()
        .any(|t| image_lc.contains(t) || name_lc.contains(t))
        || ports_lc.contains("6379/tcp")
}

fn is_manticore_container(name_lc: &str, image_lc: &str, ports_lc: &str) -> bool {
    let tokens = ["manticore", "sphinx"];
    tokens
        .iter()
        .any(|t| image_lc.contains(t) || name_lc.contains(t))
        || ports_lc.contains("9308/tcp")
}

fn parse_docker_inspect_candidates(
    inspect_json: &str,
) -> (Vec<RedisCandidate>, Vec<ManticoreCandidate>) {
    let Ok(Value::Array(containers)) = serde_json::from_str::<Value>(inspect_json) else {
        return (Vec::new(), Vec::new());
    };

    let mut redis_out = Vec::new();
    let mut manticore_out = Vec::new();

    for c in containers {
        let name = c
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_start_matches('/')
            .to_string();
        let image = c
            .pointer("/Config/Image")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let network_mode = c
            .pointer("/HostConfig/NetworkMode")
            .and_then(Value::as_str)
            .unwrap_or("");
        let ports_obj = c
            .pointer("/NetworkSettings/Ports")
            .and_then(Value::as_object);
        let ports_text = ports_obj
            .map(|m| m.keys().cloned().collect::<Vec<_>>().join(","))
            .unwrap_or_default()
            .to_ascii_lowercase();

        let name_lc = name.to_ascii_lowercase();
        let image_lc = image.to_ascii_lowercase();
        let container_ip = extract_container_ip(&c);
        let display_name = if name.is_empty() {
            image.clone()
        } else {
            name.clone()
        };

        if is_redis_container(&name_lc, &image_lc, &ports_text) {
            let endpoint = extract_mapped_port(ports_obj, "6379/tcp")
                .or_else(|| {
                    if image_lc.contains("redis")
                        || image_lc.contains("valkey")
                        || image_lc.contains("dragonfly")
                    {
                        extract_any_mapped_tcp_port(ports_obj)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    if network_mode == "host" {
                        Some(("127.0.0.1".to_string(), 6379))
                    } else {
                        container_ip.as_ref().map(|ip| (ip.clone(), 6379))
                    }
                });

            if let Some((host, port)) = endpoint {
                redis_out.push(RedisCandidate {
                    source: "docker".to_string(),
                    label: display_name.clone(),
                    host,
                    port,
                    container_name: if name.is_empty() {
                        None
                    } else {
                        Some(name.clone())
                    },
                    container_image: if image.is_empty() {
                        None
                    } else {
                        Some(image.clone())
                    },
                    keep_if_unreachable: true,
                });
            }
        }

        if is_manticore_container(&name_lc, &image_lc, &ports_text) {
            let endpoint = extract_mapped_port(ports_obj, "9308/tcp").or_else(|| {
                if network_mode == "host" {
                    Some(("127.0.0.1".to_string(), 9308))
                } else {
                    container_ip.as_ref().map(|ip| (ip.clone(), 9308))
                }
            });

            if let Some((host, port)) = endpoint {
                manticore_out.push(ManticoreCandidate {
                    source: "docker".to_string(),
                    label: display_name,
                    url: format!("http://{host}:{port}"),
                    container_name: if name.is_empty() { None } else { Some(name) },
                    container_image: if image.is_empty() { None } else { Some(image) },
                    keep_if_unreachable: true,
                });
            }
        }
    }

    (redis_out, manticore_out)
}

fn extract_mapped_port(
    ports_obj: Option<&serde_json::Map<String, Value>>,
    container_port_key: &str,
) -> Option<(String, u16)> {
    let bindings = ports_obj?.get(container_port_key)?.as_array()?;
    // 优先选择 IPv4 映射
    let mut fallback = None;
    for binding in bindings {
        let host_port = binding
            .get("HostPort")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<u16>().ok())?;
        let raw_ip = binding.get("HostIp").and_then(Value::as_str).unwrap_or("");
        let host = normalize_docker_host_ip(raw_ip);
        if !raw_ip.contains(':') {
            return Some((host, host_port));
        }
        if fallback.is_none() {
            fallback = Some((host, host_port));
        }
    }
    fallback
}

fn extract_any_mapped_tcp_port(
    ports_obj: Option<&serde_json::Map<String, Value>>,
) -> Option<(String, u16)> {
    let map = ports_obj?;
    for key in map.keys() {
        if key.ends_with("/tcp")
            && let Some(found) = extract_mapped_port(Some(map), key)
        {
            return Some(found);
        }
    }
    None
}

fn extract_container_ip(container: &Value) -> Option<String> {
    if let Some(ip) = container
        .pointer("/NetworkSettings/IPAddress")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        return Some(ip.trim().to_string());
    }
    if let Some(networks) = container
        .pointer("/NetworkSettings/Networks")
        .and_then(Value::as_object)
    {
        for net in networks.values() {
            if let Some(ip) = net
                .get("IPAddress")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
            {
                return Some(ip.trim().to_string());
            }
        }
    }
    None
}

async fn probe_redis_candidate(cand: RedisCandidate) -> Option<DetectedRedisService> {
    let url = format!("redis://{}:{}/0", cand.host, cand.port);
    let started = Instant::now();
    let client = match redis::Client::open(url) {
        Ok(c) => c,
        Err(_) => {
            return cand.keep_if_unreachable.then_some(DetectedRedisService {
                source: cand.source,
                label: cand.label,
                host: cand.host,
                port: cand.port,
                container_name: cand.container_name,
                container_image: cand.container_image,
                reachable: false,
                requires_auth: false,
                version: None,
                latency_ms: None,
            });
        }
    };

    let conn_res = tokio::time::timeout(
        REDIS_PROBE_TIMEOUT,
        client.get_multiplexed_async_connection(),
    )
    .await;

    let mut conn = match conn_res {
        Ok(Ok(c)) => c,
        _ => {
            return cand.keep_if_unreachable.then_some(DetectedRedisService {
                source: cand.source,
                label: cand.label,
                host: cand.host,
                port: cand.port,
                container_name: cand.container_name,
                container_image: cand.container_image,
                reachable: false,
                requires_auth: false,
                version: None,
                latency_ms: None,
            });
        }
    };

    let ping_res: Result<Result<String, redis::RedisError>, _> = tokio::time::timeout(
        REDIS_PROBE_TIMEOUT,
        redis::cmd("PING").query_async(&mut conn),
    )
    .await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(0);

    match ping_res {
        Ok(Ok(_pong)) => {
            let info_server: String = tokio::time::timeout(
                Duration::from_millis(600),
                redis::cmd("INFO").arg("server").query_async(&mut conn),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
            let version = extract_redis_version(&info_server);
            Some(DetectedRedisService {
                source: cand.source,
                label: cand.label,
                host: cand.host,
                port: cand.port,
                container_name: cand.container_name,
                container_image: cand.container_image,
                reachable: true,
                requires_auth: false,
                version,
                latency_ms: Some(latency_ms),
            })
        }
        Ok(Err(err)) if is_redis_auth_error(&err.to_string()) => Some(DetectedRedisService {
            source: cand.source,
            label: cand.label,
            host: cand.host,
            port: cand.port,
            container_name: cand.container_name,
            container_image: cand.container_image,
            reachable: true,
            requires_auth: true,
            version: None,
            latency_ms: Some(latency_ms),
        }),
        _ => cand.keep_if_unreachable.then_some(DetectedRedisService {
            source: cand.source,
            label: cand.label,
            host: cand.host,
            port: cand.port,
            container_name: cand.container_name,
            container_image: cand.container_image,
            reachable: false,
            requires_auth: false,
            version: None,
            latency_ms: None,
        }),
    }
}

fn is_redis_auth_error(msg: &str) -> bool {
    let upper = msg.to_ascii_uppercase();
    upper.contains("NOAUTH")
        || upper.contains("WRONGPASS")
        || upper.contains("AUTHENTICATION REQUIRED")
        || upper.contains("NOPERM")
}

fn extract_redis_version(info: &str) -> Option<String> {
    info.lines().find_map(|line| {
        line.trim()
            .strip_prefix("redis_version:")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

async fn probe_manticore_candidate(
    cand: ManticoreCandidate,
    http_client: Option<reqwest::Client>,
) -> Option<DetectedManticoreService> {
    let client = http_client?;
    let sql_url = format!("{}/sql", cand.url.trim_end_matches('/'));
    let started = Instant::now();

    let resp = match client
        .post(&sql_url)
        .form(&[("mode", "raw"), ("query", "SHOW TABLES")])
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => {
            return cand
                .keep_if_unreachable
                .then_some(DetectedManticoreService {
                    source: cand.source,
                    label: cand.label,
                    url: cand.url,
                    container_name: cand.container_name,
                    container_image: cand.container_image,
                    reachable: false,
                    requires_auth: false,
                    has_torrents_table: None,
                    latency_ms: None,
                });
        }
    };

    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(0);
    let status = resp.status();

    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Some(DetectedManticoreService {
            source: cand.source,
            label: cand.label,
            url: cand.url,
            container_name: cand.container_name,
            container_image: cand.container_image,
            reachable: true,
            requires_auth: true,
            has_torrents_table: None,
            latency_ms: Some(latency_ms),
        });
    }

    if status.is_success()
        && let Ok(text) = resp.text().await
        && let Some(has_torrents) = parse_manticore_show_tables(&text)
    {
        return Some(DetectedManticoreService {
            source: cand.source,
            label: cand.label,
            url: cand.url,
            container_name: cand.container_name,
            container_image: cand.container_image,
            reachable: true,
            requires_auth: false,
            has_torrents_table: Some(has_torrents),
            latency_ms: Some(latency_ms),
        });
    }

    cand.keep_if_unreachable
        .then_some(DetectedManticoreService {
            source: cand.source,
            label: cand.label,
            url: cand.url,
            container_name: cand.container_name,
            container_image: cand.container_image,
            reachable: false,
            requires_auth: false,
            has_torrents_table: None,
            latency_ms: None,
        })
}

/// 校验响应为有效 Manticore `/sql?mode=raw` 返回，并判断是否已存在 `torrents` 表。
fn parse_manticore_show_tables(body: &str) -> Option<bool> {
    let val: Value = serde_json::from_str(body).ok()?;
    let arr = val.as_array()?;
    let first = arr.first()?.as_object()?;
    // Manticore raw SQL 响应包含 total / error / columns / data
    if !first.contains_key("columns")
        && !first.contains_key("error")
        && !first.contains_key("total")
    {
        return None;
    }
    let mut has_torrents = false;
    if let Some(rows) = first.get("data").and_then(Value::as_array) {
        for row in rows {
            if let Some(obj) = row.as_object() {
                for v in obj.values() {
                    if v.as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case("torrents"))
                    {
                        has_torrents = true;
                    }
                }
            }
        }
    }
    Some(has_torrents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_inspect_for_redis_and_manticore() {
        let sample = r#"[
          {
            "Name": "/hameta-redis",
            "Config": { "Image": "redis:7-alpine" },
            "HostConfig": { "NetworkMode": "bridge" },
            "NetworkSettings": {
              "IPAddress": "172.17.0.2",
              "Ports": {
                "6379/tcp": [
                  { "HostIp": "0.0.0.0", "HostPort": "16379" },
                  { "HostIp": "::", "HostPort": "16379" }
                ]
              }
            }
          },
          {
            "Name": "/manticore-prod",
            "Config": { "Image": "manticoresearch/manticore:latest" },
            "HostConfig": { "NetworkMode": "bridge" },
            "NetworkSettings": {
              "Networks": { "app_net": { "IPAddress": "172.20.0.5" } },
              "Ports": {
                "9306/tcp": [{ "HostIp": "0.0.0.0", "HostPort": "9306" }],
                "9308/tcp": [{ "HostIp": "127.0.0.1", "HostPort": "19308" }]
              }
            }
          }
        ]"#;

        let (redis, manticore) = parse_docker_inspect_candidates(sample);
        assert_eq!(redis.len(), 1);
        assert_eq!(redis[0].host, "127.0.0.1");
        assert_eq!(redis[0].port, 16379);
        assert_eq!(redis[0].container_name.as_deref(), Some("hameta-redis"));

        assert_eq!(manticore.len(), 1);
        assert_eq!(manticore[0].url, "http://127.0.0.1:19308");
        assert_eq!(
            manticore[0].container_name.as_deref(),
            Some("manticore-prod")
        );
    }

    #[test]
    fn parses_env_urls_without_leaking_credentials() {
        assert_eq!(
            parse_redis_host_port("redis://:secret@10.0.0.2:6380/1"),
            Some(("10.0.0.2".to_string(), 6380))
        );
        assert_eq!(
            sanitize_http_base_url("http://user:pass@127.0.0.1:9308/"),
            Some("http://127.0.0.1:9308".to_string())
        );
    }

    #[test]
    fn parses_manticore_show_tables_response() {
        let with_table = r#"[{"columns":[{"Index":{"type":"string"}},{"Type":{"type":"string"}}],"data":[{"Index":"torrents","Type":"rt"}],"total":1,"error":""}]"#;
        assert_eq!(parse_manticore_show_tables(with_table), Some(true));

        let empty = r#"[{"columns":[],"data":[],"total":0,"error":""}]"#;
        assert_eq!(parse_manticore_show_tables(empty), Some(false));

        let non_manticore = r#"{"hello":"world"}"#;
        assert_eq!(parse_manticore_show_tables(non_manticore), None);
    }
}
