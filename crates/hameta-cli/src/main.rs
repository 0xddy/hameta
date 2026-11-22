#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use clap::{Parser, Subcommand};
use hameta_common::{
    DesiredState, ManticoreServiceConfig, RedisServiceConfig, ServiceKind, WorkerRole,
    hash_password, read_worker_bootstrap,
};
use hameta_panel::PanelConfig;
use hameta_storage::Database;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "hameta",
    version,
    about = "hameta DHT-Hash 采集、索引与 Web 运维面板"
)]
struct Cli {
    #[command(subcommand)]
    command: TopCommand,
}

#[derive(Debug, Subcommand)]
enum TopCommand {
    /// 启动 Web 主控面板与进程守护服务
    Panel {
        #[arg(long, env = "HAMETA_BIND", default_value = "0.0.0.0:8080")]
        bind: String,
        #[arg(long, env = "HAMETA_DATA_DIR", default_value = "./data")]
        data_dir: PathBuf,
        #[arg(long, default_value_t = false)]
        dev: bool,
    },
    /// 启动本地 Worker 子进程
    Worker {
        #[command(subcommand)]
        role: WorkerCommand,
    },
    /// 管理员离线/在线维护命令
    Admin {
        #[command(subcommand)]
        action: AdminCommand,
    },
}

#[derive(Debug, Subcommand)]
enum WorkerCommand {
    /// 启动 DHT 采集 Worker（默认从 stdin 读取 Panel 注入的 Bootstrap JSON，或指定 --standalone --config）
    Crawler {
        #[arg(long, default_value_t = false)]
        standalone: bool,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// 启动 Redis Streams 消费与 Manticore 入库 Worker（默认从 stdin 读取 Bootstrap JSON，或指定 --standalone --config）
    Indexer {
        #[arg(long, default_value_t = false)]
        standalone: bool,
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum AdminCommand {
    /// 重置（或初始化）管理员用户名与密码，并清空所有现存登录会话
    Reset {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
        #[arg(long, env = "HAMETA_DATA_DIR", default_value = "./data")]
        data_dir: PathBuf,
    },
    /// 查看当前实例的管理员、初始化与配置摘要信息
    Info {
        #[arg(long, env = "HAMETA_DATA_DIR", default_value = "./data")]
        data_dir: PathBuf,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

fn init_tracing(default_level: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

fn extract_worker_log_level<'a>(
    worker_config: &'a serde_json::Value,
    fallback: &'a str,
) -> &'a str {
    worker_config
        .get("log_level")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|lvl| {
            matches!(
                lvl.to_ascii_lowercase().as_str(),
                "error" | "warn" | "info" | "debug" | "trace"
            )
        })
        .unwrap_or(fallback)
}

#[derive(Debug, Clone, Serialize)]
pub struct AdminInfoReport {
    pub version: &'static str,
    pub data_dir: String,
    pub db_path: String,
    pub db_exists: bool,
    pub db_size_bytes: u64,
    pub initialized: bool,
    pub admin_username: Option<String>,
    pub admin_updated_at: Option<i64>,
    pub admin_last_login_at: Option<i64>,
    pub active_sessions: i64,
    pub redis_endpoint: Option<String>,
    pub manticore_url: Option<String>,
    pub published_filter_version: Option<i64>,
    pub worker_total: usize,
    pub worker_crawlers: usize,
    pub worker_indexers: usize,
    pub worker_desired_running: usize,
}

fn sanitize_service_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let Some(scheme_pos) = trimmed.find("://") else {
        return trimmed.to_string();
    };
    let after_scheme = &trimmed[scheme_pos + 3..];
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];
    let host_part = match authority.rfind('@') {
        Some(at_pos) => &authority[at_pos + 1..],
        None => authority,
    };
    format!(
        "{}://{}{}",
        &trimmed[..scheme_pos],
        host_part,
        &after_scheme[authority_end..]
    )
}

fn format_redis_endpoint(cfg: &RedisServiceConfig) -> String {
    if !cfg.url.trim().is_empty() {
        return sanitize_service_url(&cfg.url);
    }
    let scheme = if cfg.tls { "rediss" } else { "redis" };
    let host = cfg.host.trim();
    if cfg.db == 0 {
        format!("{scheme}://{host}:{}", cfg.port)
    } else {
        format!("{scheme}://{host}:{}/{}", cfg.port, cfg.db)
    }
}

fn format_timestamp(ts: Option<i64>) -> String {
    match ts {
        Some(secs) => match DateTime::from_timestamp(secs, 0) {
            Some(dt) => dt
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            None => secs.to_string(),
        },
        None => "从未".to_string(),
    }
}

pub async fn collect_admin_info(data_dir: &Path) -> Result<AdminInfoReport> {
    let db_path = data_dir.join("hameta.db");
    let data_dir_str = data_dir.display().to_string();
    let db_path_str = db_path.display().to_string();

    if !db_path.is_file() {
        return Ok(AdminInfoReport {
            version: env!("CARGO_PKG_VERSION"),
            data_dir: data_dir_str,
            db_path: db_path_str,
            db_exists: false,
            db_size_bytes: 0,
            initialized: false,
            admin_username: None,
            admin_updated_at: None,
            admin_last_login_at: None,
            active_sessions: 0,
            redis_endpoint: None,
            manticore_url: None,
            published_filter_version: None,
            worker_total: 0,
            worker_crawlers: 0,
            worker_indexers: 0,
            worker_desired_running: 0,
        });
    }

    let db_size_bytes = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
    let db = Database::open(&db_path)
        .await
        .with_context(|| format!("打开数据库失败: {}", db_path.display()))?;

    let initialized = db.is_initialized().await.context("查询初始化状态失败")?;
    let admin = db.get_primary_admin().await.context("查询管理员信息失败")?;
    let active_sessions = db
        .count_active_sessions()
        .await
        .context("统计有效登录会话失败")?;

    let redis_endpoint = match db
        .get_active_service_config(ServiceKind::Redis)
        .await
        .context("查询 Redis 服务配置失败")?
    {
        Some(row) => serde_json::from_str::<RedisServiceConfig>(&row.config_json)
            .ok()
            .map(|cfg| format_redis_endpoint(&cfg)),
        None => None,
    };

    let manticore_url = match db
        .get_active_service_config(ServiceKind::Manticore)
        .await
        .context("查询 Manticore 服务配置失败")?
    {
        Some(row) => serde_json::from_str::<ManticoreServiceConfig>(&row.config_json)
            .ok()
            .map(|cfg| sanitize_service_url(&cfg.url)),
        None => None,
    };

    let published_filter_version = db
        .get_published_filter_version()
        .await
        .context("查询已发布过滤规则版本失败")?
        .map(|row| row.version);

    let workers = db.list_workers().await.context("查询 Worker 列表失败")?;
    let worker_total = workers.len();
    let worker_crawlers = workers
        .iter()
        .filter(|w| w.role == WorkerRole::Crawler.as_str())
        .count();
    let worker_indexers = workers
        .iter()
        .filter(|w| w.role == WorkerRole::Indexer.as_str())
        .count();
    let worker_desired_running = workers
        .iter()
        .filter(|w| w.desired_state == DesiredState::Running.as_str())
        .count();

    Ok(AdminInfoReport {
        version: env!("CARGO_PKG_VERSION"),
        data_dir: data_dir_str,
        db_path: db_path_str,
        db_exists: true,
        db_size_bytes,
        initialized,
        admin_username: admin.as_ref().map(|a| a.username.clone()),
        admin_updated_at: admin.as_ref().map(|a| a.updated_at),
        admin_last_login_at: admin.and_then(|a| a.last_login_at),
        active_sessions,
        redis_endpoint,
        manticore_url,
        published_filter_version,
        worker_total,
        worker_crawlers,
        worker_indexers,
        worker_desired_running,
    })
}

pub async fn handle_admin_info(data_dir: &Path, json_output: bool) -> Result<()> {
    let report = collect_admin_info(data_dir).await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).context("序列化管理员信息失败")?
        );
        return Ok(());
    }

    println!("程序版本:       v{}", report.version);
    println!("数据目录:       {}", report.data_dir);
    if !report.db_exists {
        println!("状态数据库:     尚未生成 ({})", report.db_path);
        println!("初始化状态:     待初始化（首次启动面板后请在 Web 向导完成初始化）");
        return Ok(());
    }

    let size_kib = report.db_size_bytes / 1024;
    println!("状态数据库:     {} ({size_kib} KiB)", report.db_path);
    println!(
        "初始化状态:     {}",
        if report.initialized {
            "已完成初始化"
        } else {
            "未完成初始化（待 Web 向导配置）"
        }
    );
    println!(
        "管理员账号:     {}",
        report.admin_username.as_deref().unwrap_or("未创建")
    );
    println!(
        "密码更新时间:   {}",
        format_timestamp(report.admin_updated_at)
    );
    println!(
        "最近登录时间:   {}",
        format_timestamp(report.admin_last_login_at)
    );
    println!("有效登录会话:   {} 个", report.active_sessions);
    println!(
        "Redis 端点:     {}",
        report.redis_endpoint.as_deref().unwrap_or("未配置")
    );
    println!(
        "Manticore 地址: {}",
        report.manticore_url.as_deref().unwrap_or("未配置")
    );
    println!(
        "过滤规则版本:   {}",
        report
            .published_filter_version
            .map(|v| format!("v{v} (已发布)"))
            .unwrap_or_else(|| "未发布".to_string())
    );
    println!(
        "Worker 配置:    共 {} 个 (Crawler {} / Indexer {} / 期望运行 {})",
        report.worker_total,
        report.worker_crawlers,
        report.worker_indexers,
        report.worker_desired_running
    );
    Ok(())
}

pub async fn handle_admin_reset(data_dir: &Path, username: &str, password: &str) -> Result<()> {
    let username = username.trim();
    if username.is_empty() {
        bail!("管理员用户名不能为空");
    }
    if password.is_empty() {
        bail!("管理员密码不能为空");
    }

    let password_hash = hash_password(password).context("生成密码摘要失败")?;
    let db_path = data_dir.join("hameta.db");
    let db = Database::open(&db_path)
        .await
        .with_context(|| format!("打开数据库失败: {}", db_path.display()))?;

    let updated = db
        .reset_admin_credentials(username, &password_hash, "cli")
        .await
        .context("重置管理员账密失败")?;

    println!(
        "管理员账密已重置: username={} (所有现存登录会话已失效)",
        updated.username
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        TopCommand::Panel {
            bind,
            data_dir,
            dev,
        } => {
            init_tracing("info");
            hameta_panel::run_panel(PanelConfig {
                bind_addr: bind,
                data_dir,
                dev_mode: dev,
            })
            .await
        }
        TopCommand::Worker { role } => match role {
            WorkerCommand::Crawler { standalone, config } => {
                let bootstrap = tokio::task::spawn_blocking(move || {
                    read_worker_bootstrap(standalone, config.as_deref(), WorkerRole::Crawler)
                })
                .await
                .context("读取 Crawler Bootstrap 任务失败")??;
                let log_level = extract_worker_log_level(&bootstrap.worker_config, "warn");
                init_tracing(log_level);
                hameta_crawler::run_crawler_worker(bootstrap).await
            }
            WorkerCommand::Indexer { standalone, config } => {
                let bootstrap = tokio::task::spawn_blocking(move || {
                    read_worker_bootstrap(standalone, config.as_deref(), WorkerRole::Indexer)
                })
                .await
                .context("读取 Indexer Bootstrap 任务失败")??;
                let log_level = extract_worker_log_level(&bootstrap.worker_config, "info");
                init_tracing(log_level);
                hameta_indexer::run_indexer_worker(bootstrap).await
            }
        },
        TopCommand::Admin { action } => match action {
            AdminCommand::Reset {
                username,
                password,
                data_dir,
            } => {
                init_tracing("info");
                handle_admin_reset(&data_dir, &username, &password).await
            }
            AdminCommand::Info { data_dir, json } => handle_admin_info(&data_dir, json).await,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hameta_common::{generate_token, hash_token, verify_password};

    #[test]
    fn extracts_valid_worker_log_level_with_fallback() {
        assert_eq!(
            extract_worker_log_level(&serde_json::json!({"log_level": "debug"}), "warn"),
            "debug"
        );
        assert_eq!(
            extract_worker_log_level(&serde_json::json!({"log_level": "invalid"}), "warn"),
            "warn"
        );
        assert_eq!(
            extract_worker_log_level(&serde_json::json!({}), "info"),
            "info"
        );
    }

    #[tokio::test]
    async fn cli_admin_reset_updates_credentials_and_clears_sessions() {
        let dir = std::env::temp_dir().join(format!("hameta-cli-reset-{}", generate_token()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("hameta.db");

        {
            let db = Database::open(&db_path).await.unwrap();
            let old_hash = hash_password("old_secret").unwrap();
            let user = db.create_admin("admin_old", &old_hash).await.unwrap();
            db.create_session(&hash_token("tok1"), user.id, "127.0.0.1", "ua", 3600)
                .await
                .unwrap();
        }

        handle_admin_reset(&dir, "admin_new", "new_secret_123")
            .await
            .unwrap();

        {
            let db = Database::open(&db_path).await.unwrap();
            let primary = db.get_primary_admin().await.unwrap().unwrap();
            assert_eq!(primary.username, "admin_new");
            assert!(verify_password("new_secret_123", &primary.password_hash));
            assert!(!verify_password("old_secret", &primary.password_hash));
            assert!(
                db.find_valid_session(&hash_token("tok1"))
                    .await
                    .unwrap()
                    .is_none()
            );
            let audits = db.list_audits(5).await.unwrap();
            assert_eq!(audits[0].action, "admin.reset");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cli_admin_info_handles_missing_and_initialized_db() {
        let missing_dir =
            std::env::temp_dir().join(format!("hameta-cli-missing-{}", generate_token()));
        let missing = collect_admin_info(&missing_dir).await.unwrap();
        assert!(!missing.db_exists);
        assert!(!missing.initialized);
        assert!(!missing_dir.exists());

        let dir = std::env::temp_dir().join(format!("hameta-cli-info-{}", generate_token()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("hameta.db");

        {
            let db = Database::open(&db_path).await.unwrap();
            let hash = hash_password("secret_pass_123").unwrap();
            let admin = db.create_admin("root_ops", &hash).await.unwrap();
            db.create_session(&hash_token("active-tok"), admin.id, "127.0.0.1", "ua", 3600)
                .await
                .unwrap();

            let redis_cfg = RedisServiceConfig {
                url: "redis://default:supersecret@pass@10.0.0.5:6379/2".to_string(),
                ..Default::default()
            };
            db.save_service_config(
                ServiceKind::Redis,
                &serde_json::to_string(&redis_cfg).unwrap(),
                "root_ops",
                "init",
            )
            .await
            .unwrap();

            let manticore_cfg = ManticoreServiceConfig {
                url: "http://user:pw@10.0.0.6:9308".to_string(),
                ..Default::default()
            };
            db.save_service_config(
                ServiceKind::Manticore,
                &serde_json::to_string(&manticore_cfg).unwrap(),
                "root_ops",
                "init",
            )
            .await
            .unwrap();

            db.save_filter_draft("{}", "[]", "[]", "root_ops", "init")
                .await
                .unwrap();
            db.publish_filter_draft("root_ops", None).await.unwrap();

            db.create_worker("crawler-1", WorkerRole::Crawler, "C1", "{}", "root_ops")
                .await
                .unwrap();
            db.create_worker("indexer-1", WorkerRole::Indexer, "I1", "{}", "root_ops")
                .await
                .unwrap();
            db.set_worker_desired_state("crawler-1", DesiredState::Running)
                .await
                .unwrap();
            db.mark_initialized().await.unwrap();
        }

        let report = collect_admin_info(&dir).await.unwrap();
        assert!(report.db_exists);
        assert!(report.initialized);
        assert_eq!(report.admin_username.as_deref(), Some("root_ops"));
        assert_eq!(report.active_sessions, 1);
        assert_eq!(
            report.redis_endpoint.as_deref(),
            Some("redis://10.0.0.5:6379/2")
        );
        assert!(
            !report
                .redis_endpoint
                .as_deref()
                .unwrap()
                .contains("supersecret")
        );
        assert_eq!(
            report.manticore_url.as_deref(),
            Some("http://10.0.0.6:9308")
        );
        assert_eq!(report.published_filter_version, Some(1));
        assert_eq!(report.worker_total, 2);
        assert_eq!(report.worker_crawlers, 1);
        assert_eq!(report.worker_indexers, 1);
        assert_eq!(report.worker_desired_running, 1);

        handle_admin_info(&dir, false).await.unwrap();
        handle_admin_info(&dir, true).await.unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }
}
