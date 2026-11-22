pub mod cache;
pub mod dht;
pub mod ipc;
pub mod metadata;
pub mod redis_streams;
pub mod rlimit;
pub mod runner;
pub mod stats;

pub use dht::{build_dht_options, build_dht_options_with_fd_limit};
pub use ipc::{CrawlerIpcState, build_crawler_ipc_router};
pub use rlimit::current_soft_open_file_limit;
pub use runner::run_crawler_worker;
pub use stats::{CrawlerAppStats, CrawlerSnapshot, CrawlerSnapshotInput};

/// 验证本地 `dht-crawler` 依赖已正确接入并可构造配置对象。
pub fn kernel_ready_check() -> dht_crawler::DHTOptions {
    dht_crawler::DHTOptions::default()
}
