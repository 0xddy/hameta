<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { RouterLink, useRoute, useRouter } from "vue-router";
import {
  Activity,
  Cpu,
  HardDrive,
  RefreshCw,
  Stethoscope,
} from "lucide-vue-next";
import {
  getWorkerDiagnostics,
  type CrawlerSnapshot,
  type HistogramMetrics,
  type IndexerSnapshot,
  type WorkerDiagnosticsResponse,
} from "@/api/diagnostics";
import { listWorkers, type WorkerSummaryItem } from "@/api/workers";
import ObsHistogram, { type HistogramBarItem } from "@/components/ui/ObsHistogram.vue";
import ObsHorizontalBars, { type HorizontalBarItem } from "@/components/ui/ObsHorizontalBars.vue";
import ObsSelect from "@/components/ui/ObsSelect.vue";
import {
  fmtBytes,
  fmtCount,
  fmtDurMs,
  fmtDurSecs,
  fmtExact,
  fmtRate,
} from "@/utils/format";

const route = useRoute();
const router = useRouter();

const workersList = ref<WorkerSummaryItem[]>([]);
const selectedId = ref<string>("");
const diag = ref<WorkerDiagnosticsResponse | null>(null);
const loading = ref(false);
const loadError = ref("");
const autoPoll = ref(true);
let pollTimer: number | null = null;

const workerOptions = computed(() =>
  workersList.value.map((w) => ({
    value: w.id,
    label: `${w.display_name} (${w.id}) · ${w.runtime_state}`,
  })),
);

const crawlerSnap = computed<CrawlerSnapshot | null>(() => {
  if (!diag.value || diag.value.role !== "crawler" || !diag.value.snapshot) return null;
  return diag.value.snapshot as CrawlerSnapshot;
});

const indexerSnap = computed<IndexerSnapshot | null>(() => {
  if (!diag.value || diag.value.role !== "indexer" || !diag.value.snapshot) return null;
  return diag.value.snapshot as IndexerSnapshot;
});

function buildHistBars(hist: HistogramMetrics | undefined, isBytes = false): HistogramBarItem[] {
  if (!hist || !hist.bounds || hist.counts.length === 0) return [];
  const fmtBound = (b: number) => (isBytes ? fmtBytes(b) : b >= 1000 ? `${(b / 1000).toFixed(1)}s` : `${b}ms`);
  const bars: HistogramBarItem[] = [];
  for (let i = 0; i < hist.counts.length; i += 1) {
    const label = i < hist.bounds.length ? `≤${fmtBound(hist.bounds[i])}` : `>${fmtBound(hist.bounds[hist.bounds.length - 1])}`;
    bars.push({ label, value: hist.counts[i] });
  }
  if (hist.overflow > 0 && hist.bounds.length > 0) {
    bars.push({
      label: `>${fmtBound(hist.bounds[hist.bounds.length - 1])}`,
      value: hist.overflow,
    });
  }
  return bars;
}

const queueWaitBars = computed(() => buildHistBars(crawlerSnap.value?.observability.queue_wait_ms));
const fetchDurationBars = computed(() => buildHistBars(crawlerSnap.value?.observability.fetch_duration_ms));
const metaSizeBars = computed(() => buildHistBars(crawlerSnap.value?.observability.metadata_size_bytes, true));

const timeoutStageItems = computed<HorizontalBarItem[]>(() => {
  const s = crawlerSnap.value?.observability.metadata_timeout_stages;
  if (!s) return [];
  return [
    { label: "tcp_connect", value: s.tcp_connect },
    { label: "connect_handshake", value: s.connect_handshake },
    { label: "base_handshake", value: s.base_handshake },
    { label: "ext_hs_send", value: s.extension_handshake_send },
    { label: "ext_hs_recv", value: s.extension_handshake_receive },
    { label: "piece_req_send", value: s.piece_request_send },
    { label: "piece_receive", value: s.piece_receive },
    { label: "validation", value: s.validation },
  ];
});

const connErrorItems = computed<HorizontalBarItem[]>(() => {
  const c = crawlerSnap.value?.observability.metadata_connection_errors;
  if (!c) return [];
  return [
    { label: "refused", value: c.refused },
    { label: "timeout", value: c.timeout },
    { label: "reset", value: c.reset },
    { label: "closed", value: c.closed },
    { label: "aborted", value: c.aborted },
    { label: "invalid_handshake", value: c.invalid_handshake },
    { label: "info_hash_mismatch", value: c.info_hash_mismatch },
    { label: "io_other", value: c.io_other },
    { label: "other", value: c.other },
  ];
});

const crawlerSaturationMeters = computed(() => {
  const s = crawlerSnap.value;
  if (!s) return [];
  const d = s.dht ?? {};
  const def = diag.value?.profile_defaults;
  const rows = [
    {
      label: "Metadata 队列",
      cur: Number(d.metadata_queue_depth ?? 0),
      max: Math.max(1, Number(d.metadata_queue_max ?? 1)),
      cls: "bg-cyan-400",
    },
    {
      label: "Metadata 并发",
      cur: Number(d.metadata_in_flight ?? 0),
      max: Math.max(1, Number(s.effective_metadata_workers ?? 1)),
      cls: "bg-emerald-400",
    },
    {
      label: "find_node 在途",
      cur: Number(d.find_node_in_flight ?? 0),
      max: Math.max(1, Number(d.find_node_in_flight_max || def?.find_node_max_in_flight || 1)),
      cls: "bg-amber-400",
    },
    {
      label: "DHT 路由 Peer 池",
      cur: Number(d.node_pool_size ?? 0),
      max: Math.max(1, Number(d.node_pool_capacity || def?.pool_capacity || 1)),
      cls: "bg-indigo-400",
    },
    {
      label: "入口 Hash 队列",
      cur: Number(d.hash_ingress_queue_depth ?? 0),
      max: Math.max(1, Number(d.hash_ingress_queue_capacity || 1)),
      cls: "bg-sky-400",
    },
  ];
  return rows.map((m) => ({
    ...m,
    pct: Math.min(100, Math.round((m.cur / m.max) * 100)),
  }));
});

const dhtProtocolItems = computed<HorizontalBarItem[]>(() => {
  const o = crawlerSnap.value?.observability;
  if (!o) return [];
  return [
    { label: "inbound_get_peers", value: o.inbound_get_peers },
    { label: "inbound_announce", value: o.inbound_announce_peer },
    { label: "inbound_find_node", value: o.inbound_find_node },
    { label: "inbound_ping", value: o.inbound_ping },
    { label: "inbound_other", value: o.inbound_other },
  ];
});

async function loadWorkerCatalogAndSelect() {
  try {
    workersList.value = await listWorkers();
  } catch {
    workersList.value = [];
  }
  const paramId = typeof route.params.id === "string" ? route.params.id : "";
  if (paramId) {
    selectedId.value = paramId;
    return;
  }
  if (!selectedId.value && workersList.value.length > 0) {
    const runningFirst = workersList.value.find((w) => w.runtime_state === "running") ?? workersList.value[0];
    selectedId.value = runningFirst.id;
  }
}

async function fetchDiag(silent = false) {
  if (!selectedId.value) return;
  if (!silent) loading.value = true;
  loadError.value = "";
  try {
    diag.value = await getWorkerDiagnostics(selectedId.value);
  } catch (e) {
    loadError.value = e instanceof Error ? e.message : "加载诊断数据失败";
  } finally {
    loading.value = false;
  }
}

function onSelectWorker(nextId: string | number) {
  const id = String(nextId);
  selectedId.value = id;
  if (route.path.startsWith("/workers/")) {
    void router.push(`/workers/${encodeURIComponent(id)}/diagnostics`);
  } else {
    void fetchDiag();
  }
}

watch(
  () => route.params.id,
  (next) => {
    if (typeof next === "string" && next) {
      selectedId.value = next;
      void fetchDiag();
    }
  },
);

onMounted(async () => {
  await loadWorkerCatalogAndSelect();
  if (selectedId.value) await fetchDiag();
  pollTimer = window.setInterval(() => {
    if (autoPoll.value && selectedId.value) {
      void fetchDiag(true);
    }
  }, 3000);
});

onUnmounted(() => {
  if (pollTimer !== null) {
    window.clearInterval(pollTimer);
    pollTimer = null;
  }
});
</script>

<template>
  <div class="space-y-4">
    <!-- Top Header & Selector Bar -->
    <div class="obs-card rounded-xl px-4 py-3 flex flex-wrap items-center justify-between gap-3">
      <div class="flex flex-wrap items-center gap-2.5 text-xs">
        <RouterLink to="/workers" class="obs-btn-ghost px-2.5 py-1.5 rounded-lg cursor-pointer">
          ← 返回
        </RouterLink>

        <ObsSelect
          v-if="workerOptions.length > 0"
          :model-value="selectedId"
          :options="workerOptions"
          mono
          class="w-72 max-w-full"
          trigger-class="px-3! py-1.5! text-xs!"
          @update:model-value="onSelectWorker"
        />
        <span v-else class="text-xs font-mono-num text-slate-500">暂无可用 Worker 实例</span>

        <template v-if="diag">
          <span
            class="px-2 py-0.5 rounded font-mono-num"
            :class="diag.role === 'crawler' ? 'bg-cyan-500/15 text-cyan-300' : 'bg-emerald-500/15 text-emerald-300'"
          >
            {{ diag.role === "crawler" ? "Crawler" : "Indexer" }}
          </span>
          <span
            class="px-2 py-0.5 rounded font-mono-num"
            :class="
              diag.runtime_state === 'running'
                ? 'bg-emerald-500/15 text-emerald-300'
                : diag.runtime_state === 'crashed'
                  ? 'bg-rose-500/15 text-rose-300'
                  : 'bg-slate-800 text-slate-400'
            "
          >
            {{ diag.runtime_state === "running" ? "运行中" : diag.runtime_state }} · PID {{ diag.pid ?? "-" }}
          </span>
        </template>
      </div>

      <div class="flex flex-wrap items-center gap-2.5">
        <button
          type="button"
          class="text-xs font-mono-num cursor-pointer transition"
          :class="autoPoll ? 'text-cyan-400 hover:text-cyan-300' : 'text-amber-400 hover:text-amber-300'"
          :title="autoPoll ? '点击暂停 3s 自动轮询' : '点击恢复 3s 自动轮询'"
          @click="autoPoll = !autoPoll"
        >
          {{ autoPoll ? "● 3s 自动采样中" : "○ 自动采样已暂停" }}
        </button>
        <RouterLink
          v-if="selectedId"
          :to="`/workers?tune=${encodeURIComponent(selectedId)}`"
          class="obs-btn-ghost px-2.5 py-1.5 rounded-lg text-xs text-emerald-300 cursor-pointer"
        >
          调优
        </RouterLink>
        <button
          type="button"
          class="obs-btn-ghost px-2.5 py-1.5 rounded-lg text-xs inline-flex items-center gap-1 cursor-pointer"
          @click="fetchDiag()"
        >
          <RefreshCw class="w-3.5 h-3.5" :class="loading ? 'animate-spin text-emerald-400' : ''" />
          刷新
        </button>
      </div>
    </div>

    <div v-if="loadError" class="obs-card rounded-xl p-4 border-rose-500/35 text-xs text-rose-300 font-mono-num">
      {{ loadError }}
    </div>

    <div v-else-if="!diag" class="obs-card rounded-xl p-12 text-center text-xs text-slate-500 font-mono-num">
      <Stethoscope class="w-8 h-8 text-slate-600 mx-auto mb-2" />
      请选择或创建一个受管 Worker 实例以查看内核深度诊断指标
    </div>

    <div v-else-if="!diag.snapshot" class="obs-card rounded-xl p-10 text-center space-y-2">
      <Cpu class="w-8 h-8 text-slate-600 mx-auto" />
      <div class="text-sm font-medium text-slate-200">暂无实时 IPC 诊断快照</div>
      <div class="text-xs text-slate-500">
        当前进程状态为 <span class="font-mono-num text-slate-300">{{ diag.runtime_state }}</span>。如刚启动进程，请等待下一秒内核心跳上报。
      </div>
      <div v-if="diag.last_error" class="text-xs font-mono-num text-rose-300 pt-2">
        {{ diag.last_error }}
      </div>
    </div>

    <!-- CRAWLER DIAGNOSTICS -->
    <template v-else-if="crawlerSnap">
      <!-- 4 Top KPIs -->
      <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
        <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
          <div class="text-xs text-slate-400">档位与运行时长</div>
          <div class="text-lg font-semibold font-mono-num text-slate-50 truncate">
            {{ crawlerSnap.crawl_profile }} · {{ fmtDurSecs(crawlerSnap.uptime_secs) }}
          </div>
          <div class="text-[11px] font-mono-num text-slate-400 truncate">
            并发 {{ fmtCount(crawlerSnap.effective_metadata_workers) }} · 探测 {{ fmtRate(crawlerSnap.dht.find_node_effective_rate_per_sec) }}/s
          </div>
        </div>

        <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
          <div class="text-xs text-slate-400">内存 RSS &amp; 去重缓存</div>
          <div class="text-lg font-semibold font-mono-num text-slate-50 truncate">
            {{ fmtBytes(crawlerSnap.rss_bytes) }}
          </div>
          <div
            class="text-[11px] font-mono-num text-slate-400 truncate"
            :title="fmtExact(crawlerSnap.local_cache_entries)"
          >
            本地缓存 {{ fmtCount(crawlerSnap.local_cache_entries) }}
          </div>
        </div>

        <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
          <div class="text-xs text-slate-400">Redis 池与累计推流</div>
          <div class="text-lg font-semibold font-mono-num text-slate-50 truncate">
            {{ crawlerSnap.redis_pool_available }} / {{ crawlerSnap.redis_pool_size }}
          </div>
          <div
            class="text-[11px] font-mono-num text-slate-400 truncate"
            :title="`下载 ${fmtExact(crawlerSnap.app.torrents_downloaded)} / 推流 ${fmtExact(crawlerSnap.app.torrents_pushed)}`"
          >
            下载 {{ fmtCount(crawlerSnap.app.torrents_downloaded) }} · 推流 {{ fmtCount(crawlerSnap.app.torrents_pushed) }}
          </div>
        </div>

        <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
          <div class="text-xs text-slate-400">UDP 累计流量与丢包</div>
          <div class="text-lg font-semibold font-mono-num text-slate-50 truncate">
            RX {{ fmtBytes(crawlerSnap.observability.udp_rx_bytes) }}
          </div>
          <div class="text-[11px] font-mono-num text-slate-400 truncate">
            TX {{ fmtBytes(crawlerSnap.observability.udp_tx_bytes) }} · 丢包 {{ fmtCount(crawlerSnap.dht.udp_queue_full) }}
          </div>
        </div>
      </div>

      <!-- 队列与并发饱和度 -->
      <div class="obs-card rounded-xl p-5 space-y-4">
        <h2 class="text-sm font-semibold text-slate-100">队列与并发饱和度</h2>
        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4 text-xs">
          <div v-for="m in crawlerSaturationMeters" :key="m.label" class="space-y-1.5">
            <div class="flex justify-between text-slate-300">
              <span>{{ m.label }}</span>
              <span class="font-mono-num" :title="`${fmtExact(m.cur)} / ${fmtExact(m.max)}`">
                {{ fmtCount(m.cur) }} / {{ fmtCount(m.max) }} ({{ m.pct }}%)
              </span>
            </div>
            <div class="h-1.5 rounded-full bg-slate-800 overflow-hidden">
              <div class="h-full rounded-full transition-all duration-300" :class="m.cls" :style="{ width: `${m.pct}%` }"></div>
            </div>
          </div>
          <div class="space-y-1">
            <div class="flex justify-between text-slate-300">
              <span>已下载元数据</span>
              <span class="font-mono-num font-semibold text-emerald-300">
                {{ fmtBytes(crawlerSnap.observability.metadata_bytes_downloaded) }}
              </span>
            </div>
            <div class="text-[11px] font-mono-num text-slate-500">
              坏 Peer 命中 T={{ fmtCount(crawlerSnap.observability.peer_cache_timeout_hits) }} / C={{ fmtCount(crawlerSnap.observability.peer_cache_connect_hits) }}
            </div>
          </div>
        </div>
      </div>

      <!-- Histograms Row -->
      <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <div class="obs-card rounded-xl p-4 space-y-2">
          <div class="flex justify-between text-xs">
            <span class="text-slate-300 font-medium">排队等待</span>
            <span class="font-mono-num text-indigo-400">
              P50 {{ fmtDurMs(crawlerSnap.observability.queue_wait_ms.p50) }} · P95 {{ fmtDurMs(crawlerSnap.observability.queue_wait_ms.p95) }}
            </span>
          </div>
          <ObsHistogram :bars="queueWaitBars" color="#818cf8" />
        </div>

        <div class="obs-card rounded-xl p-4 space-y-2">
          <div class="flex justify-between text-xs">
            <span class="text-slate-300 font-medium">抓取耗时</span>
            <span class="font-mono-num text-sky-400">
              P50 {{ fmtDurMs(crawlerSnap.observability.fetch_duration_ms.p50) }} · P95 {{ fmtDurMs(crawlerSnap.observability.fetch_duration_ms.p95) }}
            </span>
          </div>
          <ObsHistogram :bars="fetchDurationBars" color="#38bdf8" />
        </div>

        <div class="obs-card rounded-xl p-4 space-y-2">
          <div class="flex justify-between text-xs">
            <span class="text-slate-300 font-medium">元数据体积</span>
            <span class="font-mono-num text-emerald-400">
              P50 {{ fmtBytes(crawlerSnap.observability.metadata_size_bytes.p50) }} · P95 {{ fmtBytes(crawlerSnap.observability.metadata_size_bytes.p95) }}
            </span>
          </div>
          <ObsHistogram :bars="metaSizeBars" color="#10b981" />
        </div>
      </div>

      <!-- Failure Attribution -->
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <div class="obs-card rounded-xl p-4 space-y-3">
          <div class="flex items-center justify-between">
            <div class="text-xs font-semibold text-slate-200">超时阶段分布</div>
            <span class="text-[11px] font-mono-num text-amber-300">
              总超时 {{ fmtCount(crawlerSnap.observability.metadata_failure_timeout) }}
            </span>
          </div>
          <ObsHorizontalBars :items="timeoutStageItems" color="#f59e0b" />
        </div>

        <div class="obs-card rounded-xl p-4 space-y-3">
          <div class="flex items-center justify-between">
            <div class="text-xs font-semibold text-slate-200">连接错误原因</div>
            <span class="text-[11px] font-mono-num text-rose-300">
              建连失败 {{ fmtCount(crawlerSnap.observability.metadata_failure_connect) }}
            </span>
          </div>
          <ObsHorizontalBars :items="connErrorItems" color="#f43f5e" />
        </div>
      </div>

      <!-- DHT Inbound + Raw Kernel Counter Tables -->
      <div class="grid grid-cols-1 lg:grid-cols-4 gap-4">
        <div class="obs-card rounded-xl p-4 space-y-3">
          <div class="flex items-center justify-between">
            <h3 class="text-xs font-semibold text-slate-200">DHT UDP 入站动作</h3>
            <span class="text-[11px] font-mono-num text-cyan-300">
              入池 {{ fmtCount(crawlerSnap.observability.node_admitted) }}
            </span>
          </div>
          <ObsHorizontalBars :items="dhtProtocolItems" color="#22d3ee" />
          <div class="obs-inset p-2.5 rounded-lg grid grid-cols-2 gap-1.5 text-[11px] font-mono-num">
            <span class="text-slate-400">替换 <b class="text-slate-200">{{ fmtCount(crawlerSnap.observability.node_replaced) }}</b></span>
            <span class="text-slate-400">重复丢弃 <b class="text-slate-200">{{ fmtCount(crawlerSnap.observability.node_dropped_duplicate) }}</b></span>
            <span class="text-slate-400">限流 <b class="text-amber-300">{{ fmtCount(crawlerSnap.observability.node_dropped_rate_limited) }}</b></span>
            <span class="text-slate-400">回包限流 <b class="text-amber-300">{{ fmtCount(crawlerSnap.observability.response_rate_limited) }}</b></span>
          </div>
        </div>

        <div class="obs-card rounded-xl p-4 space-y-2.5">
          <h3 class="text-xs font-semibold text-slate-200">应用层计数器 (app)</h3>
          <div class="obs-inset rounded-lg divide-y divide-slate-800/60 max-h-60 overflow-auto obs-scroll text-[11px] font-mono-num">
            <div v-for="(val, key) in crawlerSnap.app" :key="key" class="px-3 py-1.5 flex items-center justify-between gap-2">
              <span class="text-slate-400 truncate">{{ key }}</span>
              <span class="text-slate-100">{{ fmtExact(val) }}</span>
            </div>
          </div>
        </div>

        <div class="obs-card rounded-xl p-4 space-y-2.5">
          <h3 class="text-xs font-semibold text-slate-200">DHT 引擎状态 (dht)</h3>
          <div class="obs-inset rounded-lg divide-y divide-slate-800/60 max-h-60 overflow-auto obs-scroll text-[11px] font-mono-num">
            <div v-for="(val, key) in crawlerSnap.dht" :key="key" class="px-3 py-1.5 flex items-center justify-between gap-2">
              <span class="text-slate-400 truncate">{{ key }}</span>
              <span class="text-slate-100">{{ fmtExact(val) }}</span>
            </div>
          </div>
        </div>

        <div class="obs-card rounded-xl p-4 space-y-2.5">
          <h3 class="text-xs font-semibold text-slate-200">产出漏斗计数 (yield)</h3>
          <div class="obs-inset rounded-lg divide-y divide-slate-800/60 max-h-60 overflow-auto obs-scroll text-[11px] font-mono-num">
            <div v-for="(val, key) in crawlerSnap.yield_gauges" :key="`g:${key}`" class="px-3 py-1.5 flex items-center justify-between gap-2">
              <span class="text-emerald-400/90 truncate">{{ key }}</span>
              <span class="text-emerald-200">{{ fmtExact(val) }}</span>
            </div>
            <div v-for="(val, key) in crawlerSnap.yield_counters" :key="`c:${key}`" class="px-3 py-1.5 flex items-center justify-between gap-2">
              <span class="text-slate-400 truncate">{{ key }}</span>
              <span class="text-slate-100">{{ fmtExact(val) }}</span>
            </div>
          </div>
        </div>
      </div>
    </template>

    <!-- INDEXER DIAGNOSTICS -->
    <template v-else-if="indexerSnap">
      <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
        <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
          <div class="text-xs text-slate-400">规则版本与运行时长</div>
          <div class="text-lg font-semibold font-mono-num text-emerald-300">
            v{{ indexerSnap.active_rule_version }} · {{ fmtDurSecs(indexerSnap.uptime_secs) }}
          </div>
          <div class="text-[11px] font-mono-num text-slate-400">
            RSS {{ fmtBytes(indexerSnap.rss_bytes) }} · Redis {{ indexerSnap.redis_pool_available }}/{{ indexerSnap.redis_pool_size }}
          </div>
        </div>

        <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
          <div class="text-xs text-slate-400">Stream 读取 (new / hot)</div>
          <div
            class="text-lg font-semibold font-mono-num text-slate-50 truncate"
            :title="`${fmtExact(indexerSnap.app.torrent_messages_read)} / ${fmtExact(indexerSnap.app.hot_messages_read)}`"
          >
            {{ fmtCount(indexerSnap.app.torrent_messages_read) }} / {{ fmtCount(indexerSnap.app.hot_messages_read) }}
          </div>
          <div class="text-[11px] font-mono-num text-slate-400 truncate">
            reclaim {{ fmtCount(indexerSnap.app.reclaimed_new) }}/{{ fmtCount(indexerSnap.app.reclaimed_hot) }} · 毒丸 {{ fmtCount(indexerSnap.app.poison_messages_acked) }}
          </div>
        </div>

        <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
          <div class="text-xs text-slate-400">过滤判定 (放行 / 拦截)</div>
          <div
            class="text-lg font-semibold font-mono-num text-slate-50 truncate"
            :title="`${fmtExact(indexerSnap.filter.total_accepted)} / ${fmtExact(indexerSnap.filter.total_rejected)}`"
          >
            {{ fmtCount(indexerSnap.filter.total_accepted) }} / {{ fmtCount(indexerSnap.filter.total_rejected) }}
          </div>
          <div class="text-[11px] font-mono-num text-slate-400">
            buf {{ indexerSnap.app.torrent_buffered }} · inflight {{ indexerSnap.app.torrent_in_flight }}
          </div>
        </div>

        <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
          <div class="text-xs text-slate-400">Manticore 写库统计</div>
          <div
            class="text-lg font-semibold font-mono-num text-emerald-300 truncate"
            :title="`${fmtExact(indexerSnap.app.records_written)} · Hot ${fmtExact(indexerSnap.app.hot_records_updated)}`"
          >
            {{ fmtCount(indexerSnap.app.records_written) }} · Hot {{ fmtCount(indexerSnap.app.hot_records_updated) }}
          </div>
          <div class="text-[11px] font-mono-num text-slate-400">
            成功 {{ fmtCount(indexerSnap.app.batches_written) }} · 失败 {{ fmtCount(indexerSnap.app.batches_failed) }}
          </div>
        </div>
      </div>

      <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <div class="obs-card rounded-xl p-5 space-y-3">
          <div class="flex items-center gap-2">
            <Activity class="w-4 h-4 text-emerald-400" />
            <h3 class="text-xs font-semibold text-slate-100">内存缓冲与在途积压</h3>
          </div>
          <div class="obs-inset p-3.5 rounded-xl grid grid-cols-3 gap-3 text-xs font-mono-num">
            <div>
              <span class="text-[11px] text-slate-500 block font-sans">Torrent 攒批缓冲</span>
              <span class="text-lg font-bold text-slate-100">{{ indexerSnap.app.torrent_buffered }}</span>
            </div>
            <div>
              <span class="text-[11px] text-slate-500 block font-sans">正在写入在途批次</span>
              <span class="text-lg font-bold text-cyan-300">{{ indexerSnap.app.torrent_in_flight }}</span>
            </div>
            <div>
              <span class="text-[11px] text-slate-500 block font-sans">Hot 更新缓冲</span>
              <span class="text-lg font-bold text-amber-300">{{ indexerSnap.app.hot_buffered }}</span>
            </div>
          </div>
        </div>

        <div class="obs-card rounded-xl p-5 space-y-3">
          <div class="flex items-center gap-2">
            <HardDrive class="w-4 h-4 text-sky-400" />
            <h3 class="text-xs font-semibold text-slate-100">Stream 消费确认与 XAUTOCLAIM 故障回收</h3>
          </div>
          <div class="obs-inset p-3.5 rounded-xl grid grid-cols-2 sm:grid-cols-3 gap-3 text-[11px] font-mono-num">
            <div><span class="text-slate-500 block">读取 New / Hot</span><span class="text-slate-200">{{ fmtExact(indexerSnap.app.torrent_messages_read) }} / {{ fmtExact(indexerSnap.app.hot_messages_read) }}</span></div>
            <div><span class="text-slate-500 block">XACK New / Hot</span><span class="text-emerald-300">{{ fmtExact(indexerSnap.app.stream_xacked_new) }} / {{ fmtExact(indexerSnap.app.stream_xacked_hot) }}</span></div>
            <div><span class="text-slate-500 block">认领 Reclaim New / Hot</span><span class="text-sky-300">{{ indexerSnap.app.reclaimed_new }} / {{ indexerSnap.app.reclaimed_hot }}</span></div>
            <div><span class="text-slate-500 block">损坏报文跳过</span><span class="text-amber-300">{{ indexerSnap.app.malformed_messages_acked }}</span></div>
            <div><span class="text-slate-500 block">毒丸报文跳过</span><span class="text-rose-300">{{ indexerSnap.app.poison_messages_acked }}</span></div>
            <div><span class="text-slate-500 block">热度更新异常</span><span class="text-rose-300">{{ indexerSnap.app.hot_update_errors }}</span></div>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>
