-- 1. 系统全局元信息（引导完成状态、安装实例 ID 等）
CREATE TABLE IF NOT EXISTS system_meta (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
);

-- 2. 管理员账号
CREATE TABLE IF NOT EXISTS admin_users (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    username        TEXT NOT NULL UNIQUE,
    password_hash   TEXT NOT NULL,          -- 加盐 SHA-256 哈希（salt$hex）
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    last_login_at   INTEGER
);

-- 3. 登录会话（服务端只存随机 Token 的 SHA-256 摘要）
CREATE TABLE IF NOT EXISTS admin_sessions (
    token_hash      TEXT PRIMARY KEY,
    user_id         INTEGER NOT NULL REFERENCES admin_users(id) ON DELETE CASCADE,
    client_ip       TEXT NOT NULL,
    user_agent      TEXT NOT NULL DEFAULT '',
    created_at      INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL,
    last_seen_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_admin_sessions_expires ON admin_sessions(expires_at);

-- 4. 服务连接配置版本表（Redis 与 Manticore 配置统一版本化）
-- kind: 'redis' | 'manticore'
CREATE TABLE IF NOT EXISTS service_configs (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    kind                TEXT NOT NULL CHECK (kind IN ('redis', 'manticore')),
    version             INTEGER NOT NULL,
    config_json         TEXT NOT NULL,      -- 完整配置 JSON（含 Redis / Manticore 连接与密码，本地明文存储）
    is_active           INTEGER NOT NULL DEFAULT 0,
    last_test_ok        INTEGER,            -- NULL 未测试, 1 成功, 0 失败
    last_test_message   TEXT,
    last_tested_at      INTEGER,
    created_by          TEXT NOT NULL,
    note                TEXT NOT NULL DEFAULT '',
    created_at          INTEGER NOT NULL,
    UNIQUE (kind, version)
);
CREATE INDEX IF NOT EXISTS idx_service_configs_active ON service_configs(kind, is_active);

-- 5. Worker 实例注册与守护策略表
-- role: 'crawler' | 'indexer'
-- desired_state: 'stopped' | 'running'
CREATE TABLE IF NOT EXISTS worker_instances (
    id                      TEXT PRIMARY KEY,
    role                    TEXT NOT NULL CHECK (role IN ('crawler', 'indexer')),
    display_name            TEXT NOT NULL,
    desired_state           TEXT NOT NULL DEFAULT 'stopped' CHECK (desired_state IN ('stopped', 'running')),
    active_config_version   INTEGER NOT NULL DEFAULT 1,
    auto_restart            INTEGER NOT NULL DEFAULT 1,
    max_restarts            INTEGER NOT NULL DEFAULT 5,
    restart_backoff_secs    INTEGER NOT NULL DEFAULT 3,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL
);

-- 6. Worker 实例配置版本表（支持历史 Diff 与一键回滚）
CREATE TABLE IF NOT EXISTS worker_config_versions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    instance_id     TEXT NOT NULL REFERENCES worker_instances(id) ON DELETE CASCADE,
    version         INTEGER NOT NULL,
    config_json     TEXT NOT NULL,
    created_by      TEXT NOT NULL,
    note            TEXT NOT NULL DEFAULT '',
    created_at      INTEGER NOT NULL,
    UNIQUE (instance_id, version)
);

-- 7. 过滤规则与清洗词库版本表（草稿 + 线上发布版本）
-- status: 'draft' | 'published' | 'archived'
CREATE TABLE IF NOT EXISTS filter_rule_versions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    version         INTEGER NOT NULL UNIQUE,
    status          TEXT NOT NULL CHECK (status IN ('draft', 'published', 'archived')),
    filter_json     TEXT NOT NULL,
    ad_phrases_json TEXT NOT NULL,
    ad_regexes_json TEXT NOT NULL,
    created_by      TEXT NOT NULL,
    note            TEXT NOT NULL DEFAULT '',
    created_at      INTEGER NOT NULL,
    published_at    INTEGER
);
CREATE INDEX IF NOT EXISTS idx_filter_versions_status ON filter_rule_versions(status);

-- 8. 分钟级持久化历史指标表
CREATE TABLE IF NOT EXISTS metrics_minute (
    ts_minute                   INTEGER PRIMARY KEY,
    hashes_per_min              REAL NOT NULL,
    metadata_succeeded_per_min  REAL NOT NULL,
    stream_pushed_per_min       REAL NOT NULL,
    filtered_per_min            REAL NOT NULL,
    indexed_written_per_min     REAL NOT NULL,
    hot_updated_per_min         REAL NOT NULL,
    actual_qps                  REAL NOT NULL,
    stream_new_len              INTEGER NOT NULL,
    stream_hot_len              INTEGER NOT NULL,
    stream_new_lag              INTEGER NOT NULL,
    udp_rx_bps                  REAL NOT NULL,
    udp_tx_bps                  REAL NOT NULL,
    metadata_rx_bps             REAL NOT NULL,
    total_rss_bytes             INTEGER NOT NULL,
    avg_cpu_percent             REAL NOT NULL
);

-- 9. 小时级持久化历史指标表
CREATE TABLE IF NOT EXISTS metrics_hour (
    ts_hour                     INTEGER PRIMARY KEY,
    hashes_total                INTEGER NOT NULL,
    metadata_succeeded_total    INTEGER NOT NULL,
    stream_pushed_total         INTEGER NOT NULL,
    filtered_total              INTEGER NOT NULL,
    indexed_written_total       INTEGER NOT NULL,
    hot_updated_total           INTEGER NOT NULL,
    avg_qps                     REAL NOT NULL,
    max_stream_new_lag          INTEGER NOT NULL,
    avg_rss_bytes               INTEGER NOT NULL
);

-- 10. 操作审计日志
CREATE TABLE IF NOT EXISTS audit_logs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    target      TEXT NOT NULL DEFAULT '',
    detail_json TEXT NOT NULL DEFAULT '{}',
    client_ip   TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_logs_created ON audit_logs(created_at DESC);
