import { apiRequest } from "./client";
import type {
  ManticoreServiceConfig,
  ManticoreTestReport,
  RedisServiceConfig,
  RedisTestReport,
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
