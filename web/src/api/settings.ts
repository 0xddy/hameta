import { apiRequest } from "./client";
import type {
  ManticoreServiceConfig,
  ManticoreTestReport,
  RedisServiceConfig,
  RedisTestReport,
  ServiceDiscoveryReport,
} from "./setup";

export interface AffectedWorkerItem {
  id: string;
  display_name: string;
  role: "crawler" | "indexer";
  runtime_state: string;
}

export interface WorkerRestartOutcome {
  id: string;
  role: "crawler" | "indexer";
  ok: boolean;
  message: string;
}

export interface ServiceConfigDetail<T> {
  version: number;
  config: T;
  last_test_ok: boolean | null;
  last_test_message: string | null;
  last_tested_at: number | null;
  updated_by: string;
  note: string;
  updated_at: number;
  affected_running_workers: AffectedWorkerItem[];
}

export interface ServicesOverviewResponse {
  redis: ServiceConfigDetail<RedisServiceConfig>;
  manticore: ServiceConfigDetail<ManticoreServiceConfig>;
}

export interface UpdateServiceResponse<T> {
  version: number;
  config: T;
  restarted_workers: WorkerRestartOutcome[];
}

export interface ServiceHistoryItem {
  id: number;
  kind: "redis" | "manticore";
  version: number;
  config: Record<string, unknown>;
  is_active: boolean;
  last_test_ok: boolean | null;
  last_test_message: string | null;
  last_tested_at: number | null;
  created_by: string;
  note: string;
  created_at: number;
}

export interface RollbackServiceResponse {
  kind: "redis" | "manticore";
  version: number;
  config: Record<string, unknown>;
  restarted_workers: WorkerRestartOutcome[];
}

export function getServicesSettings(): Promise<ServicesOverviewResponse> {
  return apiRequest("/api/v1/settings/services");
}

export function detectSettingsServices(): Promise<ServiceDiscoveryReport> {
  return apiRequest("/api/v1/settings/detect-services");
}

export function testRedisSettings(config?: RedisServiceConfig): Promise<RedisTestReport> {
  return apiRequest("/api/v1/settings/redis/test", {
    method: "POST",
    body: JSON.stringify({ config: config ?? null }),
  });
}

export function updateRedisSettings(
  config: RedisServiceConfig,
  note: string,
  restart_affected_workers: boolean,
): Promise<UpdateServiceResponse<RedisServiceConfig>> {
  return apiRequest("/api/v1/settings/redis", {
    method: "PUT",
    body: JSON.stringify({ config, note, restart_affected_workers }),
  });
}

export function testManticoreSettings(
  config?: ManticoreServiceConfig,
): Promise<ManticoreTestReport> {
  return apiRequest("/api/v1/settings/manticore/test", {
    method: "POST",
    body: JSON.stringify({ config: config ?? null }),
  });
}

export function initManticoreTable(
  config?: ManticoreServiceConfig,
): Promise<{ ok: boolean; message: string }> {
  return apiRequest("/api/v1/settings/manticore/init-table", {
    method: "POST",
    body: JSON.stringify({ config: config ?? null }),
  });
}

export function updateManticoreSettings(
  config: ManticoreServiceConfig,
  note: string,
  restart_affected_workers: boolean,
): Promise<UpdateServiceResponse<ManticoreServiceConfig>> {
  return apiRequest("/api/v1/settings/manticore", {
    method: "PUT",
    body: JSON.stringify({ config, note, restart_affected_workers }),
  });
}

export function listSettingsHistory(
  kind: "redis" | "manticore",
  limit = 20,
): Promise<ServiceHistoryItem[]> {
  return apiRequest(`/api/v1/settings/history?kind=${kind}&limit=${limit}`);
}

export function rollbackSettings(
  kind: "redis" | "manticore",
  target_version: number,
  restart_affected_workers: boolean,
): Promise<RollbackServiceResponse> {
  return apiRequest("/api/v1/settings/rollback", {
    method: "POST",
    body: JSON.stringify({ kind, target_version, restart_affected_workers }),
  });
}

export const DEFAULT_GEOIP_RELEASES_URL = "https://github.com/P3TERX/GeoLite.mmdb/releases";
export const DEFAULT_GEOIP_DOWNLOAD_URL =
  "https://github.com/P3TERX/GeoLite.mmdb/releases/latest/download/GeoLite2-Country.mmdb";

export interface GeoIpConfig {
  enabled: boolean;
  mmdb_path: string;
  download_url: string;
  home_country: string;
  prefix_cache_slots: number;
  adaptive_sample_threshold_pps: number;
  sample_queue_capacity: number;
}

export function defaultGeoIpConfig(): GeoIpConfig {
  return {
    enabled: true,
    mmdb_path: "GeoLite2-Country.mmdb",
    download_url: DEFAULT_GEOIP_DOWNLOAD_URL,
    home_country: "CN",
    prefix_cache_slots: 65_536,
    adaptive_sample_threshold_pps: 30_000,
    sample_queue_capacity: 8_192,
  };
}

export interface GeoIpInspectSummary {
  resolved_path: string;
  mmdb_exists: boolean;
  mmdb_valid: boolean;
  mmdb_size_bytes: number;
  mmdb_build_epoch: number | null;
  database_type: string | null;
  node_count: number | null;
  status_message: string;
}

export interface GeoIpSettingsResponse {
  config: GeoIpConfig;
  resolved_path: string;
  mmdb_exists: boolean;
  mmdb_valid: boolean;
  mmdb_size_bytes: number;
  mmdb_build_epoch: number | null;
  database_type: string | null;
  node_count: number | null;
  status_message: string;
  running_crawlers: AffectedWorkerItem[];
}

export interface CrawlerGeoIpReloadResult {
  instance_id: string;
  ok: boolean;
  mmdb_loaded: boolean;
  error: string | null;
  message: string;
}

export interface UpdateGeoIpResponse {
  config: GeoIpConfig;
  inspect: GeoIpInspectSummary;
  reload_results: CrawlerGeoIpReloadResult[];
}

export interface ReloadGeoIpResponse {
  config: GeoIpConfig;
  inspect: GeoIpInspectSummary;
  reload_results: CrawlerGeoIpReloadResult[];
}

export interface DownloadGeoIpResponse {
  config: GeoIpConfig;
  inspect: GeoIpInspectSummary;
  reload_results: CrawlerGeoIpReloadResult[];
  downloaded_bytes: number;
  duration_ms: number;
  source_url: string;
  release_tag: string | null;
  previous_build_epoch: number | null;
  changed: boolean;
  message: string;
}

export function getGeoIpSettings(): Promise<GeoIpSettingsResponse> {
  return apiRequest<GeoIpSettingsResponse>("/api/v1/settings/geoip");
}

export function updateGeoIpSettings(
  config: GeoIpConfig,
  reload_crawlers = true,
): Promise<UpdateGeoIpResponse> {
  return apiRequest<UpdateGeoIpResponse>("/api/v1/settings/geoip", {
    method: "PUT",
    body: JSON.stringify({ config, reload_crawlers }),
  });
}

export function reloadGeoIpSettings(): Promise<ReloadGeoIpResponse> {
  return apiRequest<ReloadGeoIpResponse>("/api/v1/settings/geoip/reload", {
    method: "POST",
  });
}

export function downloadGeoIpMmdb(params?: {
  mmdb_path?: string;
  download_url?: string;
  reload_crawlers?: boolean;
}): Promise<DownloadGeoIpResponse> {
  return apiRequest<DownloadGeoIpResponse>("/api/v1/settings/geoip/download", {
    method: "POST",
    body: JSON.stringify({
      mmdb_path: params?.mmdb_path,
      download_url: params?.download_url,
      reload_crawlers: params?.reload_crawlers ?? true,
    }),
  });
}

export interface SavedNofileConfig {
  process_soft: number;
  process_hard: number;
  system_file_max: number | null;
  system_nr_open: number | null;
  updated_at: number;
  updated_by: string;
}

export interface NofilePresetItem {
  id: string;
  label: string;
  badge: string;
  process_soft: number;
  process_hard: number;
  system_file_max: number;
  system_nr_open: number;
  description: string;
}

export interface WorkerNofileInfo {
  id: string;
  display_name: string;
  role: "crawler" | "indexer";
  pid: number;
  soft_limit: number | null;
  hard_limit: number | null;
  open_fds: number | null;
}

export interface NofileStatusResponse {
  platform: string;
  supported: boolean;
  system_supported: boolean;
  is_root: boolean;
  in_container: boolean;
  can_write_sysctl: boolean;
  can_write_limits_d: boolean;
  panel_pid: number;
  process_soft: number | null;
  process_hard: number | null;
  process_open_fds: number | null;
  system_file_max: number | null;
  system_nr_open: number | null;
  system_allocated_fds: number | null;
  sysctl_conf_path: string | null;
  limits_conf_path: string | null;
  sysctl_conf_exists: boolean;
  limits_conf_exists: boolean;
  saved_config: SavedNofileConfig | null;
  running_workers: WorkerNofileInfo[];
  recommended_presets: NofilePresetItem[];
}

export interface UpdateNofileRequest {
  apply_process: boolean;
  process_soft?: number | null;
  process_hard?: number | null;
  apply_to_workers: boolean;
  persist_app_config: boolean;
  apply_system: boolean;
  system_file_max?: number | null;
  system_nr_open?: number | null;
  persist_system_files: boolean;
}

export interface WorkerNofileApplyResult {
  id: string;
  role: "crawler" | "indexer";
  pid: number;
  ok: boolean;
  soft_limit: number | null;
  hard_limit: number | null;
  message: string;
}

export interface UpdateNofileResponse {
  ok: boolean;
  process_applied: boolean;
  process_message: string;
  system_applied: boolean;
  system_message: string;
  persisted_app_config: boolean;
  persisted_files: string[];
  warnings: string[];
  manual_commands: string[];
  worker_results: WorkerNofileApplyResult[];
  status: NofileStatusResponse;
}

export function getNofileSettings(): Promise<NofileStatusResponse> {
  return apiRequest<NofileStatusResponse>("/api/v1/settings/nofile");
}

export function updateNofileSettings(payload: UpdateNofileRequest): Promise<UpdateNofileResponse> {
  return apiRequest<UpdateNofileResponse>("/api/v1/settings/nofile", {
    method: "PUT",
    body: JSON.stringify(payload),
  });
}
