import { apiRequest } from "./client";

export interface RedisPoolStatusDto {
  max_size: number;
  size: number;
  available: number;
  waiting: number;
}

export interface RedisConfigSummaryDto {
  torrent_stream: string;
  hot_stream: string;
  consumer_group: string;
  indexed_key_prefix: string;
  max_stream_len: number;
  max_message_size: number;
}

export interface RedisStreamConsumerDto {
  name: string;
  pending: number;
  idle_ms: number;
}

export interface RedisStreamGroupDto {
  name: string;
  consumers: number;
  pending: number;
  lag: number | null;
  last_delivered_id: string | null;
  entries_read: number | null;
}

export interface RedisStreamOverviewDto {
  role: "new" | "hot";
  stream_name: string;
  exists: boolean;
  length: number;
  entries_added: number | null;
  first_entry_id: string | null;
  last_entry_id: string | null;
  groups: RedisStreamGroupDto[];
  target_group_consumers: RedisStreamConsumerDto[];
}

export interface RedisOverviewResponse {
  ok: boolean;
  message: string | null;
  latency_ms: number;
  redis_version: string | null;
  uptime_secs: number | null;
  used_memory_bytes: number | null;
  used_memory_human: string | null;
  used_memory_peak_human: string | null;
  connected_clients: number | null;
  instantaneous_ops_per_sec: number | null;
  dbsize: number | null;
  pool: RedisPoolStatusDto;
  config_summary: RedisConfigSummaryDto;
  streams: RedisStreamOverviewDto[];
}

export interface StreamEntryPreviewDto {
  entry_id: string;
  info_hash: string | null;
  timestamp: string | null;
  name: string | null;
  total_size: number | null;
  file_count: number | null;
  payload_bytes: number;
  fields: Record<string, string>;
}

export interface PreviewStreamResponse {
  stream_name: string;
  role: "new" | "hot";
  total_len: number;
  items: StreamEntryPreviewDto[];
}

export interface TrimStreamResponse {
  ok: boolean;
  stream_name: string;
  trimmed_count: number;
  remaining_len: number;
}

export interface RedisKeyStateDto {
  key: string;
  exists: boolean;
  value: string | null;
  pttl_ms: number;
}

export interface RedisHashInspectResponse {
  info_hash: string;
  expected_id: number;
  indexed: RedisKeyStateDto;
  pending: RedisKeyStateDto;
  rejected: RedisKeyStateDto;
  manticore_checked: boolean;
  manticore_exists: boolean;
  manticore_raw_name: string | null;
  manticore_refine_name: string | null;
  manticore_hot: number | null;
  manticore_updated_at: number | null;
}

export function getRedisOverview(): Promise<RedisOverviewResponse> {
  return apiRequest<RedisOverviewResponse>("/api/v1/redis/overview");
}

export function previewRedisStream(name: string, count = 20): Promise<PreviewStreamResponse> {
  return apiRequest<PreviewStreamResponse>(
    `/api/v1/redis/streams/${encodeURIComponent(name)}/preview?count=${count}`,
  );
}

export function trimRedisStream(
  name: string,
  payload: { max_len: number; approximate: boolean; confirm_stream_name: string },
): Promise<TrimStreamResponse> {
  return apiRequest<TrimStreamResponse>(
    `/api/v1/redis/streams/${encodeURIComponent(name)}/trim`,
    {
      method: "POST",
      body: JSON.stringify(payload),
    },
  );
}

export function inspectRedisHash(infoHash: string): Promise<RedisHashInspectResponse> {
  return apiRequest<RedisHashInspectResponse>(
    `/api/v1/redis/hash/${encodeURIComponent(infoHash.trim())}`,
  );
}

export function clearRedisHashLocks(
  infoHash: string,
  opts: { clear_pending: boolean; clear_rejected: boolean; clear_indexed: boolean },
): Promise<{
  ok: boolean;
  info_hash: string;
  deleted_keys: string[];
  deleted_count: number;
}> {
  const qs = new URLSearchParams({
    clear_pending: opts.clear_pending ? "true" : "false",
    clear_rejected: opts.clear_rejected ? "true" : "false",
    clear_indexed: opts.clear_indexed ? "true" : "false",
  }).toString();
  return apiRequest(`/api/v1/redis/hash/${encodeURIComponent(infoHash.trim())}/locks?${qs}`, {
    method: "DELETE",
  });
}
