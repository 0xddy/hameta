use chrono::Local;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout};
use tokio::sync::broadcast;

pub const STDERR_RING_CAPACITY_BYTES: usize = 65_536;

/// 保留子进程最近 64 KiB 标准错误输出的线程安全环形缓冲。
#[derive(Debug)]
pub struct StderrRingBuffer {
    max_bytes: usize,
    inner: Mutex<VecDeque<u8>>,
}

impl Default for StderrRingBuffer {
    fn default() -> Self {
        Self::new(STDERR_RING_CAPACITY_BYTES)
    }
}

impl StderrRingBuffer {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes: max_bytes.max(64),
            inner: Mutex::new(VecDeque::with_capacity(max_bytes.min(65_536))),
        }
    }

    pub fn clear(&self) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clear();
    }

    pub fn push_line(&self, line: &str) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for &b in line.as_bytes() {
            if guard.len() >= self.max_bytes {
                guard.pop_front();
            }
            guard.push_back(b);
        }
        if guard.len() >= self.max_bytes {
            guard.pop_front();
        }
        guard.push_back(b'\n');
    }

    pub fn snapshot_text(&self) -> String {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bytes: Vec<u8> = guard.iter().copied().collect();
        // 若最早 UTF-8 字符被环形截断，跳过非法前导字节避免乱码字符
        let mut start = 0;
        while start < bytes.len() && std::str::from_utf8(&bytes[start..]).is_err() && start < 4 {
            start += 1;
        }
        String::from_utf8_lossy(&bytes[start..]).into_owned()
    }
}

pub fn spawn_log_pumps(
    instance_id: String,
    logs_dir: PathBuf,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    stderr_ring: Arc<StderrRingBuffer>,
    log_tx: broadcast::Sender<String>,
) {
    let (file_tx, mut file_rx) = tokio::sync::mpsc::channel::<String>(2_048);

    // 单协程串行负责按天轮转写入日志文件，消除 stdout/stderr 并发写文件交错
    let id_for_file = instance_id.clone();
    tokio::spawn(async move {
        let _ = tokio::fs::create_dir_all(&logs_dir).await;
        let mut active_date = String::new();
        let mut active_file: Option<tokio::fs::File> = None;

        while let Some(line) = file_rx.recv().await {
            let today = Local::now().format("%Y-%m-%d").to_string();
            if active_date != today || active_file.is_none() {
                let file_path = logs_dir.join(format!("worker-{id_for_file}-{today}.log"));
                match OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&file_path)
                    .await
                {
                    Ok(f) => {
                        active_date = today;
                        active_file = Some(f);
                    }
                    Err(err) => {
                        tracing::debug!("打开 Worker 日志文件 {} 失败: {err}", file_path.display());
                        continue;
                    }
                }
            }
            if let Some(file) = active_file.as_mut() {
                let _ = file.write_all(line.as_bytes()).await;
                let _ = file.write_all(b"\n").await;
            }
        }
    });

    if let Some(out) = stdout {
        let tx_file = file_tx.clone();
        let tx_broad = log_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx_broad.send(line.clone());
                let _ = tx_file.try_send(line);
            }
        });
    }

    if let Some(err_stream) = stderr {
        let tx_file = file_tx;
        let tx_broad = log_tx;
        tokio::spawn(async move {
            let mut lines = BufReader::new(err_stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stderr_ring.push_line(&line);
                let formatted = format!("[stderr] {line}");
                let _ = tx_broad.send(formatted.clone());
                let _ = tx_file.try_send(formatted);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_ring_caps_output_at_configured_bytes() {
        let ring = StderrRingBuffer::new(100);
        for idx in 0..20 {
            ring.push_line(&format!("line-{idx:02}-abcdefghij"));
        }
        let text = ring.snapshot_text();
        assert!(text.len() <= 100);
        assert!(text.contains("line-19-abcdefghij"));
        assert!(!text.contains("line-00-abcdefghij"));
    }
}
