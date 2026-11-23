use crate::config::{GeoIpConfig, ManticoreServiceConfig, RedisServiceConfig};
use crate::error::CommonError;
use crate::models::{FileInfo, WorkerRole};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;

/// 由 Panel 主进程通过子进程 `stdin` 一次性注入的引导数据包。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerBootstrap {
    pub instance_id: String,
    pub role: WorkerRole,
    pub panel_ipc_port: u16,
    pub ipc_token: String,
    pub data_dir: String,
    pub redis: RedisServiceConfig,
    pub manticore: Option<ManticoreServiceConfig>,
    pub worker_config: Value,
    pub filter_bundle: Option<Value>,
    #[serde(default)]
    pub geoip: Option<GeoIpConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct StandaloneBootstrapOverlay {
    instance_id: Option<String>,
    role: Option<WorkerRole>,
    panel_ipc_port: Option<u16>,
    ipc_token: Option<String>,
    data_dir: Option<String>,
    redis: RedisServiceConfig,
    manticore: Option<ManticoreServiceConfig>,
    worker_config: Option<Value>,
    filter_bundle: Option<Value>,
    #[serde(default)]
    geoip: Option<GeoIpConfig>,
}

/// 解析 `WorkerBootstrap` JSON 字符串；当 `allow_standalone_defaults` 为 `true` 时，允许省略部分独立运行非必需字段。
pub fn parse_worker_bootstrap_str(
    raw: &str,
    expected_role: WorkerRole,
    allow_standalone_defaults: bool,
) -> Result<WorkerBootstrap, CommonError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CommonError::InvalidConfig(
            "Worker Bootstrap JSON 不能为空".to_string(),
        ));
    }

    let bootstrap = if allow_standalone_defaults {
        let overlay: StandaloneBootstrapOverlay = serde_json::from_str(trimmed)
            .map_err(|e| CommonError::InvalidConfig(format!("解析独立启动配置 JSON 失败: {e}")))?;
        let role = overlay.role.unwrap_or(expected_role);
        WorkerBootstrap {
            instance_id: overlay
                .instance_id
                .unwrap_or_else(|| format!("standalone-{}", role.as_str())),
            role,
            panel_ipc_port: overlay.panel_ipc_port.unwrap_or(0),
            ipc_token: overlay.ipc_token.unwrap_or_default(),
            data_dir: overlay.data_dir.unwrap_or_else(|| "./data".to_string()),
            redis: overlay.redis,
            manticore: overlay.manticore,
            worker_config: overlay.worker_config.unwrap_or_else(|| json!({})),
            filter_bundle: overlay.filter_bundle,
            geoip: overlay.geoip,
        }
    } else {
        serde_json::from_str::<WorkerBootstrap>(trimmed)
            .map_err(|e| CommonError::InvalidConfig(format!("解析标准 Bootstrap JSON 失败: {e}")))?
    };

    if bootstrap.role != expected_role {
        return Err(CommonError::InvalidConfig(format!(
            "Worker 角色不匹配: 期望 {}, 实际 {}",
            expected_role.as_str(),
            bootstrap.role.as_str()
        )));
    }
    if bootstrap.instance_id.trim().is_empty() {
        return Err(CommonError::InvalidConfig(
            "Worker instance_id 不能为空".to_string(),
        ));
    }

    Ok(bootstrap)
}

/// 读取 Worker 引导配置：守护模式由 `stdin` 读取一次性 JSON，独立调试模式由 `--config` 文件读取。
pub fn read_worker_bootstrap(
    standalone: bool,
    config_path: Option<&Path>,
    expected_role: WorkerRole,
) -> Result<WorkerBootstrap, CommonError> {
    if standalone {
        let path = config_path.ok_or_else(|| {
            CommonError::InvalidConfig("--standalone 模式必须同时指定 --config <path>".to_string())
        })?;
        let raw = std::fs::read_to_string(path).map_err(|e| {
            CommonError::InvalidConfig(format!("读取配置文件 {} 失败: {e}", path.display()))
        })?;
        return parse_worker_bootstrap_str(&raw, expected_role, true);
    }

    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .map_err(|e| CommonError::InvalidConfig(format!("读取 stdin 引导参数失败: {e}")))?;
    parse_worker_bootstrap_str(&raw, expected_role, false)
}

/// 常量时间校验 HTTP `Authorization: Bearer <token>` 头，防止本地 IPC 未授权访问。
pub fn verify_bearer_token(auth_header: Option<&str>, expected_token: &str) -> bool {
    if expected_token.is_empty() {
        return false;
    }
    let Some(header) = auth_header else {
        return false;
    };
    let mut parts = header.trim().splitn(2, ' ');
    let scheme = parts.next().unwrap_or("");
    let token = parts.next().unwrap_or("").trim();
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return false;
    }
    constant_time_eq_bytes(token.as_bytes(), expected_token.as_bytes())
}

fn constant_time_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    let max_len = a.len().max(b.len());
    let mut diff = a.len() ^ b.len();
    for i in 0..max_len {
        let ba = a.get(i).copied().unwrap_or(0);
        let bb = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(ba ^ bb);
    }
    diff == 0
}

/// 查询当前进程常驻物理内存（RSS，字节）；非 Linux 平台在无额外系统调用下返回 `0`。
pub fn current_process_rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
            return 0;
        };
        let Some(resident_pages) = statm
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u64>().ok())
        else {
            return 0;
        };
        // SAFETY: `_SC_PAGESIZE` 为 POSIX 标准只读系统配置查询。
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let page_bytes = u64::try_from(page_size)
            .ok()
            .filter(|&v| v > 0)
            .unwrap_or(4096);
        resident_pages.saturating_mul(page_bytes)
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// 查询当前进程软文件描述符上限 (`RLIMIT_NOFILE`)，非 Unix 平台返回 `None`。
pub fn current_soft_open_file_limit() -> Option<usize> {
    #[cfg(unix)]
    {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `limit` 已初始化并可安全传给 POSIX `getrlimit`。
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
            return None;
        }
        if limit.rlim_cur == libc::RLIM_INFINITY {
            return None;
        }
        usize::try_from(limit.rlim_cur).ok()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRegisterRequest {
    pub instance_id: String,
    pub worker_port: u16,
    pub pid: u32,
    pub ipc_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerReadyRequest {
    pub instance_id: String,
    pub rule_version: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerShutdownRequest {
    pub graceful_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerDrainStatus {
    pub draining: bool,
    pub queued_items: usize,
    pub in_flight_items: usize,
    pub completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerReloadRulesRequest {
    pub version: i64,
    pub filter_bundle: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerReloadRulesResponse {
    pub active_rule_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerSnapshotEnvelope {
    pub instance_id: String,
    pub role: WorkerRole,
    pub captured_at_unix: u64,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerReloadGeoIpRequest {
    pub config: GeoIpConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerReloadGeoIpResponse {
    pub enabled: bool,
    pub mmdb_loaded: bool,
    pub mmdb_path: String,
    pub mmdb_size_bytes: u64,
    pub mmdb_build_epoch: u64,
    pub database_type: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestEventType {
    Indexed,
    HotUpdated,
}

mod serde_i64_as_string {
    use serde::{Deserialize, Deserializer, Serializer, de};

    pub fn serialize<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<i64, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum IntOrStr {
            Int(i64),
            Str(String),
        }

        match IntOrStr::deserialize(deserializer)? {
            IntOrStr::Int(v) => Ok(v),
            IntOrStr::Str(s) => s
                .trim()
                .parse::<i64>()
                .map_err(|_| de::Error::custom(format!("无效的 64 位整数 ID: {s}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestEventItem {
    pub info_hash: String,
    pub event_type: IngestEventType,
    pub worker_id: String,
    #[serde(with = "serde_i64_as_string")]
    pub manticore_id: i64,
    pub batch_id: u64,
    pub raw_name: String,
    pub refine_name: String,
    pub cleaned: bool,
    pub total_size: u64,
    pub file_count: i32,
    #[serde(default)]
    pub files_preview: Vec<FileInfo>,
    pub hot: i32,
    pub hot_delta: i32,
    pub latency_ms: u64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerIngestEventsRequest {
    pub instance_id: String,
    pub events: Vec<IngestEventItem>,
}
