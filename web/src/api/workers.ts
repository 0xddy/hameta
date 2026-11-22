import { apiRequest } from "./client";
import type { WorkerRole, WorkerRuntimeState } from "./overview";

export type DesiredState = "stopped" | "running";
export type NetModeConfig = "ipv4_only" | "ipv6_only" | "dual";

export interface CrawlProfileDefaults {
  find_node_rate_per_sec: number;
  find_node_burst: number;
  find_node_max_in_flight: number;
  max_new_destinations_per_minute: number;
  max_response_rate_per_sec: number;
  max_response_bytes_per_sec: number;
  max_response_rate_per_source: number;
  max_replacements_per_minute: number;
  max_in_flight_per_subnet: number;
  peer_lookup_rate_per_sec: number;
  peer_lookup_burst: number;
  peer_lookup_max_active: number;
  pool_capacity: number;
  responsive_capacity: number;
  routing_snapshot_size: number;
}

export interface CrawlProfileInfo {
  name: string;
  level: string;
  recommended_nofile: number;
  defaults: CrawlProfileDefaults;
}

export interface DhtProfilesResponse {
  profiles: CrawlProfileInfo[];
  system_soft_nofile: number | null;
}

export interface CrawlerMetadataConfig {
  timeout_secs: number;
  connect_timeout_ms: number;
  max_queue_size: number;
  max_worker_count: number;
  peer_failure_cache_capacity: number;
  peer_failure_ttl_secs: number;
  retry_initial_delay_secs: number;
  retry_max_delay_secs: number;
  retry_max_attempts: number;
  retry_history_ttl_secs: number;
  retry_history_capacity: number;
}

export interface CrawlerDiscoveryConfig {
  enabled: boolean;
  sample_queries_per_second: number;
  sample_burst: number;
  sample_max_in_flight: number;
  sample_query_timeout_secs: number;
  sample_min_interval_secs: number;
  sample_failure_backoff_secs: number;
  sample_no_new_backoff_secs: number;
  sample_max_nodes: number;
  sample_cooldown_capacity: number;
  hash_dedup_capacity: number;
  hash_dedup_ttl_secs: number;
  get_peers_hashes_per_second: number;
  get_peers_burst: number;
  sample_hashes_per_second: number;
  sample_hash_burst: number;
}

export interface CrawlerPeerLookupConfig {
  max_lookups_per_second?: number;
  burst?: number;
  max_active_lookups?: number;
  max_queries?: number;
  max_concurrent_queries?: number;
  lookup_timeout_ms?: number;
  query_timeout_ms?: number;
}

export interface CrawlerRateLimitConfig {
  max_find_node_rate_per_sec?: number;
  burst?: number;
  max_in_flight?: number;
  request_timeout_secs?: number;
  max_new_destinations_per_minute?: number;
  max_response_rate_per_sec?: number;
  max_response_bytes_per_sec?: number;
  max_response_rate_per_source?: number;
  metadata_pressure_floor_percent?: number;
  max_replacements_per_minute?: number;
  max_in_flight_per_subnet?: number;
}

export interface CrawlerPoolConfig {
  capacity?: number;
  recent_probe_ttl_secs?: number;
  responsive_capacity?: number;
  responsive_ttl_secs?: number;
  revisit_interval_secs?: number;
  low_watermark?: number;
}

export interface CrawlerBootstrapConfig {
  nodes?: string[];
  interval_secs?: number;
  max_nodes_per_round?: number;
  source_backoff_base_secs?: number;
  source_backoff_max_secs?: number;
}

export interface CrawlerTargetConfig {
  random_walk_percent?: number;
  sparse_bucket_percent?: number;
  neighbor_sender_id?: boolean;
}

export interface CrawlerSchedulerConfig {
  priority_event_channel_capacity?: number;
  discovery_event_channel_capacity?: number;
  event_batch_limit?: number;
  node_batch_limit?: number;
  routing_snapshot_size?: number;
  snapshot_refresh_millis?: number;
}

export interface CrawlerCrawlConfig {
  rate_limit: CrawlerRateLimitConfig;
  pool: CrawlerPoolConfig;
  bootstrap: CrawlerBootstrapConfig;
  target: CrawlerTargetConfig;
  scheduler: CrawlerSchedulerConfig;
}

export interface CrawlerWorkerConfig {
  port: number;
  netmode: NetModeConfig;
  crawl_profile: string;
  log_level: string;
  metadata_cache_capacity: number;
  push_concurrency: number;
  metadata_pending_ttl_secs: number;
  metadata_failure_backoff_secs: number;
  hash_queue_capacity: number;
  metadata: CrawlerMetadataConfig;
  discovery: CrawlerDiscoveryConfig;
  peer_lookup: CrawlerPeerLookupConfig;
  crawl: CrawlerCrawlConfig;
}

export interface IndexerWorkerConfig {
  torrent_workers: number;
  hot_workers: number;
  read_block_ms: number;
  read_batch_size: number;
  insert_workers: number;
  batch_size: number;
  flush_interval_ms: number;
  channel_buffer_size: number;
  hot_channel_buffer_size: number;
  xautoclaim_interval_secs: number;
  xautoclaim_min_idle_ms: number;
  max_delivery_attempts: number;
  log_level: string;
}

export interface WorkerDrainStatus {
  draining: boolean;
  queued_items: number;
  in_flight_items: number;
  completed: boolean;
}

export interface WorkerSnapshotEnvelope {
  instance_id: string;
  role: WorkerRole;
  captured_at_unix: number;
  payload: Record<string, unknown>;
}

export interface WorkerSummaryItem {
  id: string;
  role: WorkerRole;
  display_name: string;
  desired_state: DesiredState;
  runtime_state: WorkerRuntimeState;
  active_config_version: number;
  auto_restart: boolean;
  max_restarts: number;
  restart_backoff_secs: number;
  pid: number | null;
  worker_port: number | null;
  active_rule_version: number | null;
  started_at: number | null;
  restart_count: number;
  last_exit_code: number | null;
  last_error: string | null;
  stderr_tail: string;
  drain_status: WorkerDrainStatus | null;
  snapshot: WorkerSnapshotEnvelope | null;
  summary: Record<string, unknown> | null;
}

export interface CreateWorkerPayload {
  id: string;
  role: WorkerRole;
  display_name: string;
  config: CrawlerWorkerConfig | IndexerWorkerConfig;
  auto_restart: boolean;
  max_restarts: number;
  restart_backoff_secs: number;
}

export interface UpdateWorkerPolicyPayload {
  display_name: string;
  auto_restart: boolean;
  max_restarts: number;
  restart_backoff_secs: number;
}

export interface WorkerConfigDetailResponse {
  id: string;
  role: WorkerRole;
  version: number;
  config: CrawlerWorkerConfig | IndexerWorkerConfig;
  profile_defaults: CrawlProfileDefaults | null;
  note: string;
  created_by: string;
  created_at: number;
}

export interface UpdateWorkerConfigResponse {
  version: number;
  config: CrawlerWorkerConfig | IndexerWorkerConfig;
  restarted: boolean;
  restart_error: string | null;
}

export interface WorkerConfigVersionItem {
  id: number;
  version: number;
  config: Record<string, unknown>;
  created_by: string;
  note: string;
  created_at: number;
}

export function defaultCrawlerConfig(port = 12313, profile = "P1"): CrawlerWorkerConfig {
  return {
    port,
    netmode: "ipv4_only",
    crawl_profile: profile,
    log_level: "warn",
    metadata_cache_capacity: 1_000_000,
    push_concurrency: 16,
    metadata_pending_ttl_secs: 3600,
    metadata_failure_backoff_secs: 60,
    hash_queue_capacity: 4_096,
    metadata: {
      timeout_secs: 4,
      connect_timeout_ms: 2_000,
      max_queue_size: 10_000,
      max_worker_count: 256,
      peer_failure_cache_capacity: 200_000,
      peer_failure_ttl_secs: 60,
      retry_initial_delay_secs: 62,
      retry_max_delay_secs: 3_600,
      retry_max_attempts: 5,
      retry_history_ttl_secs: 21_600,
      retry_history_capacity: 100_000,
    },
    discovery: {
      enabled: true,
      sample_queries_per_second: 4,
      sample_burst: 8,
      sample_max_in_flight: 16,
      sample_query_timeout_secs: 3,
      sample_min_interval_secs: 300,
      sample_failure_backoff_secs: 900,
      sample_no_new_backoff_secs: 1_800,
      sample_max_nodes: 16_384,
      sample_cooldown_capacity: 100_000,
      hash_dedup_capacity: 100_000,
      hash_dedup_ttl_secs: 900,
      get_peers_hashes_per_second: 8,
      get_peers_burst: 16,
      sample_hashes_per_second: 16,
      sample_hash_burst: 64,
    },
    peer_lookup: {},
    crawl: {
      rate_limit: {},
      pool: {},
      bootstrap: {},
      target: {},
      scheduler: {},
    },
  };
}

export function defaultIndexerConfig(): IndexerWorkerConfig {
  return {
    torrent_workers: 64,
    hot_workers: 4,
    read_block_ms: 5_000,
    read_batch_size: 512,
    insert_workers: 64,
    batch_size: 1_000,
    flush_interval_ms: 5_000,
    channel_buffer_size: 2_000,
    hot_channel_buffer_size: 10_000,
    xautoclaim_interval_secs: 30,
    xautoclaim_min_idle_ms: 120_000,
    max_delivery_attempts: 5,
    log_level: "info",
  };
}

/** 清理未设置的 Optional 子字段（null / undefined / NaN / 空数组），保持 JSON 整洁且兼容 deny_unknown_fields */
export function normalizeCrawlerConfigPayload(cfg: CrawlerWorkerConfig): CrawlerWorkerConfig {
  const cleanOptObj = <T extends Record<string, unknown>>(obj: T | undefined): T => {
    const out: Record<string, unknown> = {};
    if (!obj) return out as T;
    for (const [k, v] of Object.entries(obj)) {
      if (v === null || v === undefined || v === "") continue;
      if (typeof v === "number" && Number.isNaN(v)) continue;
      out[k] = v;
    }
    return out as T;
  };

  return {
    port: cfg.port,
    netmode: cfg.netmode,
    crawl_profile: cfg.crawl_profile,
    log_level: cfg.log_level,
    metadata_cache_capacity: cfg.metadata_cache_capacity,
    push_concurrency: cfg.push_concurrency,
    metadata_pending_ttl_secs: cfg.metadata_pending_ttl_secs,
    metadata_failure_backoff_secs: cfg.metadata_failure_backoff_secs,
    hash_queue_capacity: cfg.hash_queue_capacity,
    metadata: { ...cfg.metadata },
    discovery: { ...cfg.discovery },
    peer_lookup: cleanOptObj(cfg.peer_lookup as Record<string, unknown>) as CrawlerPeerLookupConfig,
    crawl: {
      rate_limit: cleanOptObj(cfg.crawl?.rate_limit as Record<string, unknown>) as CrawlerRateLimitConfig,
      pool: cleanOptObj(cfg.crawl?.pool as Record<string, unknown>) as CrawlerPoolConfig,
      bootstrap: cleanOptObj(cfg.crawl?.bootstrap as Record<string, unknown>) as CrawlerBootstrapConfig,
      target: cleanOptObj(cfg.crawl?.target as Record<string, unknown>) as CrawlerTargetConfig,
      scheduler: cleanOptObj(cfg.crawl?.scheduler as Record<string, unknown>) as CrawlerSchedulerConfig,
    },
  };
}

export function getDhtProfiles(): Promise<DhtProfilesResponse> {
  return apiRequest<DhtProfilesResponse>("/api/v1/dht/profiles");
}

export function listWorkers(): Promise<WorkerSummaryItem[]> {
  return apiRequest<WorkerSummaryItem[]>("/api/v1/workers");
}

export function createWorker(payload: CreateWorkerPayload): Promise<{
  id: string;
  role: WorkerRole;
  display_name: string;
  version: number;
}> {
  return apiRequest("/api/v1/workers", {
    method: "POST",
    body: JSON.stringify(payload),
  });
}

export function deleteWorker(id: string): Promise<{ ok: boolean }> {
  return apiRequest(`/api/v1/workers/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
}

export function startWorker(id: string): Promise<{ ok: boolean }> {
  return apiRequest(`/api/v1/workers/${encodeURIComponent(id)}/start`, {
    method: "POST",
  });
}

export function stopWorker(id: string): Promise<{ ok: boolean }> {
  return apiRequest(`/api/v1/workers/${encodeURIComponent(id)}/stop`, {
    method: "POST",
  });
}

export function restartWorker(id: string): Promise<{ ok: boolean }> {
  return apiRequest(`/api/v1/workers/${encodeURIComponent(id)}/restart`, {
    method: "POST",
  });
}

export function updateWorkerPolicy(
  id: string,
  payload: UpdateWorkerPolicyPayload,
): Promise<{ ok: boolean }> {
  return apiRequest(`/api/v1/workers/${encodeURIComponent(id)}/policy`, {
    method: "PUT",
    body: JSON.stringify(payload),
  });
}

export function getWorkerConfig(id: string): Promise<WorkerConfigDetailResponse> {
  return apiRequest<WorkerConfigDetailResponse>(
    `/api/v1/workers/${encodeURIComponent(id)}/config`,
  );
}

export function updateWorkerConfig(
  id: string,
  payload: {
    config: CrawlerWorkerConfig | IndexerWorkerConfig;
    note: string;
    apply_restart: boolean;
  },
): Promise<UpdateWorkerConfigResponse> {
  return apiRequest<UpdateWorkerConfigResponse>(
    `/api/v1/workers/${encodeURIComponent(id)}/config`,
    {
      method: "PUT",
      body: JSON.stringify(payload),
    },
  );
}

export function listWorkerConfigVersions(
  id: string,
  limit = 20,
): Promise<WorkerConfigVersionItem[]> {
  return apiRequest<WorkerConfigVersionItem[]>(
    `/api/v1/workers/${encodeURIComponent(id)}/config/versions?limit=${limit}`,
  );
}

export function rollbackWorkerConfig(
  id: string,
  payload: { target_version: number; apply_restart: boolean },
): Promise<UpdateWorkerConfigResponse> {
  return apiRequest<UpdateWorkerConfigResponse>(
    `/api/v1/workers/${encodeURIComponent(id)}/config/rollback`,
    {
      method: "POST",
      body: JSON.stringify(payload),
    },
  );
}
