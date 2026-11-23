pub mod batcher;
pub mod hot_batcher;
pub mod ipc;
pub mod manticore;
pub mod runner;
pub mod stats;
pub mod streams;

pub use ipc::{IndexerIpcState, IngestEventReporter, build_indexer_ipc_router};
pub use manticore::{
    ColumnSchemaInfo, ManticoreClient, SchemaCheckReport, TableStatusInfo, validate_bulk_response,
    validate_sql_response,
};
pub use runner::run_indexer_worker;
pub use stats::{IndexerAppMetrics, IndexerAppStats, IndexerSnapshot};
pub use streams::RedisStreamsConsumer;

/// 占位验证 `hameta-filter` 依赖可达。
pub fn indexer_ready_check() -> u32 {
    hameta_filter::FILTER_ENGINE_API_VERSION
}
