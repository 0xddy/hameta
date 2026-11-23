use arc_swap::{ArcSwap, ArcSwapOption};
use dht_crawler::{InboundDhtKind, InboundDhtSample};
use hameta_common::{GeoIpConfig, WorkerReloadGeoIpResponse};
use maxminddb::{Mmap, PathElement, Reader};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

const COUNTRY_ISO_PATH: [PathElement<'static>; 2] =
    [PathElement::Key("country"), PathElement::Key("iso_code")];
const REGISTERED_COUNTRY_ISO_PATH: [PathElement<'static>; 2] = [
    PathElement::Key("registered_country"),
    PathElement::Key("iso_code"),
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeoCountryTotals {
    pub packets: u64,
    pub bytes: u64,
    pub get_peers: u64,
    pub announce_peer: u64,
    pub response: u64,
    pub find_node_ping: u64,
    pub other: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GeoTrafficSnapshot {
    pub enabled: bool,
    pub mmdb_loaded: bool,
    pub mmdb_path: String,
    pub mmdb_size_bytes: u64,
    pub mmdb_build_epoch: u64,
    pub database_type: String,
    pub error: Option<String>,
    pub home_country: String,
    pub adaptive_sample_threshold_pps: u64,
    pub sample_rate_divisor: u16,
    pub samples_processed: u64,
    pub samples_dropped: u64,
    pub unknown_packets: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_hit_ratio: f64,
    pub avg_lookup_ns: u64,
    pub countries: BTreeMap<String, GeoCountryTotals>,
}

struct LoadedMmdb {
    reader: Reader<Mmap>,
}

#[derive(Debug, Clone)]
struct GeoMetaView {
    mmdb_path: String,
    mmdb_size_bytes: u64,
    mmdb_build_epoch: u64,
    database_type: String,
    error: Option<String>,
    home_country: String,
}

#[derive(Debug, Clone, Copy)]
struct GeoPacketSample {
    ip: IpAddr,
    bytes: u16,
    kind: InboundDhtKind,
    weight: u16,
}

#[derive(Debug, Clone, Copy, Default)]
struct PrefixCacheSlot {
    tag: u64,
    generation: u32,
    iso: [u8; 2],
}

#[derive(Debug, Default)]
struct GeoWorkerCounters {
    samples_processed: u64,
    unknown_packets: u64,
    cache_hits: u64,
    cache_misses: u64,
    lookup_ns_total: u64,
    lookup_count: u64,
    countries: HashMap<[u8; 2], GeoCountryTotals>,
}

/// 进程内 GeoIP 离线归因引擎：
/// - `mmap` 只读加载 `GeoLite2-Country.mmdb`，多 Crawler 进程共享只读物理页；
/// - 观测入口 `observe_packet` 仅执行零分配非阻塞 `try_send`，满队列静默丢弃；
/// - 单一后台协程独享直映前缀缓存（IPv4 `/24` + IPv6 `/48`）并周期性发布无锁快照。
pub struct GeoIpEngine {
    enabled: AtomicBool,
    reader: ArcSwapOption<LoadedMmdb>,
    meta: ArcSwap<GeoMetaView>,
    snapshot: ArcSwap<GeoTrafficSnapshot>,
    sample_shift: AtomicU8,
    ingress_seq: AtomicU64,
    samples_dropped: AtomicU64,
    cache_generation: AtomicU32,
    cache_slots: usize,
    adaptive_threshold_pps: AtomicU64,
    sample_tx: mpsc::Sender<GeoPacketSample>,
    sample_rx: Mutex<Option<mpsc::Receiver<GeoPacketSample>>>,
}

impl GeoIpEngine {
    pub fn new(config: &GeoIpConfig, data_dir: &Path) -> Arc<Self> {
        let resolved_path = config.resolve_mmdb_path(data_dir);
        let cache_slots = config.normalized_prefix_cache_slots();
        let queue_cap = config.sample_queue_capacity.clamp(512, 131_072);
        let (sample_tx, sample_rx) = mpsc::channel(queue_cap);

        let (loaded, meta_view) = load_mmdb_state(config, &resolved_path);
        let initial_snap = GeoTrafficSnapshot {
            enabled: config.enabled,
            mmdb_loaded: loaded.is_some(),
            mmdb_path: meta_view.mmdb_path.clone(),
            mmdb_size_bytes: meta_view.mmdb_size_bytes,
            mmdb_build_epoch: meta_view.mmdb_build_epoch,
            database_type: meta_view.database_type.clone(),
            error: meta_view.error.clone(),
            home_country: meta_view.home_country.clone(),
            adaptive_sample_threshold_pps: config.adaptive_sample_threshold_pps,
            sample_rate_divisor: 1,
            ..Default::default()
        };

        Arc::new(Self {
            enabled: AtomicBool::new(config.enabled),
            reader: ArcSwapOption::from(loaded.map(Arc::new)),
            meta: ArcSwap::from_pointee(meta_view),
            snapshot: ArcSwap::from_pointee(initial_snap),
            sample_shift: AtomicU8::new(0),
            ingress_seq: AtomicU64::new(0),
            samples_dropped: AtomicU64::new(0),
            cache_generation: AtomicU32::new(1),
            cache_slots,
            adaptive_threshold_pps: AtomicU64::new(config.adaptive_sample_threshold_pps.max(1_000)),
            sample_tx,
            sample_rx: Mutex::new(Some(sample_rx)),
        })
    }

    pub fn snapshot(&self) -> GeoTrafficSnapshot {
        self.snapshot.load_full().as_ref().clone()
    }

    pub fn reload(&self, config: &GeoIpConfig, data_dir: &Path) -> WorkerReloadGeoIpResponse {
        let resolved_path = config.resolve_mmdb_path(data_dir);
        let (loaded, meta_view) = load_mmdb_state(config, &resolved_path);

        self.enabled.store(config.enabled, Ordering::Relaxed);
        self.adaptive_threshold_pps.store(
            config.adaptive_sample_threshold_pps.max(1_000),
            Ordering::Relaxed,
        );
        if !config.enabled {
            self.sample_shift.store(0, Ordering::Relaxed);
        }
        self.reader.store(loaded.map(Arc::new));
        self.meta.store(Arc::new(meta_view.clone()));

        // 通过递增代际号在 O(1) 内失效全部前缀缓存槽位
        let prev_gen = self.cache_generation.fetch_add(1, Ordering::AcqRel);
        if prev_gen == u32::MAX {
            self.cache_generation.store(1, Ordering::Release);
        }

        let current = self.snapshot.load_full();
        let mut next = (*current).clone();
        next.enabled = config.enabled;
        next.mmdb_loaded = self.reader.load().is_some();
        next.mmdb_path = meta_view.mmdb_path.clone();
        next.mmdb_size_bytes = meta_view.mmdb_size_bytes;
        next.mmdb_build_epoch = meta_view.mmdb_build_epoch;
        next.database_type = meta_view.database_type.clone();
        next.error = meta_view.error.clone();
        next.home_country = meta_view.home_country.clone();
        next.adaptive_sample_threshold_pps = config.adaptive_sample_threshold_pps.max(1_000);
        next.sample_rate_divisor = 1_u16 << self.sample_shift.load(Ordering::Relaxed);
        self.snapshot.store(Arc::new(next.clone()));

        WorkerReloadGeoIpResponse {
            enabled: next.enabled,
            mmdb_loaded: next.mmdb_loaded,
            mmdb_path: next.mmdb_path,
            mmdb_size_bytes: next.mmdb_size_bytes,
            mmdb_build_epoch: next.mmdb_build_epoch,
            database_type: next.database_type,
            error: next.error,
        }
    }

    #[inline]
    pub fn observe_packet(&self, sample: InboundDhtSample) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        // 未加载有效 MMDB 时快速降级跳过，避免无意义的跨协程传输
        if self.reader.load().is_none() {
            return;
        }

        let shift = self.sample_shift.load(Ordering::Relaxed);
        let weight = if shift == 0 {
            1_u16
        } else {
            let seq = self.ingress_seq.fetch_add(1, Ordering::Relaxed);
            let mask = (1_u64 << shift) - 1;
            if (seq & mask) != 0 {
                return;
            }
            1_u16 << shift
        };

        let pkt = GeoPacketSample {
            ip: sample.ip,
            bytes: sample.bytes,
            kind: sample.kind,
            weight,
        };
        if self.sample_tx.try_send(pkt).is_err() {
            self.samples_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn spawn_worker(
        self: &Arc<Self>,
        shutdown: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let engine = self.clone();
        tokio::spawn(async move {
            let Some(mut rx) = engine.sample_rx.lock().await.take() else {
                return;
            };
            let mut cache = vec![PrefixCacheSlot::default(); engine.cache_slots];
            let mut counters = GeoWorkerCounters::default();
            let mut publish_tick = tokio::time::interval(Duration::from_millis(500));
            publish_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            let mut rate_window_started = Instant::now();
            let mut rate_window_weighted_pkts: u64 = 0;

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        while let Ok(sample) = rx.try_recv() {
                            engine.process_sample(sample, &mut cache, &mut counters);
                        }
                        engine.publish_snapshot(&counters);
                        break;
                    }
                    maybe_sample = rx.recv() => {
                        match maybe_sample {
                            Some(sample) => {
                                rate_window_weighted_pkts = rate_window_weighted_pkts
                                    .saturating_add(u64::from(sample.weight));
                                engine.process_sample(sample, &mut cache, &mut counters);
                                while let Ok(more) = rx.try_recv() {
                                    rate_window_weighted_pkts = rate_window_weighted_pkts
                                        .saturating_add(u64::from(more.weight));
                                    engine.process_sample(more, &mut cache, &mut counters);
                                }
                            }
                            None => {
                                engine.publish_snapshot(&counters);
                                break;
                            }
                        }
                    }
                    _ = publish_tick.tick() => {
                        let elapsed = rate_window_started.elapsed();
                        if elapsed >= Duration::from_secs(1) {
                            let estimated_pps = ((rate_window_weighted_pkts as f64)
                                / elapsed.as_secs_f64().max(0.1))
                            .round() as u64;
                            let threshold = engine.adaptive_threshold_pps.load(Ordering::Relaxed);
                            let cur_shift = engine.sample_shift.load(Ordering::Relaxed);
                            let next_shift =
                                compute_next_sample_shift(cur_shift, estimated_pps, threshold);
                            if next_shift != cur_shift {
                                engine.sample_shift.store(next_shift, Ordering::Relaxed);
                            }
                            rate_window_started = Instant::now();
                            rate_window_weighted_pkts = 0;
                        }
                        engine.publish_snapshot(&counters);
                    }
                }
            }
        })
    }

    fn process_sample(
        &self,
        sample: GeoPacketSample,
        cache: &mut [PrefixCacheSlot],
        counters: &mut GeoWorkerCounters,
    ) {
        let weight = u64::from(sample.weight.max(1));
        counters.samples_processed = counters.samples_processed.saturating_add(1);

        let Some((normalized_ip, prefix_key)) = normalize_and_prefix_key(sample.ip) else {
            counters.unknown_packets = counters.unknown_packets.saturating_add(weight);
            return;
        };

        let mask = cache.len() - 1;
        let slot_idx = prefix_cache_index(prefix_key, mask);
        let cache_gen = self.cache_generation.load(Ordering::Acquire).max(1);

        let iso = {
            let slot = &mut cache[slot_idx];
            if slot.generation == cache_gen && slot.tag == prefix_key {
                counters.cache_hits = counters.cache_hits.saturating_add(1);
                slot.iso
            } else {
                counters.cache_misses = counters.cache_misses.saturating_add(1);
                let reader_guard = self.reader.load();
                let Some(loaded) = reader_guard.as_ref() else {
                    counters.unknown_packets = counters.unknown_packets.saturating_add(weight);
                    return;
                };
                let t0 = Instant::now();
                let resolved = lookup_country_iso(&loaded.reader, normalized_ip);
                let elapsed_ns = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
                counters.lookup_ns_total = counters.lookup_ns_total.saturating_add(elapsed_ns);
                counters.lookup_count = counters.lookup_count.saturating_add(1);

                slot.tag = prefix_key;
                slot.generation = cache_gen;
                slot.iso = resolved;
                resolved
            }
        };

        if iso == [0, 0] {
            counters.unknown_packets = counters.unknown_packets.saturating_add(weight);
            return;
        }

        let entry = counters.countries.entry(iso).or_default();
        entry.packets = entry.packets.saturating_add(weight);
        entry.bytes = entry
            .bytes
            .saturating_add(u64::from(sample.bytes).saturating_mul(weight));
        match sample.kind {
            InboundDhtKind::GetPeers => entry.get_peers = entry.get_peers.saturating_add(weight),
            InboundDhtKind::AnnouncePeer => {
                entry.announce_peer = entry.announce_peer.saturating_add(weight)
            }
            InboundDhtKind::Response => entry.response = entry.response.saturating_add(weight),
            InboundDhtKind::Ping | InboundDhtKind::FindNode => {
                entry.find_node_ping = entry.find_node_ping.saturating_add(weight)
            }
            InboundDhtKind::Error | InboundDhtKind::Other => {
                entry.other = entry.other.saturating_add(weight)
            }
        }
    }

    fn publish_snapshot(&self, counters: &GeoWorkerCounters) {
        let meta = self.meta.load();
        let mmdb_loaded = self.reader.load().is_some();
        let look_total = counters.cache_hits.saturating_add(counters.cache_misses);
        let cache_hit_ratio = if look_total > 0 {
            counters.cache_hits as f64 / look_total as f64
        } else {
            0.0
        };
        let avg_lookup_ns = counters
            .lookup_ns_total
            .checked_div(counters.lookup_count)
            .unwrap_or(0);

        let mut countries = BTreeMap::new();
        for (iso_bytes, totals) in &counters.countries {
            if let Ok(code) = std::str::from_utf8(iso_bytes) {
                countries.insert(code.to_string(), totals.clone());
            }
        }

        let snap = GeoTrafficSnapshot {
            enabled: self.enabled.load(Ordering::Relaxed),
            mmdb_loaded,
            mmdb_path: meta.mmdb_path.clone(),
            mmdb_size_bytes: meta.mmdb_size_bytes,
            mmdb_build_epoch: meta.mmdb_build_epoch,
            database_type: meta.database_type.clone(),
            error: meta.error.clone(),
            home_country: meta.home_country.clone(),
            adaptive_sample_threshold_pps: self.adaptive_threshold_pps.load(Ordering::Relaxed),
            sample_rate_divisor: 1_u16 << self.sample_shift.load(Ordering::Relaxed),
            samples_processed: counters.samples_processed,
            samples_dropped: self.samples_dropped.load(Ordering::Relaxed),
            unknown_packets: counters.unknown_packets,
            cache_hits: counters.cache_hits,
            cache_misses: counters.cache_misses,
            cache_hit_ratio,
            avg_lookup_ns,
            countries,
        };
        self.snapshot.store(Arc::new(snap));
    }
}

fn load_mmdb_state(
    config: &GeoIpConfig,
    resolved_path: &Path,
) -> (Option<LoadedMmdb>, GeoMetaView) {
    let path_str = resolved_path.to_string_lossy().into_owned();
    let home_country = config.normalized_home_country();

    if !config.enabled {
        return (
            None,
            GeoMetaView {
                mmdb_path: path_str,
                mmdb_size_bytes: 0,
                mmdb_build_epoch: 0,
                database_type: String::new(),
                error: Some("GeoIP 归因已禁用".to_string()),
                home_country,
            },
        );
    }

    let size_bytes = match std::fs::metadata(resolved_path) {
        Ok(m) => m.len(),
        Err(err) => {
            return (
                None,
                GeoMetaView {
                    mmdb_path: path_str,
                    mmdb_size_bytes: 0,
                    mmdb_build_epoch: 0,
                    database_type: String::new(),
                    error: Some(format!("未找到 MMDB 离线库文件: {err}")),
                    home_country,
                },
            );
        }
    };

    // SAFETY: MMDB 为只读静态文件映射；热更新时通过 ArcSwap 替换并保持旧映射直到引用释放，不在位覆写已映射文件。
    match unsafe { Reader::open_mmap(resolved_path) } {
        Ok(reader) => {
            let meta = reader.metadata();
            let build_epoch = meta.build_epoch;
            let database_type = meta.database_type.clone();
            (
                Some(LoadedMmdb { reader }),
                GeoMetaView {
                    mmdb_path: path_str,
                    mmdb_size_bytes: size_bytes,
                    mmdb_build_epoch: build_epoch,
                    database_type,
                    error: None,
                    home_country,
                },
            )
        }
        Err(err) => (
            None,
            GeoMetaView {
                mmdb_path: path_str,
                mmdb_size_bytes: size_bytes,
                mmdb_build_epoch: 0,
                database_type: String::new(),
                error: Some(format!("加载 MMDB 文件失败: {err}")),
                home_country,
            },
        ),
    }
}

/// 根据最近 1s 估算 `pkt/s` 与阈值做迟滞切换（高于阈值切换为 1:16，低于 70% 恢复 1:1）。
pub fn compute_next_sample_shift(current_shift: u8, estimated_pps: u64, threshold_pps: u64) -> u8 {
    let threshold = threshold_pps.max(1_000);
    if current_shift == 0 {
        if estimated_pps > threshold { 4 } else { 0 }
    } else {
        let recover_threshold = threshold.saturating_mul(70) / 100;
        if estimated_pps < recover_threshold {
            0
        } else {
            current_shift
        }
    }
}

fn is_routable_ipv4(ip: Ipv4Addr) -> bool {
    if ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
    {
        return false;
    }
    let [a, b, ..] = ip.octets();
    // 排除 0.0.0.0/8 ("this network") 与 100.64.0.0/10 (CGNAT RFC 6598)
    a != 0 && !(a == 100 && (b & 0b1100_0000) == 64)
}

fn is_routable_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    let seg0 = ip.segments()[0];
    // 排除 ULA fc00::/7、link-local fe80::/10 与文档地址 2001:db8::/32
    let is_ula = (seg0 & 0xFE00) == 0xFC00;
    let is_link_local = (seg0 & 0xFFC0) == 0xFE80;
    let is_doc = seg0 == 0x2001 && ip.segments()[1] == 0x0DB8;
    !is_ula && !is_link_local && !is_doc
}

/// 将 IP 归一化（解包 IPv4-mapped IPv6）并提取唯一 `/24` 或 `/48` 前缀键；私有/保留地址返回 `None`。
pub fn normalize_and_prefix_key(ip: IpAddr) -> Option<(IpAddr, u64)> {
    match ip {
        IpAddr::V4(v4) => {
            if !is_routable_ipv4(v4) {
                return None;
            }
            let [a, b, c, _] = v4.octets();
            let key = (u64::from(a) << 16) | (u64::from(b) << 8) | u64::from(c);
            Some((IpAddr::V4(v4), key))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                if !is_routable_ipv4(v4) {
                    return None;
                }
                let [a, b, c, _] = v4.octets();
                let key = (u64::from(a) << 16) | (u64::from(b) << 8) | u64::from(c);
                return Some((IpAddr::V4(v4), key));
            }
            if !is_routable_ipv6(v6) {
                return None;
            }
            let top48 = ((u128::from(v6) >> 80) as u64) & 0x0000_FFFF_FFFF_FFFF;
            let key = (1_u64 << 63) | top48;
            Some((IpAddr::V6(v6), key))
        }
    }
}

#[inline]
fn prefix_cache_index(prefix_key: u64, mask: usize) -> usize {
    let mixed = prefix_key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    ((mixed ^ (mixed >> 32)) as usize) & mask
}

fn lookup_country_iso(reader: &Reader<Mmap>, ip: IpAddr) -> [u8; 2] {
    let Ok(res) = reader.lookup(ip) else {
        return [0, 0];
    };
    if !res.has_data() {
        return [0, 0];
    }
    if let Ok(Some(code)) = res.decode_path::<&str>(&COUNTRY_ISO_PATH)
        && let Some(iso) = parse_iso2(code)
    {
        return iso;
    }
    if let Ok(Some(code)) = res.decode_path::<&str>(&REGISTERED_COUNTRY_ISO_PATH)
        && let Some(iso) = parse_iso2(code)
    {
        return iso;
    }
    [0, 0]
}

fn parse_iso2(code: &str) -> Option<[u8; 2]> {
    let b = code.trim().as_bytes();
    if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1].is_ascii_alphabetic() {
        Some([b[0].to_ascii_uppercase(), b[1].to_ascii_uppercase()])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hameta_common::generate_token;

    fn encode_ctrl(type_num: usize, size: usize, out: &mut Vec<u8>) {
        if type_num <= 7 {
            out.push(((type_num << 5) | size) as u8);
        } else {
            out.push(size as u8);
            out.push((type_num - 7) as u8);
        }
    }

    fn encode_str(s: &str, out: &mut Vec<u8>) {
        encode_ctrl(2, s.len(), out);
        out.extend_from_slice(s.as_bytes());
    }

    fn encode_uint16(val: u16, out: &mut Vec<u8>) {
        let bytes = val.to_be_bytes();
        let trimmed = if bytes[0] == 0 {
            if bytes[1] == 0 { &[][..] } else { &bytes[1..2] }
        } else {
            &bytes[..]
        };
        encode_ctrl(5, trimmed.len(), out);
        out.extend_from_slice(trimmed);
    }

    fn encode_uint32(val: u32, out: &mut Vec<u8>) {
        let bytes = val.to_be_bytes();
        let first_nz = bytes.iter().position(|&b| b != 0).unwrap_or(4);
        let trimmed = &bytes[first_nz..];
        encode_ctrl(6, trimmed.len(), out);
        out.extend_from_slice(trimmed);
    }

    fn encode_uint64(val: u64, out: &mut Vec<u8>) {
        let bytes = val.to_be_bytes();
        let first_nz = bytes.iter().position(|&b| b != 0).unwrap_or(8);
        let trimmed = &bytes[first_nz..];
        encode_ctrl(9, trimmed.len(), out);
        out.extend_from_slice(trimmed);
    }

    /// 构造一个真实可被 `maxminddb::Reader::open_mmap` 读取的最小双国家（`::/1` -> iso_a, `8000::/1` -> iso_b）MMDB 文件。
    fn build_two_country_test_mmdb(iso_a: &str, iso_b: &str) -> Vec<u8> {
        let node_count: u32 = 1;
        let record_size: u16 = 24;

        let mut data_section = Vec::new();
        let offset_a = data_section.len();
        encode_ctrl(7, 1, &mut data_section);
        encode_str("country", &mut data_section);
        encode_ctrl(7, 1, &mut data_section);
        encode_str("iso_code", &mut data_section);
        encode_str(iso_a, &mut data_section);

        let offset_b = data_section.len();
        encode_ctrl(7, 1, &mut data_section);
        encode_str("country", &mut data_section);
        encode_ctrl(7, 1, &mut data_section);
        encode_str("iso_code", &mut data_section);
        encode_str(iso_b, &mut data_section);

        let left_val = node_count + 16 + offset_a as u32;
        let right_val = node_count + 16 + offset_b as u32;

        let mut out = Vec::new();
        out.extend_from_slice(&left_val.to_be_bytes()[1..4]);
        out.extend_from_slice(&right_val.to_be_bytes()[1..4]);
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(&data_section);
        out.extend_from_slice(b"\xab\xcd\xefMaxMind.com");

        encode_ctrl(7, 9, &mut out);
        encode_str("node_count", &mut out);
        encode_uint32(node_count, &mut out);
        encode_str("record_size", &mut out);
        encode_uint16(record_size, &mut out);
        encode_str("ip_version", &mut out);
        encode_uint16(6, &mut out);
        encode_str("binary_format_major_version", &mut out);
        encode_uint16(2, &mut out);
        encode_str("binary_format_minor_version", &mut out);
        encode_uint16(0, &mut out);
        encode_str("build_epoch", &mut out);
        encode_uint64(1_763_800_000, &mut out);
        encode_str("database_type", &mut out);
        encode_str("GeoLite2-Country", &mut out);
        encode_str("description", &mut out);
        encode_ctrl(7, 1, &mut out);
        encode_str("en", &mut out);
        encode_str("Test", &mut out);
        encode_str("languages", &mut out);
        encode_ctrl(11, 1, &mut out);
        encode_str("en", &mut out);

        out
    }

    #[test]
    fn prefix_keys_filter_private_and_normalize_v4_mapped() {
        assert!(normalize_and_prefix_key("127.0.0.1".parse().unwrap()).is_none());
        assert!(normalize_and_prefix_key("10.0.0.5".parse().unwrap()).is_none());
        assert!(normalize_and_prefix_key("192.168.1.2".parse().unwrap()).is_none());
        assert!(normalize_and_prefix_key("100.64.1.1".parse().unwrap()).is_none());
        assert!(normalize_and_prefix_key("fe80::1".parse().unwrap()).is_none());
        assert!(normalize_and_prefix_key("fc00::1".parse().unwrap()).is_none());

        let v4_a = normalize_and_prefix_key("1.1.1.10".parse().unwrap()).unwrap();
        let v4_b = normalize_and_prefix_key("1.1.1.222".parse().unwrap()).unwrap();
        let mapped = normalize_and_prefix_key("::ffff:1.1.1.50".parse().unwrap()).unwrap();
        assert_eq!(v4_a.1, v4_b.1);
        assert_eq!(v4_a.1, mapped.1);
        assert!(matches!(mapped.0, IpAddr::V4(_)));

        let v6_a = normalize_and_prefix_key("2606:4700:4700::1111".parse().unwrap()).unwrap();
        let v6_b = normalize_and_prefix_key("2606:4700:4700:beef::99".parse().unwrap()).unwrap();
        assert_eq!(v6_a.1, v6_b.1);
        assert_ne!(v6_a.1, v4_a.1);
    }

    #[test]
    fn adaptive_sampling_hysteresis_behaves_correctly() {
        assert_eq!(compute_next_sample_shift(0, 29_999, 30_000), 0);
        assert_eq!(compute_next_sample_shift(0, 30_000, 30_000), 0);
        assert_eq!(compute_next_sample_shift(0, 30_001, 30_000), 4);
        assert_eq!(compute_next_sample_shift(4, 25_000, 30_000), 4);
        assert_eq!(compute_next_sample_shift(4, 21_000, 30_000), 4);
        assert_eq!(compute_next_sample_shift(4, 20_999, 30_000), 0);
    }

    #[tokio::test]
    async fn missing_mmdb_degrades_gracefully_without_error() {
        let dir = std::env::temp_dir().join(format!("hameta-geo-missing-{}", generate_token()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = GeoIpConfig::default();
        let engine = GeoIpEngine::new(&cfg, &dir);

        let snap = engine.snapshot();
        assert!(snap.enabled);
        assert!(!snap.mmdb_loaded);
        assert!(snap.error.is_some());

        // 未加载状态下回调入口快速返回
        engine.observe_packet(InboundDhtSample {
            ip: "1.1.1.1".parse().unwrap(),
            bytes: 120,
            kind: InboundDhtKind::GetPeers,
        });
        assert_eq!(engine.snapshot().samples_processed, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn mmap_lookup_prefix_cache_and_hot_reload_work_end_to_end() {
        let dir = std::env::temp_dir().join(format!("hameta-geo-real-{}", generate_token()));
        std::fs::create_dir_all(&dir).unwrap();
        let mmdb_path = dir.join("GeoLite2-Country.mmdb");
        std::fs::write(&mmdb_path, build_two_country_test_mmdb("US", "DE")).unwrap();

        let cfg = GeoIpConfig::default();
        let engine = GeoIpEngine::new(&cfg, &dir);
        let shutdown = CancellationToken::new();
        let handle = engine.spawn_worker(shutdown.clone());

        let snap0 = engine.snapshot();
        assert!(snap0.mmdb_loaded, "mmdb failed to load: {:?}", snap0.error);
        assert_eq!(snap0.database_type, "GeoLite2-Country");

        // 同 /24 两次请求应产生 1 次 miss + 1 次 cache hit；IPv6 8000:: 命中右支 DE
        engine.observe_packet(InboundDhtSample {
            ip: "1.1.1.10".parse().unwrap(),
            bytes: 100,
            kind: InboundDhtKind::GetPeers,
        });
        engine.observe_packet(InboundDhtSample {
            ip: "1.1.1.20".parse().unwrap(),
            bytes: 200,
            kind: InboundDhtKind::AnnouncePeer,
        });
        engine.observe_packet(InboundDhtSample {
            ip: "8000::1".parse().unwrap(),
            bytes: 150,
            kind: InboundDhtKind::Response,
        });
        // 私有地址计入 unknown_packets
        engine.observe_packet(InboundDhtSample {
            ip: "127.0.0.1".parse().unwrap(),
            bytes: 50,
            kind: InboundDhtKind::Ping,
        });

        // 等待后台协程刷新快照
        tokio::time::sleep(Duration::from_millis(650)).await;
        let snap1 = engine.snapshot();
        assert_eq!(snap1.samples_processed, 4);
        assert_eq!(snap1.unknown_packets, 1);
        assert_eq!(snap1.cache_hits, 1);
        assert_eq!(snap1.cache_misses, 2);
        assert_eq!(snap1.countries["US"].packets, 2);
        assert_eq!(snap1.countries["US"].bytes, 300);
        assert_eq!(snap1.countries["US"].get_peers, 1);
        assert_eq!(snap1.countries["US"].announce_peer, 1);
        assert_eq!(snap1.countries["DE"].packets, 1);
        assert_eq!(snap1.countries["DE"].response, 1);

        // 热重载替换为 JP / FR 映射的文件并失效缓存
        let reloaded_path = dir.join("GeoLite2-Country-v2.mmdb");
        std::fs::write(&reloaded_path, build_two_country_test_mmdb("JP", "FR")).unwrap();
        let next_cfg = GeoIpConfig {
            mmdb_path: "GeoLite2-Country-v2.mmdb".to_string(),
            home_country: "JP".to_string(),
            ..cfg
        };
        let reload_resp = engine.reload(&next_cfg, &dir);
        assert!(reload_resp.mmdb_loaded);
        assert!(reload_resp.error.is_none());

        engine.observe_packet(InboundDhtSample {
            ip: "1.1.1.30".parse().unwrap(),
            bytes: 111,
            kind: InboundDhtKind::Ping,
        });

        shutdown.cancel();
        handle.await.unwrap();

        let snap2 = engine.snapshot();
        assert_eq!(snap2.home_country, "JP");
        assert_eq!(snap2.cache_misses, 3);
        assert_eq!(snap2.countries["JP"].packets, 1);
        assert_eq!(snap2.countries["JP"].find_node_ping, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
