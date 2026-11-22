import { apiRequest } from "./client";

export interface TorrentFileInfo {
  path: string;
  size: number;
}

export interface TorrentSummary {
  id: number;
  info_hash: string;
  magnet_link: string;
  raw_name: string;
  refine_name: string;
  file_count: number;
  total_size: number;
  hot: number;
  created_at: number;
  updated_at: number;
}

export interface TorrentDetail extends TorrentSummary {
  files: TorrentFileInfo[];
}

export interface ListTorrentsQuery {
  q?: string;
  sort_by?: "updated_at" | "created_at" | "hot" | "file_count";
  sort_dir?: "desc" | "asc";
  min_hot?: number;
  max_hot?: number;
  min_files?: number;
  max_files?: number;
  since_ts?: number;
  until_ts?: number;
  empty_refine_only?: boolean;
  page?: number;
  page_size?: number;
}

export interface ListTorrentsResponse {
  items: TorrentSummary[];
  total: number;
  total_relation: string;
  page: number;
  page_size: number;
  took_ms: number;
}

export interface CreateTorrentPayload {
  info_hash: string;
  raw_name: string;
  refine_name?: string;
  files: TorrentFileInfo[];
  hot: number;
  sync_redis_indexed: boolean;
}

export interface UpdateTorrentPayload {
  raw_name?: string;
  refine_name?: string;
  hot?: number;
  files?: TorrentFileInfo[];
}

export interface DeleteTorrentResponse {
  ok: boolean;
  deleted_id: number;
  info_hash: string;
  keep_redis_indexed: boolean;
}

export interface BatchDeleteResponse {
  ok: boolean;
  deleted: number;
  redis_synced: number;
  keep_redis_indexed: boolean;
}

export function listTorrents(query: ListTorrentsQuery): Promise<ListTorrentsResponse> {
  const params = new URLSearchParams();
  if (query.q !== undefined && query.q.trim() !== "") params.set("q", query.q.trim());
  if (query.sort_by) params.set("sort_by", query.sort_by);
  if (query.sort_dir) params.set("sort_dir", query.sort_dir);
  if (query.min_hot !== undefined && !Number.isNaN(query.min_hot)) {
    params.set("min_hot", String(query.min_hot));
  }
  if (query.max_hot !== undefined && !Number.isNaN(query.max_hot)) {
    params.set("max_hot", String(query.max_hot));
  }
  if (query.min_files !== undefined && !Number.isNaN(query.min_files)) {
    params.set("min_files", String(query.min_files));
  }
  if (query.max_files !== undefined && !Number.isNaN(query.max_files)) {
    params.set("max_files", String(query.max_files));
  }
  if (query.since_ts !== undefined) params.set("since_ts", String(query.since_ts));
  if (query.until_ts !== undefined) params.set("until_ts", String(query.until_ts));
  if (query.empty_refine_only) params.set("empty_refine_only", "true");
  if (query.page !== undefined) params.set("page", String(query.page));
  if (query.page_size !== undefined) params.set("page_size", String(query.page_size));
  const qs = params.toString();
  return apiRequest<ListTorrentsResponse>(`/api/v1/torrents${qs ? `?${qs}` : ""}`);
}

export function getTorrentDetail(idOrHash: string | number): Promise<TorrentDetail> {
  return apiRequest<TorrentDetail>(`/api/v1/torrents/${encodeURIComponent(String(idOrHash))}`);
}

export function createTorrent(payload: CreateTorrentPayload): Promise<TorrentDetail> {
  return apiRequest<TorrentDetail>("/api/v1/torrents", {
    method: "POST",
    body: JSON.stringify(payload),
  });
}

export function updateTorrent(
  idOrHash: string | number,
  payload: UpdateTorrentPayload,
): Promise<TorrentDetail> {
  return apiRequest<TorrentDetail>(`/api/v1/torrents/${encodeURIComponent(String(idOrHash))}`, {
    method: "PATCH",
    body: JSON.stringify(payload),
  });
}

export function deleteTorrent(
  idOrHash: string | number,
  keepRedisIndexed = true,
): Promise<DeleteTorrentResponse> {
  return apiRequest<DeleteTorrentResponse>(
    `/api/v1/torrents/${encodeURIComponent(String(idOrHash))}?keep_redis_indexed=${keepRedisIndexed ? "true" : "false"}`,
    { method: "DELETE" },
  );
}

export function batchDeleteTorrents(
  ids: number[],
  keepRedisIndexed = true,
): Promise<BatchDeleteResponse> {
  return apiRequest<BatchDeleteResponse>("/api/v1/torrents/batch-delete", {
    method: "POST",
    body: JSON.stringify({ ids, keep_redis_indexed: keepRedisIndexed }),
  });
}
