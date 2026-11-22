<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import * as echarts from "echarts";
import {
  Activity,
  ArrowLeft,
  Cpu,
  HardDrive,
  RefreshCw,
  Wifi,
} from "lucide-vue-next";
import { ElMessage } from "element-plus";
import {
  getWorkerDiagnostics,
  type CrawlerSnapshot,
  type HistogramMetrics,
  type IndexerSnapshot,
  type WorkerDiagnosticsResponse,
} from "@/api/diagnostics";
import { listWorkers, type WorkerSummaryItem } from "@/api/workers";
import type { WorkerRuntimeState } from "@/api/overview";

const route = useRoute();
const router = useRouter();

const loading = ref(true);
const workers = ref<WorkerSummaryItem[]>([]);
const selectedId = ref(String(route.params.id ?? ""));
const diag = ref<WorkerDiagnosticsResponse | null>(null);
const autoRefresh = ref(true);
let pollTimer: number | null = null;

// 5 个 ECharts 实例与 DOM 引用
const queueHistRef = ref<HTMLDivElement | null>(null);
const fetchHistRef = ref<HTMLDivElement | null>(null);
const sizeHistRef = ref<HTMLDivElement | null>(null);
const timeoutStagesRef = ref<HTMLDivElement | null>(null);
const connErrorsRef = ref<HTMLDivElement | null>(null);

let queueHistChart: echarts.ECharts | null = null;
let fetchHistChart: echarts.ECharts | null = null;
let sizeHistChart: echarts.ECharts | null = null;
let timeoutStagesChart: echarts.ECharts | null = null;
let connErrorsChart: echarts.ECharts | null = null;

const crawlerSnap = computed<CrawlerSnapshot | null>(() => {
  if (!diag.value || diag.value.role !== "crawler" || !diag.value.snapshot) return null;
  return diag.value.snapshot as CrawlerSnapshot;
});

const indexerSnap = computed<IndexerSnapshot | null>(() => {
  if (!diag.value || diag.value.role !== "indexer" || !diag.value.snapshot) return null;
  return diag.value.snapshot as IndexerSnapshot;
});

const yieldCounterRows = computed(() => {
  const c = crawlerSnap.value?.yield_counters;
  if (!c) return [];
  return Object.entries(c).map(([key, value]) => ({ key, value }));
});

const yieldGaugeRows = computed(() => {
  const g = crawlerSnap.value?.yield_gauges;
  if (!g) return [];
  return Object.entries(g).map(([key, value]) => ({ key, value }));
});

const appStatRows = computed(() => {
  const app = crawlerSnap.value?.app;
  if (!app) return [];
  return Object.entries(app).map(([key, value]) => ({ key, value }));
});

function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || Number.isNaN(bytes)) return "-";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let val = bytes / 1024;
  let idx = 0;
  while (val >= 1024 && idx < units.length - 1) {
    val /= 1024;
    idx += 1;
  }
  return `${val.toFixed(val >= 100 ? 0 : 2)} ${units[idx]}`;
}

function formatDuration(secs: number | null | undefined): string {
  if (secs === null || secs === undefined) return "-";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m}m ${s}s`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

function formatTs(ts: number | null | undefined): string {
  if (!ts) return "-";
  return new Date(ts * 1000).toLocaleString("zh-CN", { hour12: false });
}

function pct(used: number | undefined, max: number | undefined): number {
  if (!used || !max || max <= 0) return 0;
  return Math.min(100, Math.round((used / max) * 100));
}

function stateLabel(state: WorkerRuntimeState): string {
  switch (state) {
    case "running":
      return "运行中";
    case "starting":
      return "启动中";
    case "draining":
      return "排空停机中";
    case "crashed":
      return "异常熔断";
    default:
      return "已停止";
  }
}

function stateTagType(
  state: WorkerRuntimeState,
): "success" | "warning" | "danger" | "info" | "primary" {
  switch (state) {
    case "running":
      return "success";
    case "starting":
      return "primary";
    case "draining":
      return "warning";
    case "crashed":
      return "danger";
    default:
      return "info";
  }
}

function buildHistogramLabels(hist: HistogramMetrics, byteMode = false): string[] {
  const labels = hist.bounds.map((b) => (byteMode ? `≤${formatBytes(b)}` : `≤${b}`));
  if (hist.counts.length > hist.bounds.length || hist.overflow > 0) {
    const last = hist.bounds[hist.bounds.length - 1] ?? 0;
    labels.push(byteMode ? `>${formatBytes(last)}` : `>${last}`);
  }
  return labels;
}

function buildHistogramSeriesData(hist: HistogramMetrics): number[] {
  const data = [...hist.counts];
  if (hist.counts.length <= hist.bounds.length && hist.overflow > 0) {
    data.push(hist.overflow);
  }
  return data;
}

function renderBarChart(
  chart: echarts.ECharts,
  labels: string[],
  values: number[],
  color: string,
) {
  chart.setOption(
    {
      tooltip: { trigger: "axis" },
      grid: { left: 45, right: 16, top: 20, bottom: 28 },
      xAxis: {
        type: "category",
        data: labels,
        axisLabel: { fontSize: 10, color: "#64748b" },
      },
      yAxis: {
        type: "value",
        splitLine: { lineStyle: { color: "#f1f5f9" } },
        axisLabel: { fontSize: 10, color: "#64748b" },
      },
      series: [
        {
          type: "bar",
          barMaxWidth: 32,
          itemStyle: { color, borderRadius: [4, 4, 0, 0] },
          data: values,
        },
      ],
    },
    { notMerge: true },
  );
}

function renderHorizontalChart(
  chart: echarts.ECharts,
  items: Array<{ name: string; value: number }>,
  color: string,
) {
  chart.setOption(
    {
      tooltip: { trigger: "axis" },
      grid: { left: 140, right: 24, top: 12, bottom: 24 },
      xAxis: {
        type: "value",
        splitLine: { lineStyle: { color: "#f1f5f9" } },
        axisLabel: { fontSize: 10, color: "#64748b" },
      },
      yAxis: {
        type: "category",
        data: items.map((i) => i.name),
        axisLabel: { fontSize: 11, color: "#475569" },
      },
      series: [
        {
          type: "bar",
          barMaxWidth: 20,
          itemStyle: { color, borderRadius: [0, 4, 4, 0] },
          data: items.map((i) => i.value),
        },
      ],
    },
    { notMerge: true },
  );
}

function renderCrawlerCharts() {
  const obs = crawlerSnap.value?.observability;
  if (!obs) return;

  if (queueHistRef.value && obs.queue_wait_ms) {
    if (!queueHistChart) queueHistChart = echarts.init(queueHistRef.value);
    renderBarChart(
      queueHistChart,
      buildHistogramLabels(obs.queue_wait_ms, false),
      buildHistogramSeriesData(obs.queue_wait_ms),
      "#6366f1",
    );
  }
  if (fetchHistRef.value && obs.fetch_duration_ms) {
    if (!fetchHistChart) fetchHistChart = echarts.init(fetchHistRef.value);
    renderBarChart(
      fetchHistChart,
      buildHistogramLabels(obs.fetch_duration_ms, false),
      buildHistogramSeriesData(obs.fetch_duration_ms),
      "#0ea5e9",
    );
  }
  if (sizeHistRef.value && obs.metadata_size_bytes) {
    if (!sizeHistChart) sizeHistChart = echarts.init(sizeHistRef.value);
    renderBarChart(
      sizeHistChart,
      buildHistogramLabels(obs.metadata_size_bytes, true),
      buildHistogramSeriesData(obs.metadata_size_bytes),
      "#10b981",
    );
  }
  if (timeoutStagesRef.value && obs.metadata_timeout_stages) {
    if (!timeoutStagesChart) timeoutStagesChart = echarts.init(timeoutStagesRef.value);
    const t = obs.metadata_timeout_stages;
    renderHorizontalChart(
      timeoutStagesChart,
      [
        { name: "tcp_connect", value: t.tcp_connect },
        { name: "connect_handshake", value: t.connect_handshake },
        { name: "base_handshake", value: t.base_handshake },
        { name: "ext_send", value: t.extension_handshake_send },
        { name: "ext_receive", value: t.extension_handshake_receive },
        { name: "piece_request", value: t.piece_request_send },
        { name: "piece_receive", value: t.piece_receive },
        { name: "validation", value: t.validation },
      ],
      "#f59e0b",
    );
  }
  if (connErrorsRef.value && obs.metadata_connection_errors) {
    if (!connErrorsChart) connErrorsChart = echarts.init(connErrorsRef.value);
    const e = obs.metadata_connection_errors;
    renderHorizontalChart(
      connErrorsChart,
      [
        { name: "timeout", value: e.timeout },
        { name: "refused", value: e.refused },
        { name: "reset", value: e.reset },
        { name: "closed", value: e.closed },
        { name: "invalid_handshake", value: e.invalid_handshake },
        { name: "info_hash_mismatch", value: e.info_hash_mismatch },
        { name: "aborted", value: e.aborted },
        { name: "io_other", value: e.io_other },
        { name: "other", value: e.other },
      ],
      "#ef4444",
    );
  }
}

function disposeAllCharts() {
  queueHistChart?.dispose();
  fetchHistChart?.dispose();
  sizeHistChart?.dispose();
  timeoutStagesChart?.dispose();
  connErrorsChart?.dispose();
  queueHistChart = null;
  fetchHistChart = null;
  sizeHistChart = null;
  timeoutStagesChart = null;
  connErrorsChart = null;
}

function handleResize() {
  queueHistChart?.resize();
  fetchHistChart?.resize();
  sizeHistChart?.resize();
  timeoutStagesChart?.resize();
  connErrorsChart?.resize();
}

async function fetchDiagnostics(silent = false) {
  if (!selectedId.value) return;
  if (!silent) loading.value = true;
  try {
    diag.value = await getWorkerDiagnostics(selectedId.value);
    await nextTick();
    renderCrawlerCharts();
  } catch (err) {
    if (!silent) {
      ElMessage.error(err instanceof Error ? err.message : "加载节点诊断数据失败");
    }
  } finally {
    loading.value = false;
  }
}

function handleWorkerChange(nextId: string) {
  disposeAllCharts();
  void router.push(`/workers/${encodeURIComponent(nextId)}/diagnostics`);
}

watch(
  () => route.params.id,
  (newId) => {
    const id = String(newId ?? "");
    if (id && id !== selectedId.value) {
      selectedId.value = id;
      disposeAllCharts();
      void fetchDiagnostics();
    }
  },
);

onMounted(async () => {
  window.addEventListener("resize", handleResize);
  try {
    workers.value = await listWorkers();
  } catch {
    workers.value = [];
  }
  await fetchDiagnostics();
  pollTimer = window.setInterval(() => {
    if (autoRefresh.value) {
      void fetchDiagnostics(true);
    }
  }, 3000);
});

onUnmounted(() => {
  window.removeEventListener("resize", handleResize);
  if (pollTimer !== null) {
    window.clearInterval(pollTimer);
    pollTimer = null;
  }
  disposeAllCharts();
});
</script>

<template>
  <div class="max-w-7xl mx-auto space-y-5" v-loading="loading">
    <!-- 顶部栏 -->
    <div class="rounded-xl border border-slate-200 bg-white px-5 py-4 shadow-sm flex flex-wrap items-center justify-between gap-3">
      <div class="flex flex-wrap items-center gap-3">
        <el-button size="small" @click="router.push('/workers')">
          <ArrowLeft class="h-3.5 w-3.5 mr-1" />
          返回节点管理
        </el-button>
        <el-select
          :model-value="selectedId"
          size="small"
          style="width: 240px"
          @change="handleWorkerChange"
        >
          <el-option
            v-for="w in workers"
            :key="w.id"
            :label="`${w.display_name} (${w.id})`"
            :value="w.id"
          />
        </el-select>
        <template v-if="diag">
          <el-tag size="small" :type="diag.role === 'crawler' ? 'primary' : 'success'" effect="plain">
            {{ diag.role === "crawler" ? "Crawler" : "Indexer" }}
          </el-tag>
          <el-tag size="small" :type="stateTagType(diag.runtime_state)">
            {{ stateLabel(diag.runtime_state) }}
          </el-tag>
          <span class="text-xs font-mono text-slate-500">
            PID: {{ diag.pid ?? "-" }} · cfg v{{ diag.active_config_version }}
            <span v-if="diag.captured_at_unix">· 快照: {{ formatTs(diag.captured_at_unix) }}</span>
          </span>
        </template>
      </div>

      <div class="flex items-center gap-3">
        <el-switch
          v-model="autoRefresh"
          size="small"
          active-text="3s 自动刷新"
          inactive-text="已暂停"
        />
        <el-button size="small" :loading="loading" @click="fetchDiagnostics()">
          <RefreshCw class="h-3.5 w-3.5 mr-1" />
          刷新诊断
        </el-button>
      </div>
    </div>

    <el-alert
      v-if="diag && !diag.snapshot"
      type="info"
      show-icon
      :closable="false"
      title="当前节点尚未产生运行态快照（请确认 Worker 已处于运行中 Running 状态）。"
    />

    <!-- Crawler 深度诊断视图 -->
    <template v-if="crawlerSnap">
      <!-- 关键指标卡 -->
      <div class="grid grid-cols-2 sm:grid-cols-4 gap-4">
        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
          <div class="text-xs text-slate-500">采集档位 & 运行时长</div>
          <div class="text-lg font-bold font-mono text-slate-900">
            {{ crawlerSnap.crawl_profile }} · {{ formatDuration(crawlerSnap.uptime_secs) }}
          </div>
          <div class="text-[11px] text-slate-500 font-mono">
            生效并发: {{ crawlerSnap.effective_metadata_workers }} · QPS上限: {{ crawlerSnap.dht.find_node_effective_rate_per_sec ?? "-" }}/s
          </div>
        </div>

        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
          <div class="text-xs text-slate-500">内存 RSS & 本地 Moka 缓存</div>
          <div class="text-lg font-bold font-mono text-slate-900">
            {{ formatBytes(crawlerSnap.rss_bytes) }}
          </div>
          <div class="text-[11px] text-slate-500 font-mono">
            Moka 缓存条目: {{ crawlerSnap.local_cache_entries.toLocaleString("zh-CN") }}
          </div>
        </div>

        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
          <div class="text-xs text-slate-500">Redis 连接池 & 推送统计</div>
          <div class="text-lg font-bold font-mono text-slate-900">
            {{ crawlerSnap.redis_pool_available }} / {{ crawlerSnap.redis_pool_size }} 空闲
          </div>
          <div class="text-[11px] text-slate-500 font-mono">
            元数据成功: {{ crawlerSnap.app.torrents_downloaded ?? 0 }} · 推流: {{ crawlerSnap.app.torrents_pushed ?? 0 }}
          </div>
        </div>

        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
          <div class="text-xs text-slate-500">UDP 收发流量与丢包</div>
          <div class="text-lg font-bold font-mono text-slate-900">
            RX {{ formatBytes(crawlerSnap.observability.udp_rx_bytes) }}
          </div>
          <div class="text-[11px] text-slate-500 font-mono">
            TX {{ formatBytes(crawlerSnap.observability.udp_tx_bytes) }} · 队列满丢包 {{ crawlerSnap.dht.udp_queue_full ?? 0 }}
          </div>
        </div>
      </div>

      <!-- 队列与并发饱和度 -->
      <div class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-3">
        <div class="text-sm font-semibold text-slate-900 flex items-center gap-2">
          <Activity class="h-4 w-4 text-indigo-600" />
          核心队列与并发饱和度
        </div>
        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4 text-xs">
          <div class="space-y-1">
            <div class="flex justify-between">
              <span>Metadata 排队深度</span>
              <span class="font-mono">{{ crawlerSnap.dht.metadata_queue_depth ?? 0 }} / {{ crawlerSnap.dht.metadata_queue_max ?? 0 }}</span>
            </div>
            <el-progress :percentage="pct(crawlerSnap.dht.metadata_queue_depth, crawlerSnap.dht.metadata_queue_max)" />
          </div>

          <div class="space-y-1">
            <div class="flex justify-between">
              <span>Metadata Worker 并发</span>
              <span class="font-mono">{{ crawlerSnap.dht.metadata_in_flight ?? 0 }} / {{ crawlerSnap.effective_metadata_workers }}</span>
            </div>
            <el-progress :percentage="pct(crawlerSnap.dht.metadata_in_flight, crawlerSnap.effective_metadata_workers)" status="success" />
          </div>

          <div class="space-y-1">
            <div class="flex justify-between">
              <span>find_node 在途并发</span>
              <span class="font-mono">{{ crawlerSnap.dht.find_node_in_flight ?? 0 }} / {{ crawlerSnap.dht.find_node_in_flight_max ?? 0 }}</span>
            </div>
            <el-progress :percentage="pct(crawlerSnap.dht.find_node_in_flight, crawlerSnap.dht.find_node_in_flight_max)" status="warning" />
          </div>

          <div class="space-y-1">
            <div class="flex justify-between">
              <span>DHT 路由节点池</span>
              <span class="font-mono">{{ crawlerSnap.dht.node_pool_size ?? 0 }} / {{ crawlerSnap.dht.node_pool_capacity ?? 0 }}</span>
            </div>
            <el-progress :percentage="pct(crawlerSnap.dht.node_pool_size, crawlerSnap.dht.node_pool_capacity)" />
          </div>

          <div class="space-y-1">
            <div class="flex justify-between">
              <span>入口 Hash 队列</span>
              <span class="font-mono">{{ crawlerSnap.dht.hash_ingress_queue_depth ?? 0 }} / {{ crawlerSnap.dht.hash_ingress_queue_capacity ?? 0 }}</span>
            </div>
            <el-progress :percentage="pct(crawlerSnap.dht.hash_ingress_queue_depth, crawlerSnap.dht.hash_ingress_queue_capacity)" />
          </div>

          <div class="space-y-1">
            <div class="flex justify-between">
              <span>已下载元数据体积</span>
              <span class="font-mono">{{ formatBytes(crawlerSnap.observability.metadata_bytes_downloaded) }}</span>
            </div>
            <div class="text-[11px] text-slate-500 font-mono">
              坏 Peer 缓存命中: T={{ crawlerSnap.observability.peer_cache_timeout_hits }} / C={{ crawlerSnap.observability.peer_cache_connect_hits }}
            </div>
          </div>
        </div>
      </div>

      <!-- 三个直方图 -->
      <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm">
          <div class="flex items-center justify-between text-xs mb-2">
            <span class="font-semibold text-slate-800">排队等待耗时 (ms)</span>
            <span class="font-mono text-indigo-700">
              P50: {{ crawlerSnap.observability.queue_wait_ms.p50 ?? "-" }} · P95: {{ crawlerSnap.observability.queue_wait_ms.p95 ?? "-" }}
            </span>
          </div>
          <div ref="queueHistRef" class="h-56 w-full" />
        </div>

        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm">
          <div class="flex items-center justify-between text-xs mb-2">
            <span class="font-semibold text-slate-800">Metadata 抓取耗时 (ms)</span>
            <span class="font-mono text-sky-700">
              P50: {{ crawlerSnap.observability.fetch_duration_ms.p50 ?? "-" }} · P95: {{ crawlerSnap.observability.fetch_duration_ms.p95 ?? "-" }}
            </span>
          </div>
          <div ref="fetchHistRef" class="h-56 w-full" />
        </div>

        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm">
          <div class="flex items-center justify-between text-xs mb-2">
            <span class="font-semibold text-slate-800">Metadata 体积分布</span>
            <span class="font-mono text-emerald-700">
              P50: {{ formatBytes(crawlerSnap.observability.metadata_size_bytes.p50) }} · P95: {{ formatBytes(crawlerSnap.observability.metadata_size_bytes.p95) }}
            </span>
          </div>
          <div ref="sizeHistRef" class="h-56 w-full" />
        </div>
      </div>

      <!-- 超时阶段与连接错误拆解 -->
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm">
          <div class="text-xs font-semibold text-slate-800 mb-2">
            Metadata 分阶段超时拆解 (`metadata_timeout_stages`)
          </div>
          <div ref="timeoutStagesRef" class="h-64 w-full" />
        </div>

        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm">
          <div class="text-xs font-semibold text-slate-800 mb-2">
            TCP / 握手错误原因拆解 (`metadata_connection_errors`)
          </div>
          <div ref="connErrorsRef" class="h-64 w-full" />
        </div>
      </div>

      <!-- 渠道产出与应用计数器明细 -->
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-3">
          <div class="text-xs font-semibold text-slate-800">发现渠道收益与状态指标 (`yield_counters` / `yield_gauges`)</div>
          <el-table :data="yieldCounterRows" size="small" border stripe max-height="250">
            <el-table-column prop="key" label="渠道指标" min-width="200">
              <template #default="{ row }"><span class="font-mono text-xs">{{ row.key }}</span></template>
            </el-table-column>
            <el-table-column prop="value" label="累计计数" width="120">
              <template #default="{ row }"><span class="font-mono text-xs font-semibold">{{ row.value.toLocaleString("zh-CN") }}</span></template>
            </el-table-column>
          </el-table>
          <el-table v-if="yieldGaugeRows.length > 0" :data="yieldGaugeRows" size="small" border stripe max-height="180">
            <el-table-column prop="key" label="瞬时指标 (Gauge)" min-width="200">
              <template #default="{ row }"><span class="font-mono text-xs">{{ row.key }}</span></template>
            </el-table-column>
            <el-table-column prop="value" label="当前值" width="120">
              <template #default="{ row }"><span class="font-mono text-xs">{{ row.value.toLocaleString("zh-CN") }}</span></template>
            </el-table-column>
          </el-table>
        </div>

        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-3">
          <div class="text-xs font-semibold text-slate-800">Crawler 应用层原子计数器 (`app`)</div>
          <el-table :data="appStatRows" size="small" border stripe max-height="360">
            <el-table-column prop="key" label="计数器名称" min-width="220">
              <template #default="{ row }"><span class="font-mono text-xs">{{ row.key }}</span></template>
            </el-table-column>
            <el-table-column prop="value" label="数值" width="130">
              <template #default="{ row }"><span class="font-mono text-xs font-semibold">{{ row.value.toLocaleString("zh-CN") }}</span></template>
            </el-table-column>
          </el-table>
        </div>
      </div>
    </template>

    <!-- Indexer 深度诊断视图 -->
    <template v-else-if="indexerSnap">
      <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
          <div class="flex items-center justify-between text-xs text-slate-500">
            <span>运行时长 & 规则版本</span>
            <Cpu class="h-4 w-4 text-indigo-500" />
          </div>
          <div class="text-lg font-bold font-mono text-slate-900">
            v{{ indexerSnap.active_rule_version }} · {{ formatDuration(indexerSnap.uptime_secs) }}
          </div>
          <div class="text-[11px] text-slate-500 font-mono">
            内存 RSS: {{ formatBytes(indexerSnap.rss_bytes) }}
          </div>
        </div>

        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
          <div class="flex items-center justify-between text-xs text-slate-500">
            <span>Redis Stream 读取与 ACK</span>
            <Wifi class="h-4 w-4 text-sky-500" />
          </div>
          <div class="text-lg font-bold font-mono text-slate-900">
            读取 {{ indexerSnap.app.torrent_messages_read }} / {{ indexerSnap.app.hot_messages_read }}
          </div>
          <div class="text-[11px] text-slate-500 font-mono">
            XAUTOCLAIM: new={{ indexerSnap.app.reclaimed_new }} hot={{ indexerSnap.app.reclaimed_hot }} · 毒丸={{ indexerSnap.app.poison_messages_acked }}
          </div>
        </div>

        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
          <div class="flex items-center justify-between text-xs text-slate-500">
            <span>过滤判定汇总</span>
            <Activity class="h-4 w-4 text-amber-500" />
          </div>
          <div class="text-lg font-bold font-mono text-slate-900">
            放行 {{ indexerSnap.app.filter_accepted }} / 拦截 {{ indexerSnap.app.filter_rejected }}
          </div>
          <div class="text-[11px] text-slate-500 font-mono">
            在途待刷盘: buf={{ indexerSnap.app.torrent_buffered }} · inflight={{ indexerSnap.app.torrent_in_flight }}
          </div>
        </div>

        <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
          <div class="flex items-center justify-between text-xs text-slate-500">
            <span>Manticore 写库 & Redis 回写</span>
            <HardDrive class="h-4 w-4 text-emerald-500" />
          </div>
          <div class="text-lg font-bold font-mono text-slate-900">
            入库 {{ indexerSnap.app.records_written }} · Hot {{ indexerSnap.app.hot_records_updated }}
          </div>
          <div class="text-[11px] text-slate-500 font-mono">
            批次 成功={{ indexerSnap.app.batches_written }} 失败={{ indexerSnap.app.batches_failed }}
          </div>
        </div>
      </div>
    </template>
  </div>
</template>
