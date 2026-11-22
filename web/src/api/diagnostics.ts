import { apiRequest } from "./client";
import type { WorkerRole, WorkerRuntimeState } from "./overview";
import type { CrawlProfileDefaults } from "./workers";

export interface HistogramMetrics {
  bounds: number[];
  counts: number[];
  overflow: number;
  count: number;
  sum: number;
  p50: number | null;
  p95: number | null;
}

export interface TimeoutStagesMetrics {
  connect_handshake: number;
  tcp_connect: number;
  base_handshake: number;
  extension_handshake_send: number;
  extension_handshake_receive: number;
  piece_request_send: number;
  piece_receive: number;
  validation: number;
}

export interface ConnectionErrorsMetrics {
  refused: number;
  reset: number;
  aborted: number;
  closed: number;
  timeout: number;
  invalid_handshake: number;
  info_hash_mismatch: number;
  io_other: number;
  other: number;
}

export interface CrawlerSnapshot {
  uptime_secs: number;
  crawl_profile: string;
  effective_metadata_workers: number;
  local_cache_entries: number;
  redis_pool_size: number;
  redis_pool_available: number;
  rss_bytes: number;
  app: Record<string, number>;
  dht: Record<string, number>;
  observability: {
    udp_rx_packets: number;
    udp_rx_bytes: number;
    udp_tx_packets: number;
    udp_tx_bytes: number;
    inbound_ping: number;
    inbound_find_node: number;
    inbound_get_peers: number;
    inbound_announce_peer: number;
    inbound_other: number;
    response_normal: number;
    response_fallback: number;
    response_rate_limited: number;
    response_send_failed: number;
    announce_accepted: number;
    announce_invalid_token: number;
    announce_filtered: number;
    node_admitted: number;
    node_replaced: number;
    node_dropped_duplicate: number;
    node_dropped_rate_limited: number;
    node_dropped_invalid: number;
    metadata_bytes_downloaded: number;
    metadata_failure_timeout: number;
    metadata_failure_connect: number;
    metadata_timeout_stages: TimeoutStagesMetrics;
    metadata_connection_errors: ConnectionErrorsMetrics;
    metadata_failure_no_extension: number;
    metadata_failure_send: number;
    metadata_failure_size_limit: number;
    metadata_failure_sha1: number;
    metadata_failure_parse: number;
    metadata_failure_peer_closed: number;
    metadata_failure_protocol: number;
    metadata_failure_other: number;
    metadata_failure_reject: number;
    metadata_failure_tcp_connect: number;
    metadata_failure_base_handshake: number;
    peer_cache_timeout_hits: number;
    peer_cache_connect_hits: number;
    peer_cache_endpoint_hits: number;
    peer_cache_metadata_hits: number;
    queue_wait_ms: HistogramMetrics;
    fetch_duration_ms: HistogramMetrics;
    metadata_size_bytes: HistogramMetrics;
  };
  yield_counters: Record<string, number>;
  yield_gauges: Record<string, number>;
}

export interface IndexerSnapshot {
  uptime_secs: number;
  active_rule_version: number;
  redis_pool_size: number;
  redis_pool_available: number;
  rss_bytes: number;
  app: {
    torrent_messages_read: number;
    hot_messages_read: number;
    malformed_messages_acked: number;
    poison_messages_acked: number;
    filter_accepted: number;
    filter_rejected: number;
    batches_written: number;
    batches_failed: number;
    records_written: number;
    redis_indexed_marked: number;
    redis_mark_errors: number;
    stream_xacked_new: number;
    stream_xacked_hot: number;
    hot_records_updated: number;
    hot_update_errors: number;
    reclaimed_new: number;
    reclaimed_hot: number;
    torrent_buffered: number;
    torrent_in_flight: number;
    hot_buffered: number;
  };
  filter: {
    active_version: number;
    total_evaluated: number;
    total_accepted: number;
    total_rejected: number;
  };
}

export interface WorkerDiagnosticsResponse {
  id: string;
  display_name: string;
  role: WorkerRole;
  runtime_state: WorkerRuntimeState;
  pid: number | null;
  worker_port: number | null;
  active_config_version: number;
  active_rule_version: number | null;
  started_at: number | null;
  restart_count: number;
  last_error: string | null;
  captured_at_unix: number | null;
  snapshot: CrawlerSnapshot | IndexerSnapshot | null;
  config: Record<string, unknown> | null;
  profile_defaults: CrawlProfileDefaults | null;
}

export function getWorkerDiagnostics(id: string): Promise<WorkerDiagnosticsResponse> {
  return apiRequest<WorkerDiagnosticsResponse>(
    `/api/v1/workers/${encodeURIComponent(id)}/diagnostics`,
  );
}
