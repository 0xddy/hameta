import { apiRequest } from "./client";

export interface GlobeChannelRates {
  get_peers_pps: number;
  announce_peer_pps: number;
  response_pps: number;
  find_node_ping_pps: number;
  error_pps: number;
  other_pps: number;
  parse_error_pps: number;
  invalid_udp_pps: number;
  queue_full_pps: number;
}

export interface GlobeCountryTraffic {
  iso_code: string;
  is_home: boolean;
  pps: number;
  bps: number;
  share_pct: number;
  get_peers_pps: number;
  announce_peer_pps: number;
  response_pps: number;
  find_node_ping_pps: number;
  other_pps: number;
  cumulative_packets: number;
  cumulative_bytes: number;
}

export interface GlobeGeoIpStatus {
  enabled: boolean;
  mmdb_loaded: boolean;
  mmdb_file_ready?: boolean;
  mmdb_path: string;
  mmdb_size_bytes: number;
  mmdb_build_epoch: number;
  database_type: string;
  error: string | null;
  home_country: string;
  sample_rate_divisor: number;
  adaptive_sample_threshold_pps: number;
  cache_hit_ratio: number;
  avg_lookup_ns: number;
  samples_dropped_pps: number;
  unknown_pps: number;
}

export interface GlobeTrafficSnapshot {
  seq: number;
  captured_at: number;
  running_crawlers: number;
  udp_rx_pps: number;
  udp_tx_pps: number;
  udp_rx_bps: number;
  udp_tx_bps: number;
  ipv4_pps: number;
  ipv6_pps: number;
  channels: GlobeChannelRates;
  geoip: GlobeGeoIpStatus;
  countries: GlobeCountryTraffic[];
  other_countries_pps: number;
  other_countries_bps: number;
  active_country_count: number;
}

export const GLOBE_STREAM_URL = "/api/v1/globe/stream";

export function getGlobeSnapshot(): Promise<GlobeTrafficSnapshot> {
  return apiRequest<GlobeTrafficSnapshot>("/api/v1/globe/snapshot");
}
