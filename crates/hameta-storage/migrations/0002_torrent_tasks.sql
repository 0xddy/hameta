-- 磁力库异步维护任务（同步文件大小、按最新规则重洗标题等）
CREATE TABLE IF NOT EXISTS torrent_tasks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    kind            TEXT NOT NULL,
    status          TEXT NOT NULL,
    params_json     TEXT NOT NULL,
    cursor_id       INTEGER NOT NULL DEFAULT 0,
    estimated_total INTEGER NOT NULL DEFAULT 0,
    scanned_count   INTEGER NOT NULL DEFAULT 0,
    updated_count   INTEGER NOT NULL DEFAULT 0,
    skipped_count   INTEGER NOT NULL DEFAULT 0,
    error_count     INTEGER NOT NULL DEFAULT 0,
    batches_done    INTEGER NOT NULL DEFAULT 0,
    rate_per_sec    REAL NOT NULL DEFAULT 0.0,
    last_error      TEXT NOT NULL DEFAULT '',
    sample_diffs_json TEXT NOT NULL DEFAULT '[]',
    created_by      TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    started_at      INTEGER,
    updated_at      INTEGER NOT NULL,
    finished_at     INTEGER
);

CREATE INDEX IF NOT EXISTS idx_torrent_tasks_status ON torrent_tasks(status, id DESC);
CREATE INDEX IF NOT EXISTS idx_torrent_tasks_created ON torrent_tasks(id DESC);
