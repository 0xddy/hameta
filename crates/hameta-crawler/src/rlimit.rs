const FD_SAFETY_PERCENT: usize = 70;
const NON_METADATA_FD_RESERVE: usize = 32;
const DEFAULT_TARGET_NOFILE: usize = 262_144;

pub fn required_open_file_limit(configured: usize, reserved_fd_count: usize) -> usize {
    let reserved = reserved_fd_count.saturating_add(NON_METADATA_FD_RESERVE);
    let needed_budget = configured
        .max(1)
        .saturating_mul(dht_crawler::scheduler::MAX_METADATA_PEERS_PER_HASH)
        .saturating_add(reserved);
    let required = needed_budget
        .saturating_mul(100)
        .saturating_add(FD_SAFETY_PERCENT - 1)
        / FD_SAFETY_PERCENT;
    required.max(DEFAULT_TARGET_NOFILE)
}

#[cfg(target_os = "linux")]
fn try_raise_open_file_limit(target: usize) {
    let target_rlim = libc::rlim_t::try_from(target).unwrap_or(libc::RLIM_INFINITY);
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` 为已初始化的 `libc::rlimit`，合法提供给 `getrlimit` 写入。
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        tracing::warn!(
            "读取 RLIMIT_NOFILE 失败: {}",
            std::io::Error::last_os_error()
        );
        return;
    }
    if limit.rlim_cur >= target_rlim {
        return;
    }

    let previous_cur = limit.rlim_cur;
    let previous_max = limit.rlim_max;
    let raised_hard = target_rlim.max(limit.rlim_max);
    let desired = libc::rlimit {
        rlim_cur: target_rlim,
        rlim_max: raised_hard,
    };
    // SAFETY: `desired` 满足 `rlim_cur <= rlim_max`。
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &desired) } == 0 {
        tracing::info!(
            "已提升进程 RLIMIT_NOFILE: {previous_cur}/{previous_max} -> {target_rlim}/{raised_hard}"
        );
        return;
    }

    let hard_raise_err = std::io::Error::last_os_error();
    let soft_only_target = target_rlim.min(limit.rlim_max);
    if soft_only_target > limit.rlim_cur {
        let soft_desired = libc::rlimit {
            rlim_cur: soft_only_target,
            rlim_max: limit.rlim_max,
        };
        // SAFETY: `soft_desired` 满足 `rlim_cur <= rlim_max`。
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &soft_desired) } == 0 {
            tracing::info!(
                "已提升进程软 RLIMIT_NOFILE: {previous_cur}/{previous_max} -> {soft_only_target}/{previous_max}"
            );
            if soft_only_target < target_rlim {
                tracing::warn!(
                    "进程 RLIMIT_NOFILE 硬上限 ({previous_max}) 低于目标 {target_rlim}，请在 systemd 配置 LimitNOFILE={target_rlim} 或执行 ulimit -n {target_rlim}"
                );
            }
            return;
        }
    }

    tracing::warn!(
        "无法提升 RLIMIT_NOFILE ({previous_cur}/{previous_max} -> {target_rlim}): {hard_raise_err}；请在 systemd 配置 LimitNOFILE={target_rlim} 或执行 ulimit -n {target_rlim}"
    );
}

#[cfg(not(target_os = "linux"))]
fn try_raise_open_file_limit(_target: usize) {}

/// 查询当前进程软文件描述符上限（仅 Linux 返回实际值）。
pub fn current_soft_open_file_limit() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        let limits = std::fs::read_to_string("/proc/self/limits").ok()?;
        limits.lines().find_map(|line| {
            let rest = line.strip_prefix("Max open files")?;
            rest.split_whitespace().next()?.parse().ok()
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub fn effective_metadata_worker_count(configured: usize, reserved_fd_count: usize) -> usize {
    let configured = configured.max(1);
    try_raise_open_file_limit(required_open_file_limit(configured, reserved_fd_count));
    clamp_metadata_workers(
        configured,
        reserved_fd_count,
        current_soft_open_file_limit(),
    )
}

pub fn clamp_metadata_workers(
    configured: usize,
    reserved_fd_count: usize,
    open_file_limit: Option<usize>,
) -> usize {
    let configured = configured.max(1);
    let Some(limit) = open_file_limit else {
        return configured;
    };
    let effective = metadata_workers_for_open_file_limit(configured, reserved_fd_count, limit);
    if effective < configured {
        let reserved = reserved_fd_count.saturating_add(NON_METADATA_FD_RESERVE);
        tracing::warn!(
            "由于进程文件描述符软上限为 {limit}，已将 Metadata 并发协程由 {configured} 安全收敛至 {effective} (reserved_fds={reserved}, peer_race_width={}, budget={FD_SAFETY_PERCENT}%)",
            dht_crawler::scheduler::MAX_METADATA_PEERS_PER_HASH,
        );
    }
    effective
}

pub fn metadata_workers_for_open_file_limit(
    configured: usize,
    reserved_fd_count: usize,
    open_file_limit: usize,
) -> usize {
    let fd_budget = open_file_limit.saturating_mul(FD_SAFETY_PERCENT) / 100;
    let reserved = reserved_fd_count.saturating_add(NON_METADATA_FD_RESERVE);
    let safe_workers = fd_budget
        .saturating_sub(reserved)
        .checked_div(dht_crawler::scheduler::MAX_METADATA_PEERS_PER_HASH)
        .unwrap_or_default()
        .max(1);
    configured.max(1).min(safe_workers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_worker_count_under_tight_fd_limit() {
        let clamped = metadata_workers_for_open_file_limit(512, 100, 1024);
        assert!(clamped < 512);
        assert!(clamped >= 1);

        let plenty = metadata_workers_for_open_file_limit(256, 100, 262_144);
        assert_eq!(plenty, 256);

        assert!(required_open_file_limit(1024, 100) >= DEFAULT_TARGET_NOFILE);
    }
}
