use crate::supervisor::Supervisor;
use hameta_common::WorkerRole;
use hameta_storage::Database;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const META_NOFILE_CONFIG_KEY: &str = "nofile_config";

pub const MIN_PROCESS_NOFILE: u64 = 1_024;
pub const MAX_PROCESS_NOFILE: u64 = 10_485_760;
pub const MIN_SYSTEM_FILE_MAX: u64 = 65_536;
pub const MAX_SYSTEM_FILE_MAX: u64 = 100_000_000;
pub const MIN_SYSTEM_NR_OPEN: u64 = 65_536;
pub const MAX_SYSTEM_NR_OPEN: u64 = 10_485_760;

pub const SYSCTL_CONF_PATH: &str = "/etc/sysctl.d/99-hameta-nofile.conf";
pub const LIMITS_CONF_PATH: &str = "/etc/security/limits.d/99-hameta-nofile.conf";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedNofileConfig {
    pub process_soft: u64,
    pub process_hard: u64,
    #[serde(default)]
    pub system_file_max: Option<u64>,
    #[serde(default)]
    pub system_nr_open: Option<u64>,
    pub updated_at: i64,
    pub updated_by: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NofilePresetItem {
    pub id: &'static str,
    pub label: &'static str,
    pub badge: &'static str,
    pub process_soft: u64,
    pub process_hard: u64,
    pub system_file_max: u64,
    pub system_nr_open: u64,
    pub description: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkerNofileInfo {
    pub id: String,
    pub display_name: String,
    pub role: WorkerRole,
    pub pid: u32,
    pub soft_limit: Option<u64>,
    pub hard_limit: Option<u64>,
    pub open_fds: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NofileStatusResponse {
    pub platform: &'static str,
    pub supported: bool,
    pub system_supported: bool,
    pub is_root: bool,
    pub in_container: bool,
    pub can_write_sysctl: bool,
    pub can_write_limits_d: bool,
    pub panel_pid: u32,
    pub process_soft: Option<u64>,
    pub process_hard: Option<u64>,
    pub process_open_fds: Option<u64>,
    pub system_file_max: Option<u64>,
    pub system_nr_open: Option<u64>,
    pub system_allocated_fds: Option<u64>,
    pub sysctl_conf_path: Option<String>,
    pub limits_conf_path: Option<String>,
    pub sysctl_conf_exists: bool,
    pub limits_conf_exists: bool,
    pub saved_config: Option<SavedNofileConfig>,
    pub running_workers: Vec<WorkerNofileInfo>,
    pub recommended_presets: Vec<NofilePresetItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateNofileRequest {
    #[serde(default = "default_true")]
    pub apply_process: bool,
    pub process_soft: Option<u64>,
    pub process_hard: Option<u64>,
    #[serde(default = "default_true")]
    pub apply_to_workers: bool,
    #[serde(default = "default_true")]
    pub persist_app_config: bool,
    #[serde(default)]
    pub apply_system: bool,
    pub system_file_max: Option<u64>,
    pub system_nr_open: Option<u64>,
    #[serde(default)]
    pub persist_system_files: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkerNofileApplyResult {
    pub id: String,
    pub role: WorkerRole,
    pub pid: u32,
    pub ok: bool,
    pub soft_limit: Option<u64>,
    pub hard_limit: Option<u64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateNofileResponse {
    pub ok: bool,
    pub process_applied: bool,
    pub process_message: String,
    pub system_applied: bool,
    pub system_message: String,
    pub persisted_app_config: bool,
    pub persisted_files: Vec<String>,
    pub warnings: Vec<String>,
    pub manual_commands: Vec<String>,
    pub worker_results: Vec<WorkerNofileApplyResult>,
    pub status: NofileStatusResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedNofileUpdate {
    pub process: Option<(u64, u64)>,
    pub system_file_max: Option<u64>,
    pub system_nr_open: Option<u64>,
}

pub fn recommended_nofile_presets() -> Vec<NofilePresetItem> {
    vec![
        NofilePresetItem {
            id: "standard",
            label: "65,536",
            badge: "标准 P0/P1",
            process_soft: 65_536,
            process_hard: 65_536,
            system_file_max: 524_288,
            system_nr_open: 1_048_576,
            description: "日常轻量采集，低内存开销",
        },
        NofilePresetItem {
            id: "high",
            label: "262,144",
            badge: "推荐 P2/P3",
            process_soft: 262_144,
            process_hard: 262_144,
            system_file_max: 1_048_576,
            system_nr_open: 1_048_576,
            description: "高并发元数据拉取推荐基线",
        },
        NofilePresetItem {
            id: "extreme",
            label: "524,288",
            badge: "极致 P4",
            process_soft: 524_288,
            process_hard: 524_288,
            system_file_max: 2_097_152,
            system_nr_open: 1_048_576,
            description: "多爬虫实例高并发防 EMFILE",
        },
        NofilePresetItem {
            id: "max",
            label: "1,048,576",
            badge: "超大规模 P5",
            process_soft: 1_048_576,
            process_hard: 1_048_576,
            system_file_max: 4_194_304,
            system_nr_open: 2_097_152,
            description: "单机满载多进程极致吞吐",
        },
    ]
}

pub fn validate_update_request(
    req: &UpdateNofileRequest,
    current_hard: Option<u64>,
) -> Result<ValidatedNofileUpdate, String> {
    if !req.apply_process && !req.apply_system {
        return Err("请至少勾选「程序级 File Limit」或「系统级内核参数」中的一项".to_string());
    }

    let process = if req.apply_process {
        let soft = req
            .process_soft
            .ok_or_else(|| "启用程序级修改时必须填写软上限 (process_soft)".to_string())?;
        let hard = req
            .process_hard
            .unwrap_or_else(|| current_hard.map_or(soft, |cur| cur.max(soft)));
        if !(MIN_PROCESS_NOFILE..=MAX_PROCESS_NOFILE).contains(&soft) {
            return Err(format!(
                "程序软上限需在 {MIN_PROCESS_NOFILE} ~ {MAX_PROCESS_NOFILE} 之间"
            ));
        }
        if !(MIN_PROCESS_NOFILE..=MAX_PROCESS_NOFILE).contains(&hard) {
            return Err(format!(
                "程序硬上限需在 {MIN_PROCESS_NOFILE} ~ {MAX_PROCESS_NOFILE} 之间"
            ));
        }
        if soft > hard {
            return Err(format!("程序软上限 ({soft}) 不能大于硬上限 ({hard})"));
        }
        Some((soft, hard))
    } else {
        None
    };

    let (system_file_max, system_nr_open) = if req.apply_system {
        if req.system_file_max.is_none() && req.system_nr_open.is_none() {
            return Err("启用系统级修改时，fs.file-max 与 fs.nr_open 至少填写一项".to_string());
        }
        if let Some(fmax) = req.system_file_max
            && !(MIN_SYSTEM_FILE_MAX..=MAX_SYSTEM_FILE_MAX).contains(&fmax)
        {
            return Err(format!(
                "系统 fs.file-max 需在 {MIN_SYSTEM_FILE_MAX} ~ {MAX_SYSTEM_FILE_MAX} 之间"
            ));
        }
        if let Some(nr) = req.system_nr_open
            && !(MIN_SYSTEM_NR_OPEN..=MAX_SYSTEM_NR_OPEN).contains(&nr)
        {
            return Err(format!(
                "系统 fs.nr_open 需在 {MIN_SYSTEM_NR_OPEN} ~ {MAX_SYSTEM_NR_OPEN} 之间"
            ));
        }
        if let (Some(fmax), Some(nr)) = (req.system_file_max, req.system_nr_open)
            && fmax < nr
        {
            return Err(format!(
                "系统 fs.file-max ({fmax}) 不能小于单进程顶限 fs.nr_open ({nr})"
            ));
        }
        if let (Some((_, hard)), Some(nr)) = (process, req.system_nr_open)
            && nr < hard
        {
            return Err(format!(
                "系统 fs.nr_open ({nr}) 不能小于程序硬上限 ({hard})，否则 setrlimit 会被内核拒绝"
            ));
        }
        (req.system_file_max, req.system_nr_open)
    } else {
        (None, None)
    };

    Ok(ValidatedNofileUpdate {
        process,
        system_file_max,
        system_nr_open,
    })
}

pub async fn load_saved_nofile_config(db: &Database) -> Option<SavedNofileConfig> {
    let raw = db.get_meta(META_NOFILE_CONFIG_KEY).await.ok()??;
    serde_json::from_str::<SavedNofileConfig>(&raw).ok()
}

/// 面板启动早期自动应用 SQLite 中已保存的 NOFILE 配置（必须在拉起 Worker 之前执行，便于子进程继承）。
pub async fn apply_saved_nofile_on_startup(db: &Database) {
    let Some(saved) = load_saved_nofile_config(db).await else {
        return;
    };
    if saved.system_file_max.is_some() || saved.system_nr_open.is_some() {
        let (ok, msg) = apply_system_sysctl_live(saved.system_file_max, saved.system_nr_open);
        if ok {
            tracing::info!("启动自动应用已保存系统 NOFILE 参数: {msg}");
        } else {
            tracing::debug!("启动跳过或未能修改系统 NOFILE 参数: {msg}");
        }
    }
    let outcome = apply_self_process_nofile(saved.process_soft, saved.process_hard);
    if outcome.applied {
        tracing::info!("启动自动应用已保存程序 RLIMIT_NOFILE: {}", outcome.message);
    } else {
        tracing::warn!(
            "启动自动应用已保存程序 RLIMIT_NOFILE ({}/{}) 未完全生效: {}",
            saved.process_soft,
            saved.process_hard,
            outcome.message
        );
    }
}

pub async fn collect_nofile_status(db: &Database, supervisor: &Supervisor) -> NofileStatusResponse {
    let (process_soft, process_hard) = read_self_process_nofile();
    let process_open_fds = read_pid_open_fd_count(std::process::id());
    let system_file_max = read_linux_proc_u64("/proc/sys/fs/file-max");
    let system_nr_open = read_linux_proc_u64("/proc/sys/fs/nr_open");
    let system_allocated_fds = read_linux_file_nr_allocated();
    let saved_config = load_saved_nofile_config(db).await;

    let mut running_workers = Vec::new();
    if let Ok(statuses) = supervisor.list_worker_statuses().await {
        for w in statuses {
            if let Some(pid) = w.pid {
                let (soft_limit, hard_limit) = read_pid_nofile(pid);
                let open_fds = read_pid_open_fd_count(pid);
                running_workers.push(WorkerNofileInfo {
                    id: w.id,
                    display_name: w.display_name,
                    role: w.role,
                    pid,
                    soft_limit,
                    hard_limit,
                    open_fds,
                });
            }
        }
    }

    let is_linux = cfg!(target_os = "linux");
    let is_unix = cfg!(unix);

    NofileStatusResponse {
        platform: std::env::consts::OS,
        supported: is_unix,
        system_supported: is_linux,
        is_root: is_current_process_root(),
        in_container: is_running_in_container(),
        can_write_sysctl: is_linux && can_open_for_write("/proc/sys/fs/file-max"),
        can_write_limits_d: is_linux && can_write_dir_or_file(LIMITS_CONF_PATH),
        panel_pid: std::process::id(),
        process_soft,
        process_hard,
        process_open_fds,
        system_file_max,
        system_nr_open,
        system_allocated_fds,
        sysctl_conf_path: is_linux.then(|| SYSCTL_CONF_PATH.to_string()),
        limits_conf_path: is_linux.then(|| LIMITS_CONF_PATH.to_string()),
        sysctl_conf_exists: is_linux && Path::new(SYSCTL_CONF_PATH).exists(),
        limits_conf_exists: is_linux && Path::new(LIMITS_CONF_PATH).exists(),
        saved_config,
        running_workers,
        recommended_presets: recommended_nofile_presets(),
    }
}

pub async fn execute_nofile_update(
    db: &Database,
    supervisor: &Supervisor,
    req: &UpdateNofileRequest,
    operator: &str,
) -> Result<UpdateNofileResponse, String> {
    let (_, current_hard) = read_self_process_nofile();
    let validated = validate_update_request(req, current_hard)?;

    let mut warnings: Vec<String> = Vec::new();
    let mut persisted_files: Vec<String> = Vec::new();
    let mut manual_commands: Vec<String> = Vec::new();
    let in_container = is_running_in_container();

    // 1) 先修改系统内核参数（fs.file-max -> fs.nr_open），这样紧随其后的 setrlimit/prlimit 才能突破原 nr_open 顶限
    let (system_applied, system_message) = if req.apply_system {
        let (ok, msg) =
            apply_system_sysctl_live(validated.system_file_max, validated.system_nr_open);
        if !ok {
            warnings.push(msg.clone());
            manual_commands.extend(build_system_manual_commands(
                validated.system_file_max,
                validated.system_nr_open,
                in_container,
            ));
        }
        (ok, msg)
    } else {
        (false, "本次未勾选系统级内核参数修改".to_string())
    };

    // 2) 修改面板进程 RLIMIT_NOFILE
    let (process_applied, process_message) = if let Some((soft, hard)) = validated.process {
        let outcome = apply_self_process_nofile(soft, hard);
        if !outcome.applied || outcome.partial {
            warnings.push(outcome.message.clone());
            manual_commands.extend(build_process_manual_commands(soft, hard, in_container));
        }
        (outcome.applied, outcome.message)
    } else {
        (false, "本次未勾选程序级 RLIMIT_NOFILE 修改".to_string())
    };

    // 3) 同步通过 Linux prlimit 热更新所有运行中的 Worker 子进程
    let mut worker_results = Vec::new();
    if let Some((soft, hard)) = validated.process
        && req.apply_to_workers
        && let Ok(statuses) = supervisor.list_worker_statuses().await
    {
        for w in statuses {
            let Some(pid) = w.pid else {
                continue;
            };
            let res = apply_pid_nofile(pid, soft, hard);
            let (new_soft, new_hard) = read_pid_nofile(pid);
            if !res.applied {
                warnings.push(format!("Worker {} (PID {pid}): {}", w.id, res.message));
            }
            worker_results.push(WorkerNofileApplyResult {
                id: w.id,
                role: w.role,
                pid,
                ok: res.applied && !res.partial,
                soft_limit: new_soft,
                hard_limit: new_hard,
                message: res.message,
            });
        }
    }

    // 4) 可选写入 Linux 系统持久化配置文件
    if req.persist_system_files {
        if cfg!(target_os = "linux") {
            if req.apply_system
                && (validated.system_file_max.is_some() || validated.system_nr_open.is_some())
            {
                let content =
                    render_sysctl_conf(validated.system_file_max, validated.system_nr_open);
                match write_system_config_file(SYSCTL_CONF_PATH, &content) {
                    Ok(()) => persisted_files.push(SYSCTL_CONF_PATH.to_string()),
                    Err(err) => warnings.push(format!("写入 {SYSCTL_CONF_PATH} 失败: {err}")),
                }
            }
            if let Some((soft, hard)) = validated.process {
                let content = render_limits_conf(soft, hard);
                match write_system_config_file(LIMITS_CONF_PATH, &content) {
                    Ok(()) => persisted_files.push(LIMITS_CONF_PATH.to_string()),
                    Err(err) => warnings.push(format!("写入 {LIMITS_CONF_PATH} 失败: {err}")),
                }
            }
        } else {
            warnings
                .push("非 Linux 平台跳过写入 /etc/sysctl.d 与 /etc/security/limits.d".to_string());
        }
    }

    // 5) 持久化程序级期望值到 SQLite（下次 Panel 启动自动应用）
    let mut persisted_app_config = false;
    if req.persist_app_config {
        let existing = load_saved_nofile_config(db).await;
        let (cur_soft, cur_hard) = read_self_process_nofile();
        let (save_soft, save_hard) = validated.process.unwrap_or_else(|| {
            existing
                .as_ref()
                .map(|c| (c.process_soft, c.process_hard))
                .unwrap_or((cur_soft.unwrap_or(65_536), cur_hard.unwrap_or(65_536)))
        });
        let saved = SavedNofileConfig {
            process_soft: save_soft,
            process_hard: save_hard,
            system_file_max: if req.apply_system {
                validated.system_file_max
            } else {
                existing.as_ref().and_then(|c| c.system_file_max)
            },
            system_nr_open: if req.apply_system {
                validated.system_nr_open
            } else {
                existing.as_ref().and_then(|c| c.system_nr_open)
            },
            updated_at: chrono::Utc::now().timestamp(),
            updated_by: operator.to_string(),
        };
        if let Ok(json_str) = serde_json::to_string(&saved) {
            if let Err(err) = db.set_meta(META_NOFILE_CONFIG_KEY, &json_str).await {
                warnings.push(format!("保存到面板数据库失败: {err}"));
            } else {
                persisted_app_config = true;
            }
        }
    }

    // 去重 manual_commands
    let mut dedup_cmds = Vec::new();
    for cmd in manual_commands {
        if !dedup_cmds.contains(&cmd) {
            dedup_cmds.push(cmd);
        }
    }

    let status = collect_nofile_status(db, supervisor).await;
    let overall_ok =
        (!req.apply_process || process_applied) && (!req.apply_system || system_applied);

    Ok(UpdateNofileResponse {
        ok: overall_ok,
        process_applied,
        process_message,
        system_applied,
        system_message,
        persisted_app_config,
        persisted_files,
        warnings,
        manual_commands: dedup_cmds,
        worker_results,
        status,
    })
}

pub fn render_sysctl_conf(file_max: Option<u64>, nr_open: Option<u64>) -> String {
    let mut lines = vec!["# Managed by hameta panel (NOFILE system limits)".to_string()];
    if let Some(v) = file_max {
        lines.push(format!("fs.file-max = {v}"));
    }
    if let Some(v) = nr_open {
        lines.push(format!("fs.nr_open = {v}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

pub fn render_limits_conf(soft: u64, hard: u64) -> String {
    format!(
        "# Managed by hameta panel (NOFILE process limits)\n\
         * soft nofile {soft}\n\
         * hard nofile {hard}\n\
         root soft nofile {soft}\n\
         root hard nofile {hard}\n"
    )
}

fn build_system_manual_commands(
    file_max: Option<u64>,
    nr_open: Option<u64>,
    in_container: bool,
) -> Vec<String> {
    let mut cmds = Vec::new();
    if let Some(v) = file_max {
        cmds.push(format!("sudo sysctl -w fs.file-max={v}"));
    }
    if let Some(v) = nr_open {
        cmds.push(format!("sudo sysctl -w fs.nr_open={v}"));
    }
    if in_container {
        cmds.push(
            "# Docker 容器内 /proc/sys 通常只读，请在物理主机执行上述 sysctl 命令，并在 docker-compose.yml 配置 ulimits.nofile"
                .to_string(),
        );
    }
    cmds
}

fn build_process_manual_commands(soft: u64, hard: u64, in_container: bool) -> Vec<String> {
    let mut cmds = Vec::new();
    if in_container {
        cmds.push(format!(
            "# docker-compose.yml:\nulimits:\n  nofile:\n    soft: {soft}\n    hard: {hard}"
        ));
    } else {
        cmds.push(format!(
            "# systemd unit ([Service] 段):\nLimitNOFILE={soft}:{hard}"
        ));
        cmds.push(format!("ulimit -n {soft}"));
    }
    cmds
}

struct ProcessLimitOutcome {
    applied: bool,
    partial: bool,
    message: String,
}

#[cfg(unix)]
fn read_self_process_nofile() -> (Option<u64>, Option<u64>) {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` 已初始化并可安全传递给 POSIX `getrlimit`。
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return (None, None);
    }
    let soft = rlim_to_option_u64(limit.rlim_cur);
    let hard = rlim_to_option_u64(limit.rlim_max);
    (soft, hard)
}

#[cfg(not(unix))]
fn read_self_process_nofile() -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[cfg(unix)]
#[allow(clippy::useless_conversion)]
fn rlim_to_option_u64(val: libc::rlim_t) -> Option<u64> {
    if val == libc::RLIM_INFINITY {
        None
    } else {
        u64::try_from(val).ok()
    }
}

#[cfg(unix)]
#[allow(clippy::useless_conversion)]
fn u64_to_rlim(val: u64) -> Option<libc::rlim_t> {
    libc::rlim_t::try_from(val).ok()
}

#[cfg(unix)]
fn apply_self_process_nofile(soft: u64, hard: u64) -> ProcessLimitOutcome {
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `current` 已初始化。
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut current) } != 0 {
        let err = std::io::Error::last_os_error();
        return ProcessLimitOutcome {
            applied: false,
            partial: false,
            message: format!("读取当前 RLIMIT_NOFILE 失败: {err}"),
        };
    }

    let prev_cur = rlim_to_option_u64(current.rlim_cur).unwrap_or(0);
    let prev_max = rlim_to_option_u64(current.rlim_max).unwrap_or(u64::MAX);

    let Some(soft_rlim) = u64_to_rlim(soft) else {
        return ProcessLimitOutcome {
            applied: false,
            partial: false,
            message: "软上限超出平台 rlim_t 范围".to_string(),
        };
    };
    let Some(hard_rlim) = u64_to_rlim(hard) else {
        return ProcessLimitOutcome {
            applied: false,
            partial: false,
            message: "硬上限超出平台 rlim_t 范围".to_string(),
        };
    };

    let desired = libc::rlimit {
        rlim_cur: soft_rlim,
        rlim_max: hard_rlim,
    };
    // SAFETY: 校验已保证 `soft <= hard`。
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &desired) } == 0 {
        return ProcessLimitOutcome {
            applied: true,
            partial: false,
            message: format!("已更新程序 RLIMIT_NOFILE: {prev_cur}/{prev_max} -> {soft}/{hard}"),
        };
    }

    let full_err = std::io::Error::last_os_error();
    // 尝试退化：若仅是无法突破当前硬上限，在当前硬上限内尽可能调整软上限
    let capped_soft = soft.min(prev_max);
    if capped_soft != prev_cur
        && let Some(capped_rlim) = u64_to_rlim(capped_soft)
    {
        let fallback = libc::rlimit {
            rlim_cur: capped_rlim,
            rlim_max: current.rlim_max,
        };
        // SAFETY: `capped_rlim <= current.rlim_max`。
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &fallback) } == 0 {
            return ProcessLimitOutcome {
                applied: true,
                partial: true,
                message: format!(
                    "硬上限提升被拒绝 ({full_err})，已在当前硬上限 ({prev_max}) 内将软上限调整为 {capped_soft}"
                ),
            };
        }
    }

    ProcessLimitOutcome {
        applied: false,
        partial: false,
        message: format!(
            "修改程序 RLIMIT_NOFILE ({prev_cur}/{prev_max} -> {soft}/{hard}) 失败: {full_err}（提升硬上限需要 root / CAP_SYS_RESOURCE 或更高的 systemd/Docker LimitNOFILE）"
        ),
    }
}

#[cfg(not(unix))]
fn apply_self_process_nofile(_soft: u64, _hard: u64) -> ProcessLimitOutcome {
    ProcessLimitOutcome {
        applied: false,
        partial: false,
        message: format!(
            "当前运行平台 ({}) 不支持 POSIX RLIMIT_NOFILE，已仅记录配置",
            std::env::consts::OS
        ),
    }
}

#[cfg(target_os = "linux")]
fn apply_pid_nofile(pid: u32, soft: u64, hard: u64) -> ProcessLimitOutcome {
    let Ok(raw_pid) = libc::pid_t::try_from(pid) else {
        return ProcessLimitOutcome {
            applied: false,
            partial: false,
            message: "无效 PID".to_string(),
        };
    };
    let (Some(soft_rlim), Some(hard_rlim)) = (u64_to_rlim(soft), u64_to_rlim(hard)) else {
        return ProcessLimitOutcome {
            applied: false,
            partial: false,
            message: "目标值超出 rlim_t 范围".to_string(),
        };
    };
    let desired = libc::rlimit {
        rlim_cur: soft_rlim,
        rlim_max: hard_rlim,
    };
    // SAFETY: `desired` 有效且 `soft <= hard`，`old_limit` 传空指针符合 Linux `prlimit(2)` 约定。
    if unsafe { libc::prlimit(raw_pid, libc::RLIMIT_NOFILE, &desired, std::ptr::null_mut()) } == 0 {
        return ProcessLimitOutcome {
            applied: true,
            partial: false,
            message: format!("已热更新至 {soft}/{hard}"),
        };
    }

    let full_err = std::io::Error::last_os_error();
    let (_, cur_hard) = read_pid_nofile(pid);
    if let Some(h) = cur_hard {
        let capped_soft = soft.min(h);
        if let (Some(cs), Some(ch)) = (u64_to_rlim(capped_soft), u64_to_rlim(h)) {
            let fallback = libc::rlimit {
                rlim_cur: cs,
                rlim_max: ch,
            };
            // SAFETY: `cs <= ch`。
            if unsafe {
                libc::prlimit(
                    raw_pid,
                    libc::RLIMIT_NOFILE,
                    &fallback,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return ProcessLimitOutcome {
                    applied: true,
                    partial: true,
                    message: format!("硬上限受限 ({full_err})，软上限已热更新至 {capped_soft}/{h}"),
                };
            }
        }
    }

    ProcessLimitOutcome {
        applied: false,
        partial: false,
        message: format!("prlimit 失败: {full_err}"),
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_pid_nofile(_pid: u32, _soft: u64, _hard: u64) -> ProcessLimitOutcome {
    ProcessLimitOutcome {
        applied: false,
        partial: false,
        message: "当前平台不支持对运行中子进程执行 prlimit，重启 Worker 后生效".to_string(),
    }
}

#[cfg(target_os = "linux")]
fn apply_system_sysctl_live(file_max: Option<u64>, nr_open: Option<u64>) -> (bool, String) {
    let mut applied_parts = Vec::new();
    let mut errors = Vec::new();

    if let Some(v) = file_max {
        match std::fs::write("/proc/sys/fs/file-max", format!("{v}\n")) {
            Ok(()) => applied_parts.push(format!("fs.file-max={v}")),
            Err(err) => errors.push(format!("fs.file-max 写入失败: {err}")),
        }
    }
    if let Some(v) = nr_open {
        match std::fs::write("/proc/sys/fs/nr_open", format!("{v}\n")) {
            Ok(()) => applied_parts.push(format!("fs.nr_open={v}")),
            Err(err) => errors.push(format!("fs.nr_open 写入失败: {err}")),
        }
    }

    if errors.is_empty() {
        (
            !applied_parts.is_empty(),
            format!("系统内核参数已生效: {}", applied_parts.join(", ")),
        )
    } else if applied_parts.is_empty() {
        (
            false,
            format!(
                "修改系统内核参数失败（{}）；请确认以 root 运行且非只读容器 /proc/sys",
                errors.join("；")
            ),
        )
    } else {
        (
            false,
            format!(
                "部分生效（{}），失败项：{}",
                applied_parts.join(", "),
                errors.join("；")
            ),
        )
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_system_sysctl_live(_file_max: Option<u64>, _nr_open: Option<u64>) -> (bool, String) {
    (
        false,
        format!(
            "当前平台 ({}) 不支持通过 /proc/sys 修改 Linux 系统内核参数",
            std::env::consts::OS
        ),
    )
}

#[cfg(target_os = "linux")]
fn write_system_config_file(path: &str, content: &str) -> Result<(), String> {
    let target = Path::new(path);
    if let Some(parent) = target.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(target, content).map_err(|e| e.to_string())
}

#[cfg(not(target_os = "linux"))]
fn write_system_config_file(_path: &str, _content: &str) -> Result<(), String> {
    Err("非 Linux 平台不支持写入系统配置文件".to_string())
}

#[cfg(target_os = "linux")]
fn read_linux_proc_u64(path: &str) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn read_linux_proc_u64(_path: &str) -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn read_linux_file_nr_allocated() -> Option<u64> {
    let raw = std::fs::read_to_string("/proc/sys/fs/file-nr").ok()?;
    parse_file_nr_allocated(&raw)
}

#[cfg(not(target_os = "linux"))]
fn read_linux_file_nr_allocated() -> Option<u64> {
    None
}

pub fn parse_file_nr_allocated(raw: &str) -> Option<u64> {
    raw.split_whitespace().next()?.parse::<u64>().ok()
}

pub fn parse_proc_limits_nofile(raw: &str) -> (Option<u64>, Option<u64>) {
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("Max open files") {
            let mut parts = rest.split_whitespace();
            let soft = parts.next().and_then(parse_limit_token);
            let hard = parts.next().and_then(parse_limit_token);
            return (soft, hard);
        }
    }
    (None, None)
}

fn parse_limit_token(tok: &str) -> Option<u64> {
    if tok.eq_ignore_ascii_case("unlimited") {
        None
    } else {
        tok.parse::<u64>().ok()
    }
}

#[cfg(target_os = "linux")]
fn read_pid_nofile(pid: u32) -> (Option<u64>, Option<u64>) {
    let Ok(raw) = std::fs::read_to_string(format!("/proc/{pid}/limits")) else {
        return (None, None);
    };
    parse_proc_limits_nofile(&raw)
}

#[cfg(not(target_os = "linux"))]
fn read_pid_nofile(_pid: u32) -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[cfg(target_os = "linux")]
fn read_pid_open_fd_count(pid: u32) -> Option<u64> {
    let entries = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    let count = entries.filter_map(Result::ok).count();
    u64::try_from(count).ok()
}

#[cfg(not(target_os = "linux"))]
fn read_pid_open_fd_count(_pid: u32) -> Option<u64> {
    None
}

#[cfg(unix)]
fn is_current_process_root() -> bool {
    // SAFETY: `geteuid` 为无副作用 POSIX 只读调用。
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn is_current_process_root() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn is_running_in_container() -> bool {
    if Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists() {
        return true;
    }
    if let Ok(cgroup) = std::fs::read_to_string("/proc/1/cgroup")
        && (cgroup.contains("docker")
            || cgroup.contains("containerd")
            || cgroup.contains("kubepods"))
    {
        return true;
    }
    false
}

#[cfg(not(target_os = "linux"))]
fn is_running_in_container() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn can_open_for_write(path: &str) -> bool {
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

#[cfg(not(target_os = "linux"))]
fn can_open_for_write(_path: &str) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn can_write_dir_or_file(path: &str) -> bool {
    let target = Path::new(path);
    if target.exists() {
        return std::fs::OpenOptions::new().write(true).open(target).is_ok();
    }
    let Some(parent) = target.parent() else {
        return false;
    };
    if !parent.exists() {
        return false;
    }
    // 使用 access(W_OK) 检查目录写权限而不创建临时垃圾文件
    use std::ffi::CString;
    let Ok(cpath) = CString::new(parent.to_string_lossy().as_bytes()) else {
        return false;
    };
    // SAFETY: `cpath` 为合法以 NUL 结尾的 C 字符串。
    unsafe { libc::access(cpath.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(target_os = "linux"))]
fn can_write_dir_or_file(_path: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_limits_and_file_nr() {
        let sample = "\
Limit                     Soft Limit           Hard Limit           Units
Max cpu time              unlimited            unlimited            seconds
Max open files            65536                1048576              files
Max locked memory         65536                65536                bytes
";
        assert_eq!(
            parse_proc_limits_nofile(sample),
            (Some(65_536), Some(1_048_576))
        );
        assert_eq!(
            parse_file_nr_allocated("12480\t0\t9223372036854775807\n"),
            Some(12_480)
        );
    }

    #[test]
    fn validates_process_and_system_bounds() {
        let bad_soft_gt_hard = UpdateNofileRequest {
            apply_process: true,
            process_soft: Some(524_288),
            process_hard: Some(262_144),
            apply_to_workers: true,
            persist_app_config: true,
            apply_system: false,
            system_file_max: None,
            system_nr_open: None,
            persist_system_files: false,
        };
        assert!(validate_update_request(&bad_soft_gt_hard, Some(65_536)).is_err());

        let bad_nr_lt_hard = UpdateNofileRequest {
            apply_process: true,
            process_soft: Some(524_288),
            process_hard: Some(524_288),
            apply_to_workers: true,
            persist_app_config: true,
            apply_system: true,
            system_file_max: Some(1_048_576),
            system_nr_open: Some(262_144),
            persist_system_files: false,
        };
        assert!(validate_update_request(&bad_nr_lt_hard, Some(65_536)).is_err());

        let valid = UpdateNofileRequest {
            apply_process: true,
            process_soft: Some(262_144),
            process_hard: Some(262_144),
            apply_to_workers: true,
            persist_app_config: true,
            apply_system: true,
            system_file_max: Some(1_048_576),
            system_nr_open: Some(1_048_576),
            persist_system_files: true,
        };
        let out = validate_update_request(&valid, Some(65_536)).unwrap();
        assert_eq!(out.process, Some((262_144, 262_144)));
        assert_eq!(out.system_file_max, Some(1_048_576));
        assert_eq!(out.system_nr_open, Some(1_048_576));
    }

    #[test]
    fn renders_sysctl_and_limits_conf_cleanly() {
        let sysctl = render_sysctl_conf(Some(2_097_152), Some(1_048_576));
        assert!(sysctl.contains("fs.file-max = 2097152"));
        assert!(sysctl.contains("fs.nr_open = 1048576"));

        let limits = render_limits_conf(262_144, 524_288);
        assert!(limits.contains("* soft nofile 262144"));
        assert!(limits.contains("root hard nofile 524288"));
    }
}
