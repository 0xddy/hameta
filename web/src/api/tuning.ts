import { apiRequest } from "./client";

export type TuningGoal = "balanced" | "throughput" | "low_resource";
export type TuningSessionState = "collecting" | "completed" | "cancelled";
export type TuningConfidenceLevel = "insufficient" | "low" | "medium" | "high";
export type TuningSeverity = "critical" | "warning" | "opportunity" | "info";
export type TuningAction = "increase" | "decrease" | "tune" | "system";
export type TuningRegime =
  | "nofile_clamped"
  | "cpu_udp_saturated"
  | "redis_sink_bottleneck"
  | "bandwidth_ceiling"
  | "overfed_backpressure"
  | "underfed_starvation"
  | "balanced_healthy";

export interface TuningConstraints {
  up_mbps: number | null;
  down_mbps: number | null;
  goal: TuningGoal;
}

export interface TuningWindowSummary {
  sample_count: number;
  continuous_samples: number;
  restart_detected: boolean;
  window_secs: number;
  configured_find_node_qps: number;
  actual_find_node_qps: number;
  effective_find_node_p50: number;
  effective_find_node_min: number;
  throttled_time_pct: number;
  severe_throttled_time_pct: number;

  est_wire_tx_mbps: number;
  est_wire_rx_mbps: number;
  udp_tx_mbps: number;
  udp_rx_mbps: number;
  metadata_rx_mbps: number;
  udp_total_pps: number;

  meta_queue_fill_avg_pct: number;
  meta_queue_fill_p95_pct: number;
  meta_queue_full_time_pct: number;
  meta_queue_wait_p50_ms: number | null;
  meta_queue_wait_p95_ms: number | null;
  meta_queue_evict_pct: number;
  meta_queue_stale_pct: number;
  hash_ingress_dropped_total: number;

  configured_workers: number;
  effective_workers: number;
  fd_clamped: boolean;
  worker_util_avg_pct: number;
  worker_util_p95_pct: number;
  fetch_duration_p50_ms: number | null;
  fetch_duration_p95_ms: number | null;
  torrents_per_min: number;
  job_success_rate_pct: number;
  peer_timeout_rate_pct: number;
  timeout_tcp_connect_share_pct: number;
  timeout_piece_recv_share_pct: number;
  timeout_validation_share_pct: number;

  response_rate_limited_pct: number;
  udp_queue_full_total: number;
  find_node_in_flight_p95_pct: number;
  find_node_unmatched_pct: number;
  node_pool_fill_p95_pct: number;
  node_dropped_rate_limited_pct: number;
  peer_lookup_rate_limited_pct: number;
  peer_lookup_yield_per_started: number;
  discovery_get_peers_limited_pct: number;
  discovery_sample_hash_limited_pct: number;

  local_cache_fill_pct: number;
  redis_pool_exhausted_time_pct: number;
  sink_backpressure_time_pct: number;
  rss_p95_mb: number;
  rss_growth_mb_per_hour: number;
}

export interface TuningRecommendation {
  id: string;
  category: string;
  severity: TuningSeverity;
  action: TuningAction;
  param_path: string;
  param_label: string;
  current_display: string;
  recommended_display: string;
  current_value: unknown;
  recommended_value: unknown;
  delta_pct: number | null;
  evidence: string;
  reason: string;
  apply_supported: boolean;
}

export interface TuningReport {
  worker_id: string;
  source: "session" | "ring" | string;
  generated_at: number;
  config_version: number;
  confidence: TuningConfidenceLevel;
  confidence_note: string;
  health_score: number;
  regime: TuningRegime;
  regime_title: string;
  regime_summary: string;
  constraints: TuningConstraints;
  summary: TuningWindowSummary;
  recommendations: TuningRecommendation[];
  increase_count: number;
  decrease_count: number;
  system_count: number;
}

export interface TuningSessionStatus {
  worker_id: string;
  state: TuningSessionState;
  started_at: number;
  finished_at: number | null;
  target_duration_secs: number;
  elapsed_secs: number;
  sample_interval_secs: number;
  sample_count: number;
  memory_bytes: number;
  progress_pct: number;
  constraints: TuningConstraints;
}

export interface RollingBufferStatus {
  sample_count: number;
  max_samples: number;
  window_secs: number;
  memory_bytes: number;
  confidence: TuningConfidenceLevel;
}

export interface TuningWorkerOverview {
  worker_id: string;
  session: TuningSessionStatus | null;
  rolling: RollingBufferStatus;
  report: TuningReport | null;
}

export interface StartTuningPayload {
  duration_secs?: number;
  sample_interval_secs?: number;
  up_mbps?: number | null;
  down_mbps?: number | null;
  goal?: TuningGoal;
}

export interface ApplyTuningPayload {
  recommendation_ids: string[];
  source?: "session" | "ring";
  up_mbps?: number | null;
  down_mbps?: number | null;
  goal?: TuningGoal;
  apply_restart?: boolean;
  note?: string;
}

export interface ApplyTuningResponse {
  version: number;
  applied: string[];
  config: Record<string, unknown>;
  restarted: boolean;
  restart_error: string | null;
}

export function listTuningSessions(): Promise<TuningSessionStatus[]> {
  return apiRequest<TuningSessionStatus[]>("/api/v1/tuning/sessions");
}

export function getWorkerTuningStatus(
  id: string,
  params?: {
    source?: "session" | "ring";
    up_mbps?: number | null;
    down_mbps?: number | null;
    goal?: TuningGoal;
  },
): Promise<TuningWorkerOverview> {
  const qs = new URLSearchParams();
  if (params?.source) qs.set("source", params.source);
  if (params?.up_mbps && params.up_mbps > 0) qs.set("up_mbps", String(params.up_mbps));
  if (params?.down_mbps && params.down_mbps > 0) qs.set("down_mbps", String(params.down_mbps));
  if (params?.goal) qs.set("goal", params.goal);
  const query = qs.toString();
  return apiRequest<TuningWorkerOverview>(
    `/api/v1/workers/${encodeURIComponent(id)}/tuning/status${query ? `?${query}` : ""}`,
  );
}

export function startWorkerTuning(
  id: string,
  payload: StartTuningPayload,
): Promise<TuningSessionStatus> {
  return apiRequest<TuningSessionStatus>(
    `/api/v1/workers/${encodeURIComponent(id)}/tuning/start`,
    {
      method: "POST",
      body: JSON.stringify(payload),
    },
  );
}

export function finishWorkerTuning(id: string): Promise<TuningSessionStatus> {
  return apiRequest<TuningSessionStatus>(
    `/api/v1/workers/${encodeURIComponent(id)}/tuning/finish`,
    {
      method: "POST",
    },
  );
}

export function cancelWorkerTuning(id: string): Promise<{ ok: boolean; removed: boolean }> {
  return apiRequest<{ ok: boolean; removed: boolean }>(
    `/api/v1/workers/${encodeURIComponent(id)}/tuning/cancel`,
    {
      method: "POST",
    },
  );
}

export function applyWorkerTuning(
  id: string,
  payload: ApplyTuningPayload,
): Promise<ApplyTuningResponse> {
  return apiRequest<ApplyTuningResponse>(
    `/api/v1/workers/${encodeURIComponent(id)}/tuning/apply`,
    {
      method: "POST",
      body: JSON.stringify(payload),
    },
  );
}
