import { apiRequest } from "./client";

export interface RedisServiceConfig {
  url: string;
  host: string;
  port: number;
  username: string;
  password: string;
  db: number;
  tls: boolean;
  conn_timeout_secs: number;
  wait_timeout_secs: number;
  recycle_timeout_secs: number;
  crawler_pool_size: number;
  indexer_pool_size: number;
  indexed_key_prefix: string;
  torrent_stream: string;
  hot_stream: string;
  consumer_group: string;
  max_stream_len: number;
  max_message_size: number;
}

export type ManticoreAuthMode = "none" | "basic" | "bearer" | "header";

export interface ManticoreServiceConfig {
  url: string;
  auth_mode: ManticoreAuthMode;
  username: string;
  password: string;
  bearer_token: string;
  header_name: string;
  header_value: string;
  table_name: string;
  request_timeout_secs: number;
  max_connections: number;
}

export function defaultRedisConfig(): RedisServiceConfig {
  return {
    url: "",
    host: "127.0.0.1",
    port: 6379,
    username: "",
    password: "",
    db: 0,
    tls: false,
    conn_timeout_secs: 3,
    wait_timeout_secs: 2,
    recycle_timeout_secs: 3,
    crawler_pool_size: 100,
    indexer_pool_size: 128,
    indexed_key_prefix: "metadata:indexed:",
    torrent_stream: "hameta:stream:torrent:new",
    hot_stream: "hameta:stream:torrent:hot",
    consumer_group: "hameta:cg:indexer",
    max_stream_len: 3_000_000,
    max_message_size: 1_048_576,
  };
}

export function defaultManticoreConfig(): ManticoreServiceConfig {
  return {
    url: "http://127.0.0.1:9308",
    auth_mode: "none",
    username: "",
    password: "",
    bearer_token: "",
    header_name: "X-API-Key",
    header_value: "",
    table_name: "torrents",
    request_timeout_secs: 10,
    max_connections: 1280,
  };
}

export function normalizeManticoreConfig(
  raw?: Partial<ManticoreServiceConfig> | null,
): ManticoreServiceConfig {
  const base = defaultManticoreConfig();
  if (!raw) return base;
  const merged: ManticoreServiceConfig = {
    ...base,
    ...raw,
    auth_mode: raw.auth_mode ?? base.auth_mode,
    username: raw.username ?? "",
    password: raw.password ?? "",
    bearer_token: raw.bearer_token ?? "",
    header_name: raw.header_name ?? "X-API-Key",
    header_value: raw.header_value ?? "",
    table_name: raw.table_name || "torrents",
    request_timeout_secs: raw.request_timeout_secs ?? base.request_timeout_secs,
    max_connections: raw.max_connections ?? base.max_connections,
  };
  if (merged.auth_mode === "none" && (merged.username.trim() !== "" || merged.password !== "")) {
    merged.auth_mode = "basic";
  }
  return merged;
}

export interface SetupStatusResponse {
  initialized: boolean;
  has_admin: boolean;
  has_redis: boolean;
  has_manticore: boolean;
  version: string;
}

export interface RedisTestReport {
  ok: boolean;
  latency_ms: number;
  redis_version: string | null;
  streams_supported: boolean;
  xautoclaim_supported: boolean;
  used_memory_human: string | null;
  dbsize: number | null;
  rw_probe_ok: boolean;
  message: string;
}

export interface ColumnSchemaInfo {
  field: string;
  column_type: string;
  properties: string;
}

export interface SchemaCheckReport {
  table_exists: boolean;
  compatible: boolean;
  columns: ColumnSchemaInfo[];
  missing_columns: string[];
  message: string;
}

export interface TableStatusInfo {
  indexed_documents: number | null;
  disk_bytes: number | null;
  ram_bytes: number | null;
  raw_rows: Record<string, string>;
}

export interface ManticoreTestReport {
  ok: boolean;
  latency_ms: number;
  schema: SchemaCheckReport | null;
  status: TableStatusInfo | null;
  message: string;
}

export interface SetupInitRequest {
  admin_username: string;
  admin_password: string;
  redis: RedisServiceConfig;
  manticore: ManticoreServiceConfig;
  legacy_filter_json?: string | null;
  create_default_workers: boolean;
  crawler_port?: number;
  crawler_profile?: string;
}

export interface SetupInitResponse {
  initialized: boolean;
  username: string;
  filter_version: number;
  created_workers: string[];
}

export interface DetectedRedisService {
  source: string;
  label: string;
  host: string;
  port: number;
  container_name: string | null;
  container_image: string | null;
  reachable: boolean;
  requires_auth: boolean;
  version: string | null;
  latency_ms: number | null;
}

export interface DetectedManticoreService {
  source: string;
  label: string;
  url: string;
  container_name: string | null;
  container_image: string | null;
  reachable: boolean;
  requires_auth: boolean;
  has_torrents_table: boolean | null;
  latency_ms: number | null;
}

export interface ServiceDiscoveryReport {
  docker_available: boolean;
  redis: DetectedRedisService[];
  manticore: DetectedManticoreService[];
}

export function getSetupStatus(): Promise<SetupStatusResponse> {
  return apiRequest("/api/v1/setup/status");
}

export function detectSetupServices(): Promise<ServiceDiscoveryReport> {
  return apiRequest("/api/v1/setup/detect-services");
}

export function testSetupRedis(redis: RedisServiceConfig): Promise<RedisTestReport> {
  return apiRequest("/api/v1/setup/test-redis", {
    method: "POST",
    body: JSON.stringify({ redis }),
  });
}

export function testSetupManticore(
  manticore: ManticoreServiceConfig,
): Promise<ManticoreTestReport> {
  return apiRequest("/api/v1/setup/test-manticore", {
    method: "POST",
    body: JSON.stringify({ manticore }),
  });
}

export function initSetupManticoreTable(
  manticore: ManticoreServiceConfig,
): Promise<{ ok: boolean; message: string }> {
  return apiRequest("/api/v1/setup/init-manticore-table", {
    method: "POST",
    body: JSON.stringify({ manticore }),
  });
}

export function submitSetupInit(req: SetupInitRequest): Promise<SetupInitResponse> {
  return apiRequest("/api/v1/setup/init", {
    method: "POST",
    body: JSON.stringify(req),
  });
}
