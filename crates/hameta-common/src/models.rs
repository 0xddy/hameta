use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentMessage {
    pub info_hash: String,
    pub name: String,
    pub total_size: u64,
    pub file_count: usize,
    pub files: Vec<FileInfo>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentRecord {
    pub info_hash: String,
    pub raw_name: String,
    pub refine_name: Option<String>,
    pub file_count: i32,
    pub file_info: Vec<FileInfo>,
    pub hot: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    Crawler,
    Indexer,
}

impl WorkerRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crawler => "crawler",
            Self::Indexer => "indexer",
        }
    }
}

impl std::str::FromStr for WorkerRole {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "crawler" => Ok(Self::Crawler),
            "indexer" => Ok(Self::Indexer),
            _ => Err(format!("未知的 Worker 角色: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesiredState {
    Stopped,
    Running,
}

impl DesiredState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Running => "running",
        }
    }
}

impl std::str::FromStr for DesiredState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stopped" => Ok(Self::Stopped),
            "running" => Ok(Self::Running),
            _ => Err(format!("未知的期望状态: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRuntimeState {
    Stopped,
    Starting,
    Running,
    Draining,
    Crashed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    Redis,
    Manticore,
}

impl ServiceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Redis => "redis",
            Self::Manticore => "manticore",
        }
    }
}

impl std::str::FromStr for ServiceKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "redis" => Ok(Self::Redis),
            "manticore" => Ok(Self::Manticore),
            _ => Err(format!("未知的服务类型: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterVersionStatus {
    Draft,
    Published,
    Archived,
}

impl FilterVersionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsMinutePoint {
    pub ts_minute: i64,
    pub hashes_per_min: f64,
    pub metadata_succeeded_per_min: f64,
    pub stream_pushed_per_min: f64,
    pub filtered_per_min: f64,
    pub indexed_written_per_min: f64,
    pub hot_updated_per_min: f64,
    pub actual_qps: f64,
    pub stream_new_len: i64,
    pub stream_hot_len: i64,
    pub stream_new_lag: i64,
    pub udp_rx_bps: f64,
    pub udp_tx_bps: f64,
    pub metadata_rx_bps: f64,
    pub total_rss_bytes: i64,
    pub avg_cpu_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsHourPoint {
    pub ts_hour: i64,
    pub hashes_total: i64,
    pub metadata_succeeded_total: i64,
    pub stream_pushed_total: i64,
    pub filtered_total: i64,
    pub indexed_written_total: i64,
    pub hot_updated_total: i64,
    pub avg_qps: f64,
    pub max_stream_new_lag: i64,
    pub avg_rss_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: i64,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail_json: String,
    pub client_ip: String,
    pub created_at: i64,
}
