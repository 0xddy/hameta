use crate::db::Database;
use crate::error::StorageError;
use hameta_common::{MetricsHourPoint, MetricsMinutePoint};
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow)]
struct MinuteRow {
    ts_minute: i64,
    hashes_per_min: f64,
    metadata_succeeded_per_min: f64,
    stream_pushed_per_min: f64,
    filtered_per_min: f64,
    indexed_written_per_min: f64,
    hot_updated_per_min: f64,
    actual_qps: f64,
    stream_new_len: i64,
    stream_hot_len: i64,
    stream_new_lag: i64,
    udp_rx_bps: f64,
    udp_tx_bps: f64,
    metadata_rx_bps: f64,
    total_rss_bytes: i64,
    avg_cpu_percent: f64,
}

impl From<MinuteRow> for MetricsMinutePoint {
    fn from(r: MinuteRow) -> Self {
        Self {
            ts_minute: r.ts_minute,
            hashes_per_min: r.hashes_per_min,
            metadata_succeeded_per_min: r.metadata_succeeded_per_min,
            stream_pushed_per_min: r.stream_pushed_per_min,
            filtered_per_min: r.filtered_per_min,
            indexed_written_per_min: r.indexed_written_per_min,
            hot_updated_per_min: r.hot_updated_per_min,
            actual_qps: r.actual_qps,
            stream_new_len: r.stream_new_len,
            stream_hot_len: r.stream_hot_len,
            stream_new_lag: r.stream_new_lag,
            udp_rx_bps: r.udp_rx_bps,
            udp_tx_bps: r.udp_tx_bps,
            metadata_rx_bps: r.metadata_rx_bps,
            total_rss_bytes: r.total_rss_bytes,
            avg_cpu_percent: r.avg_cpu_percent,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
struct HourRow {
    ts_hour: i64,
    hashes_total: i64,
    metadata_succeeded_total: i64,
    stream_pushed_total: i64,
    filtered_total: i64,
    indexed_written_total: i64,
    hot_updated_total: i64,
    avg_qps: f64,
    max_stream_new_lag: i64,
    avg_rss_bytes: i64,
}

impl From<HourRow> for MetricsHourPoint {
    fn from(r: HourRow) -> Self {
        Self {
            ts_hour: r.ts_hour,
            hashes_total: r.hashes_total,
            metadata_succeeded_total: r.metadata_succeeded_total,
            stream_pushed_total: r.stream_pushed_total,
            filtered_total: r.filtered_total,
            indexed_written_total: r.indexed_written_total,
            hot_updated_total: r.hot_updated_total,
            avg_qps: r.avg_qps,
            max_stream_new_lag: r.max_stream_new_lag,
            avg_rss_bytes: r.avg_rss_bytes,
        }
    }
}

impl Database {
    pub async fn upsert_metrics_minute(
        &self,
        point: &MetricsMinutePoint,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO metrics_minute (
                ts_minute, hashes_per_min, metadata_succeeded_per_min, stream_pushed_per_min,
                filtered_per_min, indexed_written_per_min, hot_updated_per_min, actual_qps,
                stream_new_len, stream_hot_len, stream_new_lag, udp_rx_bps, udp_tx_bps,
                metadata_rx_bps, total_rss_bytes, avg_cpu_percent
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(ts_minute) DO UPDATE SET
                hashes_per_min = excluded.hashes_per_min,
                metadata_succeeded_per_min = excluded.metadata_succeeded_per_min,
                stream_pushed_per_min = excluded.stream_pushed_per_min,
                filtered_per_min = excluded.filtered_per_min,
                indexed_written_per_min = excluded.indexed_written_per_min,
                hot_updated_per_min = excluded.hot_updated_per_min,
                actual_qps = excluded.actual_qps,
                stream_new_len = excluded.stream_new_len,
                stream_hot_len = excluded.stream_hot_len,
                stream_new_lag = excluded.stream_new_lag,
                udp_rx_bps = excluded.udp_rx_bps,
                udp_tx_bps = excluded.udp_tx_bps,
                metadata_rx_bps = excluded.metadata_rx_bps,
                total_rss_bytes = excluded.total_rss_bytes,
                avg_cpu_percent = excluded.avg_cpu_percent",
        )
        .bind(point.ts_minute)
        .bind(point.hashes_per_min)
        .bind(point.metadata_succeeded_per_min)
        .bind(point.stream_pushed_per_min)
        .bind(point.filtered_per_min)
        .bind(point.indexed_written_per_min)
        .bind(point.hot_updated_per_min)
        .bind(point.actual_qps)
        .bind(point.stream_new_len)
        .bind(point.stream_hot_len)
        .bind(point.stream_new_lag)
        .bind(point.udp_rx_bps)
        .bind(point.udp_tx_bps)
        .bind(point.metadata_rx_bps)
        .bind(point.total_rss_bytes)
        .bind(point.avg_cpu_percent)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn query_metrics_minute(
        &self,
        since_ts: i64,
        limit: i64,
    ) -> Result<Vec<MetricsMinutePoint>, StorageError> {
        let rows = sqlx::query_as::<_, MinuteRow>(
            "SELECT ts_minute, hashes_per_min, metadata_succeeded_per_min, stream_pushed_per_min,
                    filtered_per_min, indexed_written_per_min, hot_updated_per_min, actual_qps,
                    stream_new_len, stream_hot_len, stream_new_lag, udp_rx_bps, udp_tx_bps,
                    metadata_rx_bps, total_rss_bytes, avg_cpu_percent
             FROM metrics_minute
             WHERE ts_minute >= ?1
             ORDER BY ts_minute ASC
             LIMIT ?2",
        )
        .bind(since_ts)
        .bind(limit.max(1))
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn upsert_metrics_hour(&self, point: &MetricsHourPoint) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO metrics_hour (
                ts_hour, hashes_total, metadata_succeeded_total, stream_pushed_total,
                filtered_total, indexed_written_total, hot_updated_total,
                avg_qps, max_stream_new_lag, avg_rss_bytes
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(ts_hour) DO UPDATE SET
                hashes_total = excluded.hashes_total,
                metadata_succeeded_total = excluded.metadata_succeeded_total,
                stream_pushed_total = excluded.stream_pushed_total,
                filtered_total = excluded.filtered_total,
                indexed_written_total = excluded.indexed_written_total,
                hot_updated_total = excluded.hot_updated_total,
                avg_qps = excluded.avg_qps,
                max_stream_new_lag = excluded.max_stream_new_lag,
                avg_rss_bytes = excluded.avg_rss_bytes",
        )
        .bind(point.ts_hour)
        .bind(point.hashes_total)
        .bind(point.metadata_succeeded_total)
        .bind(point.stream_pushed_total)
        .bind(point.filtered_total)
        .bind(point.indexed_written_total)
        .bind(point.hot_updated_total)
        .bind(point.avg_qps)
        .bind(point.max_stream_new_lag)
        .bind(point.avg_rss_bytes)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn query_metrics_hour(
        &self,
        since_ts: i64,
        limit: i64,
    ) -> Result<Vec<MetricsHourPoint>, StorageError> {
        let rows = sqlx::query_as::<_, HourRow>(
            "SELECT ts_hour, hashes_total, metadata_succeeded_total, stream_pushed_total,
                    filtered_total, indexed_written_total, hot_updated_total,
                    avg_qps, max_stream_new_lag, avg_rss_bytes
             FROM metrics_hour
             WHERE ts_hour >= ?1
             ORDER BY ts_hour ASC
             LIMIT ?2",
        )
        .bind(since_ts)
        .bind(limit.max(1))
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn prune_metrics(
        &self,
        minute_before_ts: i64,
        hour_before_ts: i64,
    ) -> Result<(u64, u64), StorageError> {
        let min_res = sqlx::query("DELETE FROM metrics_minute WHERE ts_minute < ?1")
            .bind(minute_before_ts)
            .execute(self.pool())
            .await?;
        let hour_res = sqlx::query("DELETE FROM metrics_hour WHERE ts_hour < ?1")
            .bind(hour_before_ts)
            .execute(self.pool())
            .await?;
        Ok((min_res.rows_affected(), hour_res.rows_affected()))
    }
}
