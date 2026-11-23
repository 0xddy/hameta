import { apiRequest } from "./client";

export type IngestEventType = "indexed" | "hot_updated";

export interface ConsoleFilePreview {
  path: string;
  size: number;
}

export interface ConsoleIngestEvent {
  seq: number;
  info_hash: string;
  event_type: IngestEventType;
  worker_id: string;
  manticore_id: number;
  batch_id: number;
  raw_name: string;
  refine_name: string;
  cleaned: boolean;
  total_size: number;
  file_count: number;
  files_preview: ConsoleFilePreview[];
  hot: number;
  hot_delta: number;
  latency_ms: number;
  updated_at: number;
}

export interface IngestHubStats {
  latest_seq: number;
  ring_len: number;
  ring_capacity: number;
  subscribers: number;
  total_published: number;
}

export interface ConsoleRecentResponse {
  stats: IngestHubStats;
  events: ConsoleIngestEvent[];
}

export interface ConsoleStreamQueryParams {
  limit?: number;
  replay?: number;
  event_type?: string;
  worker_id?: string;
  max_eps?: number;
}

export function getConsoleRecent(params?: ConsoleStreamQueryParams): Promise<ConsoleRecentResponse> {
  const qs = new URLSearchParams();
  if (params?.limit) qs.set("limit", String(params.limit));
  if (params?.event_type && params.event_type !== "all") {
    qs.set("event_type", params.event_type);
  }
  if (params?.worker_id && params.worker_id !== "all") {
    qs.set("worker_id", params.worker_id);
  }
  const q = qs.toString();
  return apiRequest<ConsoleRecentResponse>(`/api/v1/console/recent${q ? `?${q}` : ""}`);
}

export function buildConsoleStreamUrl(params?: ConsoleStreamQueryParams): string {
  const qs = new URLSearchParams();
  if (params?.replay !== undefined) qs.set("replay", String(params.replay));
  if (params?.event_type && params.event_type !== "all") {
    qs.set("event_type", params.event_type);
  }
  if (params?.worker_id && params.worker_id !== "all") {
    qs.set("worker_id", params.worker_id);
  }
  if (params?.max_eps && params.max_eps > 0) {
    qs.set("max_eps", String(params.max_eps));
  }
  const q = qs.toString();
  return `/api/v1/console/ingest-stream${q ? `?${q}` : ""}`;
}
