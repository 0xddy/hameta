import { apiRequest } from "./client";

export type WorkerRole = "crawler" | "indexer";
export type WorkerRuntimeState =
  | "stopped"
  | "starting"
  | "running"
  | "draining"
  | "crashed";
export type HistoryRange = "1h" | "6h" | "24h" | "7d" | "30d";

export interface OverviewHealth {
  panel_ok: boolean;
  redis_ok: boolean;
  redis_message: string | null;
  manticore_ok: boolean;
  manticore_message: string | null;
  running_crawlers: number;
  total_crawlers: number;
  running_indexers: number;
  total_indexers: number;
  crashed_workers: number;
}

export interface OverviewFunnel {
  hashes_per_min: number;
  dedup_blocked_per_min: number;
  metadata_succeeded_per_min: number;
  stream_pushed_per_min: number;
  filter_accepted_per_min: number;
  filter_rejected_per_min: number;
  indexed_written_per_min: number;
  hot_pushed_per_min: number;
  hot_updated_per_min: number;
}

export interface OverviewKpi {
  actual_qps: number;
  indexed_written_per_min: number;
  estimated_daily_docs: number;
  today_indexed_docs: number;
  torrents_total_docs: number | null;
  torrents_disk_bytes: number | null;
  torrents_status_cached_at: number | null;
  stream_new_len: number;
  stream_hot_len: number;
  stream_new_pending: number;
  stream_new_lag: number;
  total_rss_bytes: number;
  udp_rx_bps: number;
  udp_tx_bps: number;
  udp_rx_pps?: number;
  udp_tx_pps?: number;
  metadata_rx_bps: number;
  system_soft_nofile: number | null;
}

export interface OverviewWorkerBrief {
  id: string;
  display_name: string;
  role: WorkerRole;
  runtime_state: WorkerRuntimeState;
  pid: number | null;
  restart_count: number;
  rss_bytes: number;
  last_error: string | null;
}

export interface OverviewSnapshot {
  captured_at: number;
  health: OverviewHealth;
  funnel: OverviewFunnel;
  kpi: OverviewKpi;
  workers: OverviewWorkerBrief[];
}

export interface HistoryPoint {
  ts: number;
  hashes_per_min: number;
  metadata_succeeded_per_min: number;
  stream_pushed_per_min: number;
  filtered_per_min: number;
  indexed_written_per_min: number;
  hot_updated_per_min: number;
  actual_qps: number;
  stream_new_len: number;
  stream_hot_len: number;
  stream_new_lag: number;
  udp_rx_bps: number;
  udp_tx_bps: number;
  metadata_rx_bps: number;
  total_rss_bytes: number;
}

export interface OverviewHistoryResponse {
  range: HistoryRange;
  granularity: "minute" | "hour";
  points: HistoryPoint[];
}

export function getOverviewSnapshot(): Promise<OverviewSnapshot> {
  return apiRequest<OverviewSnapshot>("/api/v1/overview/snapshot");
}

export function getOverviewHistory(range: HistoryRange): Promise<OverviewHistoryResponse> {
  const qs = new URLSearchParams({ range }).toString();
  return apiRequest<OverviewHistoryResponse>(`/api/v1/overview/history?${qs}`);
}
