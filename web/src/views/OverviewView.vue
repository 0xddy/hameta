<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { RouterLink } from "vue-router";
import {
  Activity,
  ArrowRight,
  HardDrive,
  RefreshCw,
} from "lucide-vue-next";
import {
  getOverviewHistory,
  type HistoryPoint,
  type HistoryRange,
  type WorkerRuntimeState,
} from "@/api/overview";
import ObsTrendChart, { type TrendChartSeries } from "@/components/ui/ObsTrendChart.vue";
import { useTelemetryStore } from "@/stores/telemetry";
import {
  fmtBps,
  fmtBytes,
  fmtCount,
  fmtExact,
  fmtRate,
} from "@/utils/format";

const telemetry = useTelemetryStore();

const range = ref<HistoryRange>("1h");
const historyLoading = ref(false);
const historyGranularity = ref<"minute" | "hour">("minute");
const historyPoints = ref<HistoryPoint[]>([]);
let historyTimer: number | null = null;

const RANGE_OPTIONS: Array<{ value: HistoryRange; label: string }> = [
  { value: "1h", label: "1h" },
  { value: "6h", label: "6h" },
  { value: "24h", label: "24h" },
  { value: "7d", label: "7d" },
  { value: "30d", label: "30d" },
];

const snap = computed(() => telemetry.snapshot);
const kpi = computed(() => snap.value?.kpi);
const health = computed(() => snap.value?.health);
const funnel = computed(() => snap.value?.funnel);
const workers = computed(() => snap.value?.workers ?? []);

function buildSparkline(values: number[]): { line: string; area: string } {
  const w = 96;
  const h = 40;
  const slice = values.slice(-24);
  if (slice.length < 2) {
    return {
      line: `M0 24 L${w} 24`,
      area: `M0 24 L${w} 24 L${w} ${h} L0 ${h} Z`,
    };
  }
  let max = 0;
  let min = Number.POSITIVE_INFINITY;
  for (const v of slice) {
    if (v > max) max = v;
    if (v < min) min = v;
  }
  const span = Math.max(1e-6, max - min);
  const pts = slice.map((v, i) => {
    const x = (i / Math.max(1, slice.length - 1)) * w;
    const norm = max === min ? 0.5 : (v - min) / span;
    const y = h - 7 - norm * (h - 14);
    return { x, y };
  });
  const line = pts.map((p, i) => `${i === 0 ? "M" : "L"}${p.x.toFixed(1)} ${p.y.toFixed(1)}`).join(" ");
  const area = `${line} L${pts[pts.length - 1].x.toFixed(1)} ${h} L0 ${h} Z`;
  return { line, area };
}

const qpsSpark = computed(() => buildSparkline(historyPoints.value.map((p) => p.actual_qps)));
const writeSpark = computed(() => buildSparkline(historyPoints.value.map((p) => p.indexed_written_per_min)));
const lagSpark = computed(() => buildSparkline(historyPoints.value.map((p) => p.stream_new_lag)));
const metaBwSpark = computed(() => buildSparkline(historyPoints.value.map((p) => p.metadata_rx_bps)));

const docAvgSize = computed(() => {
  const k = kpi.value;
  if (!k || k.torrents_disk_bytes == null || !k.torrents_total_docs || k.torrents_total_docs <= 0) {
    return "—";
  }
  return `${fmtBytes(k.torrents_disk_bytes / k.torrents_total_docs)} / doc`;
});

const hotPersistPct = computed(() => {
  const f = funnel.value;
  if (!f || f.hot_pushed_per_min <= 0) return "—";
  return `${((f.hot_updated_per_min / f.hot_pushed_per_min) * 100).toFixed(1)}%`;
});

const funnelStages = computed(() => {
  const f = funnel.value;
  if (!f) return [];
  const base = Math.max(1, f.hashes_per_min);
  const pct = (v: number) => Math.min(100, Math.round((v / base) * 100));

  return [
    {
      no: "01",
      sub: "DHT 探测",
      name: "Hash 发现",
      val: f.hashes_per_min,
      foot: "基线入口",
      pct: f.hashes_per_min > 0 ? 100 : 0,
      noCls: "text-indigo-400",
      barCls: "bg-indigo-500",
      boxCls: "bg-[#0c1019] border-slate-800/90",
    },
    {
      no: "02",
      sub: "缓存跳过",
      name: "去重拦截",
      val: f.dedup_blocked_per_min,
      foot: "Hit Cache",
      pct: pct(f.dedup_blocked_per_min),
      noCls: "text-slate-400",
      barCls: "bg-slate-500",
      boxCls: "bg-[#0c1019] border-slate-800/90",
    },
    {
      no: "03",
      sub: "BEP-9",
      name: "元数据下载",
      val: f.metadata_succeeded_per_min,
      foot: "抓取成功",
      pct: pct(f.metadata_succeeded_per_min),
      noCls: "text-sky-400",
      barCls: "bg-sky-500",
      boxCls: "bg-[#0c1019] border-slate-800/90",
    },
    {
      no: "04",
      sub: "Redis Streams",
      name: "进入队列",
      val: f.stream_pushed_per_min,
      foot: "XADD 写入",
      pct: pct(f.stream_pushed_per_min),
      noCls: "text-cyan-400",
      barCls: "bg-cyan-500",
      boxCls: "bg-[#0c1019] border-slate-800/90",
    },
    {
      no: "05",
      sub: `拦截 ${fmtRate(f.filter_rejected_per_min)}/m`,
      name: "过滤放行",
      val: f.filter_accepted_per_min,
      foot: "放行率",
      pct: pct(f.filter_accepted_per_min),
      noCls: "text-amber-400",
      barCls: "bg-amber-500",
      boxCls: "bg-[#0c1019] border-slate-800/90",
    },
    {
      no: "06",
      sub: `Hot ${fmtRate(f.hot_updated_per_min)}/m`,
      name: "Manticore 入库",
      val: f.indexed_written_per_min,
      foot: "净入库",
      pct: pct(f.indexed_written_per_min),
      noCls: "text-emerald-400",
      barCls: "bg-emerald-400",
      boxCls: "bg-emerald-950/25 border-emerald-500/30",
    },
  ];
});

const conversionRate = computed(() => {
  const f = funnel.value;
  if (!f || f.hashes_per_min <= 0) return "0.00%";
  return `${Math.min(100, (f.indexed_written_per_min / f.hashes_per_min) * 100).toFixed(2)}%`;
});

const chartLabels = computed(() =>
  historyPoints.value.map((pt) => {
    const d = new Date(pt.ts * 1000);
    const pad2 = (n: number) => String(n).padStart(2, "0");
    if (range.value === "7d" || range.value === "30d") {
      return `${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:00`;
    }
    return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
  }),
);

const ingestSeries = computed<TrendChartSeries[]>(() => [
  {
    name: "Metadata",
    color: "#38bdf8",
    values: historyPoints.value.map((p) => p.metadata_succeeded_per_min),
    fmt: (v) => `${fmtCount(v)}/min`,
  },
  {
    name: "入库",
    color: "#34d399",
    area: true,
    values: historyPoints.value.map((p) => p.indexed_written_per_min),
    fmt: (v) => `${fmtCount(v)}/min`,
  },
  {
    name: "拦截",
    color: "#fbbf24",
    dash: true,
    values: historyPoints.value.map((p) => p.filtered_per_min),
    fmt: (v) => `${fmtCount(v)}/min`,
  },
]);

const qpsMaxAxis = computed(() => {
  const maxQps = Math.max(0, ...historyPoints.value.map((p) => p.actual_qps));
  if (maxQps > 0) return maxQps * 1.15;
  const hasLagOrBw = historyPoints.value.some((p) => p.stream_new_lag > 0 || p.metadata_rx_bps > 0);
  return hasLagOrBw ? 100 : 10;
});

const opsSeries = computed<TrendChartSeries[]>(() => {
  const pts = historyPoints.value;
  const maxQps = qpsMaxAxis.value;
  const rawLag = pts.map((p) => p.stream_new_lag);
  const rawBw = pts.map((p) => p.metadata_rx_bps);
  const maxLag = Math.max(1, ...rawLag);
  const maxBw = Math.max(1, ...rawBw);

  return [
    {
      name: "DHT QPS",
      color: "#2dd4bf",
      area: true,
      values: pts.map((p) => p.actual_qps),
      raw: pts.map((p) => p.actual_qps),
      fmt: (v) => `${fmtRate(v)} qps`,
    },
    {
      name: "Lag",
      color: "#fb7185",
      values: rawLag.map((v) => (v <= 0 ? 0 : (v / maxLag) * maxQps * 0.78)),
      raw: rawLag,
      fmt: (v) => fmtCount(v),
    },
    {
      name: "带宽",
      color: "#a78bfa",
      values: rawBw.map((v) => (v <= 0 ? 0 : (v / maxBw) * maxQps * 0.68)),
      raw: rawBw,
      fmt: (v) => fmtBps(v),
    },
  ];
});

async function loadHistory(silent = false) {
  if (!silent) historyLoading.value = true;
  try {
    const res = await getOverviewHistory(range.value);
    historyGranularity.value = res.granularity;
    historyPoints.value = res.points;
  } catch {
    // retain previous points if transient poll fails
  } finally {
    historyLoading.value = false;
  }
}

function selectRange(next: HistoryRange) {
  if (range.value === next) return;
  range.value = next;
  void loadHistory();
}

function stateBadge(state: WorkerRuntimeState): { text: string; cls: string; dot: string } {
  switch (state) {
    case "running":
      return {
        text: "运行中",
        cls: "bg-emerald-500/10 text-emerald-300 border-emerald-500/30",
        dot: "bg-emerald-400",
      };
    case "starting":
      return {
        text: "启动中",
        cls: "bg-sky-500/10 text-sky-300 border-sky-500/30",
        dot: "bg-sky-400 animate-pulse",
      };
    case "draining":
      return {
        text: "排空中",
        cls: "bg-amber-500/10 text-amber-300 border-amber-500/30",
        dot: "bg-amber-400",
      };
    case "crashed":
      return {
        text: "异常退出",
        cls: "bg-rose-500/10 text-rose-300 border-rose-500/30",
        dot: "bg-rose-400",
      };
    default:
      return {
        text: "已停止",
        cls: "bg-slate-800/70 text-slate-400 border-slate-700/60",
        dot: "bg-slate-500",
      };
  }
}

onMounted(() => {
  telemetry.connect();
  void loadHistory();
  historyTimer = window.setInterval(() => {
    void loadHistory(true);
  }, 30_000);
});

onUnmounted(() => {
  telemetry.disconnect();
  if (historyTimer !== null) {
    window.clearInterval(historyTimer);
    historyTimer = null;
  }
});
</script>

<template>
  <div class="space-y-4">
    <!-- 6 个抗挤压自适应单位 KPI 卡片 -->
    <div class="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-4">
      <!-- 1. DHT 探测 QPS -->
      <div class="obs-card obs-card-hover rounded-xl p-4 space-y-3 min-w-0">
        <div class="flex items-center justify-between gap-2">
          <span class="text-xs font-medium text-slate-400">DHT 探测 QPS</span>
          <span
            class="font-mono-num text-[11px] px-1.5 py-0.5 rounded border shrink-0"
            :class="
              (health?.running_crawlers ?? 0) > 0
                ? 'bg-emerald-500/10 text-emerald-300 border-emerald-500/20'
                : 'bg-slate-800/80 text-slate-400 border-slate-700/60'
            "
          >
            {{ (health?.running_crawlers ?? 0) > 0 ? "LIVE" : "IDLE" }}
          </span>
        </div>
        <div class="flex items-end justify-between gap-3 min-w-0">
          <div class="min-w-0">
            <div
              class="text-2xl font-bold font-mono-num text-slate-50 tracking-tight truncate"
              :title="kpi ? fmtExact(kpi.actual_qps, 'req/s') : undefined"
            >
              {{ kpi ? fmtRate(kpi.actual_qps) : "—" }}
              <span class="text-xs font-normal text-slate-400">req/s</span>
            </div>
            <div class="text-[11px] text-slate-500 font-mono-num mt-0.5 truncate">
              {{ health?.running_crawlers ?? 0 }} 个 Crawler 子进程聚合
            </div>
          </div>
          <svg class="w-24 h-10 shrink-0" viewBox="0 0 96 40">
            <defs>
              <linearGradient id="sp_qps" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stop-color="#10b981" stop-opacity="0.35" />
                <stop offset="100%" stop-color="#10b981" stop-opacity="0" />
              </linearGradient>
            </defs>
            <path :d="qpsSpark.area" fill="url(#sp_qps)" />
            <path :d="qpsSpark.line" fill="none" stroke="#10b981" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        </div>
        <div class="pt-2.5 border-t border-slate-800/80 flex flex-wrap items-center justify-between gap-2 text-[11px] font-mono-num">
          <span class="text-slate-500">RLIMIT_NOFILE</span>
          <span
            class="text-slate-300"
            :title="kpi?.system_soft_nofile != null ? fmtExact(kpi.system_soft_nofile) : undefined"
          >
            {{ kpi?.system_soft_nofile != null ? fmtCount(kpi.system_soft_nofile) : "N/A" }}
            <span v-if="kpi?.system_soft_nofile != null" class="text-emerald-400">OK</span>
          </span>
        </div>
      </div>

      <!-- 2. Manticore 入库速率 -->
      <div class="obs-card obs-card-hover rounded-xl p-4 space-y-3 min-w-0">
        <div class="flex items-center justify-between gap-2">
          <span class="text-xs font-medium text-slate-400">Manticore 入库速率</span>
          <span class="font-mono-num text-[11px] px-1.5 py-0.5 rounded bg-cyan-500/10 text-cyan-300 border border-cyan-500/20 shrink-0">
            稳态
          </span>
        </div>
        <div class="flex items-end justify-between gap-3 min-w-0">
          <div class="min-w-0">
            <div
              class="text-2xl font-bold font-mono-num text-slate-50 tracking-tight truncate"
              :title="kpi ? fmtExact(kpi.indexed_written_per_min, 'docs/min') : undefined"
            >
              {{ kpi ? fmtRate(kpi.indexed_written_per_min) : "—" }}
              <span class="text-xs font-normal text-slate-400">/min</span>
            </div>
            <div class="text-[11px] text-slate-500 font-mono-num mt-0.5 truncate">
              {{ health?.running_indexers ?? 0 }} 个 Indexer 净入库
            </div>
          </div>
          <svg class="w-24 h-10 shrink-0" viewBox="0 0 96 40">
            <path :d="writeSpark.line" fill="none" stroke="#06b6d4" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        </div>
        <div class="pt-2.5 border-t border-slate-800/80 flex flex-wrap items-center justify-between gap-x-3 gap-y-1 text-[11px] font-mono-num">
          <span
            class="text-slate-500"
            :title="kpi ? `今日精确入库: ${fmtExact(kpi.today_indexed_docs)}` : undefined"
          >
            今日 <strong class="text-slate-200">{{ kpi ? fmtCount(kpi.today_indexed_docs) : "—" }}</strong>
          </span>
          <span
            class="text-slate-500"
            :title="kpi ? `日预估精确值: ${fmtExact(kpi.estimated_daily_docs)}` : undefined"
          >
            日预估 <strong class="text-emerald-400">{{ kpi ? fmtCount(kpi.estimated_daily_docs) : "—" }}</strong>
          </span>
        </div>
      </div>

      <!-- 3. 磁力索引总量 -->
      <div class="obs-card obs-card-hover rounded-xl p-4 space-y-3 min-w-0">
        <div class="flex items-center justify-between gap-2">
          <span class="text-xs font-medium text-slate-400">磁力索引总量</span>
          <span class="font-mono-num text-[11px] text-slate-500">RT TABLE</span>
        </div>
        <div class="flex items-end justify-between gap-3 min-w-0">
          <div class="min-w-0">
            <div
              class="text-2xl font-bold font-mono-num text-slate-50 tracking-tight truncate"
              :title="kpi?.torrents_total_docs != null ? `精确文档数: ${fmtExact(kpi.torrents_total_docs)}` : undefined"
            >
              {{ kpi?.torrents_total_docs != null ? fmtCount(kpi.torrents_total_docs) : "—" }}
              <span class="text-xs font-normal text-slate-400">docs</span>
            </div>
            <div class="text-[11px] text-slate-500 font-mono-num mt-0.5">torrents · 已清洗</div>
          </div>
          <div class="text-right shrink-0">
            <div class="text-xs font-mono-num text-slate-300">
              {{ kpi?.torrents_disk_bytes != null ? fmtBytes(kpi.torrents_disk_bytes) : "—" }}
            </div>
            <div class="text-[10px] text-slate-500">磁盘占用</div>
          </div>
        </div>
        <div class="pt-2.5 border-t border-slate-800/80 flex items-center justify-between text-[11px] font-mono-num">
          <span class="text-slate-500">单条均值</span>
          <span class="text-slate-300">{{ docAvgSize }}</span>
        </div>
      </div>

      <!-- 4. 队列积压 (Stream Lag) -->
      <div class="obs-card obs-card-hover rounded-xl p-4 space-y-3 min-w-0">
        <div class="flex items-center justify-between gap-2">
          <span class="text-xs font-medium text-slate-400">队列积压 (Stream Lag)</span>
          <span
            class="font-mono-num text-[11px] px-1.5 py-0.5 rounded border"
            :class="
              (kpi?.stream_new_lag ?? 0) > 100000
                ? 'bg-amber-500/15 text-amber-300 border-amber-500/30'
                : 'bg-emerald-500/10 text-emerald-300 border-emerald-500/20'
            "
          >
            {{ (kpi?.stream_new_lag ?? 0) > 100000 ? "高水位" : "健康" }}
          </span>
        </div>
        <div class="flex items-end justify-between gap-3 min-w-0">
          <div class="min-w-0">
            <div
              class="text-2xl font-bold font-mono-num text-slate-50 tracking-tight truncate"
              :title="kpi ? `精确 Lag: ${fmtExact(kpi.stream_new_lag)}` : undefined"
            >
              {{ kpi ? fmtCount(kpi.stream_new_lag) : "—" }}
            </div>
            <div class="text-[11px] text-slate-500 font-mono-num mt-0.5">未消费消息</div>
          </div>
          <svg class="w-24 h-10 shrink-0" viewBox="0 0 96 40">
            <path :d="lagSpark.line" fill="none" stroke="#f59e0b" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        </div>
        <div class="pt-2.5 border-t border-slate-800/80 flex flex-wrap items-center justify-between gap-x-2 gap-y-1 text-[11px] font-mono-num text-slate-400">
          <span :title="kpi ? `new: ${fmtExact(kpi.stream_new_len)}` : undefined">
            new <strong class="text-slate-200">{{ kpi ? fmtCount(kpi.stream_new_len) : "—" }}</strong>
          </span>
          <span :title="kpi ? `hot: ${fmtExact(kpi.stream_hot_len)}` : undefined">
            hot <strong class="text-slate-200">{{ kpi ? fmtCount(kpi.stream_hot_len) : "—" }}</strong>
          </span>
          <span :title="kpi ? `pel: ${fmtExact(kpi.stream_new_pending)}` : undefined">
            pel <strong class="text-amber-300">{{ kpi ? fmtCount(kpi.stream_new_pending) : "—" }}</strong>
          </span>
        </div>
      </div>

      <!-- 5. BEP-9 元数据带宽 -->
      <div class="obs-card obs-card-hover rounded-xl p-4 space-y-3 min-w-0">
        <div class="flex items-center justify-between gap-2">
          <span class="text-xs font-medium text-slate-400">BEP-9 元数据带宽</span>
          <span class="font-mono-num text-[11px] text-cyan-400">TCP + UDP</span>
        </div>
        <div class="flex items-end justify-between gap-3 min-w-0">
          <div class="min-w-0">
            <div class="text-2xl font-bold font-mono-num text-slate-50 tracking-tight truncate">
              {{ kpi ? fmtBps(kpi.metadata_rx_bps) : "—" }}
            </div>
            <div class="text-[11px] text-slate-500 font-mono-num mt-0.5">元数据下载吞吐</div>
          </div>
          <svg class="w-24 h-10 shrink-0" viewBox="0 0 96 40">
            <path :d="metaBwSpark.line" fill="none" stroke="#818cf8" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        </div>
        <div class="pt-2.5 border-t border-slate-800/80 flex flex-wrap items-center justify-between gap-2 text-[11px] font-mono-num text-slate-400">
          <span>UDP RX <strong class="text-slate-200">{{ kpi ? fmtBps(kpi.udp_rx_bps) : "—" }}</strong></span>
          <span>TX <strong class="text-slate-200">{{ kpi ? fmtBps(kpi.udp_tx_bps) : "—" }}</strong></span>
        </div>
      </div>

      <!-- 6. 子进程内存 (Total RSS) -->
      <div class="obs-card obs-card-hover rounded-xl p-4 space-y-3 min-w-0">
        <div class="flex items-center justify-between gap-2">
          <span class="text-xs font-medium text-slate-400">子进程内存 (Total RSS)</span>
          <span class="font-mono-num text-[11px] text-slate-400">mimalloc</span>
        </div>
        <div class="flex items-end justify-between gap-3 min-w-0">
          <div class="min-w-0">
            <div class="text-2xl font-bold font-mono-num text-slate-50 tracking-tight truncate">
              {{ kpi ? fmtBytes(kpi.total_rss_bytes) : "—" }}
            </div>
            <div class="text-[11px] text-slate-500 font-mono-num mt-0.5">
              {{ telemetry.runningWorkerCount }} 个运行中 Worker 子进程
            </div>
          </div>
          <div class="text-right font-mono-num shrink-0">
            <div class="text-xs text-emerald-400">{{ hotPersistPct }}</div>
            <div class="text-[10px] text-slate-500">Hot 落库率</div>
          </div>
        </div>
        <div class="pt-2.5 border-t border-slate-800/80 flex flex-wrap items-center justify-between gap-2 text-[11px] font-mono-num text-slate-400">
          <span>
            Hot 推 <strong class="text-slate-200">{{ funnel ? `${fmtRate(funnel.hot_pushed_per_min)}/m` : "—" }}</strong>
          </span>
          <span>
            落库 <strong class="text-emerald-400">{{ funnel ? `${fmtRate(funnel.hot_updated_per_min)}/m` : "—" }}</strong>
          </span>
        </div>
      </div>
    </div>

    <!-- 实时数据流拓扑漏斗 -->
    <section class="obs-card rounded-xl p-5 space-y-4">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="flex flex-wrap items-center gap-2.5">
          <span class="h-2 w-2 rounded-full bg-cyan-400"></span>
          <h2 class="text-sm font-semibold text-slate-100">全链路实时吞吐漏斗</h2>
          <span class="text-xs font-mono-num text-slate-500">
            DHT → BEP-9 → Redis Streams → Filter → Manticore
          </span>
        </div>
        <span class="text-[11px] font-mono-num text-slate-400">
          端到端净转化率 <strong class="text-emerald-400">{{ conversionRate }}</strong>
        </span>
      </div>

      <div class="relative h-[2px] w-full bg-slate-800/90 rounded-full overflow-hidden">
        <div class="absolute inset-0 flow-shimmer"></div>
      </div>

      <div v-if="funnelStages.length > 0" class="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-3">
        <div
          v-for="stage in funnelStages"
          :key="stage.no"
          class="rounded-xl border p-3.5 space-y-2.5 min-w-0"
          :class="stage.boxCls"
        >
          <div class="flex items-center justify-between text-[11px] gap-1">
            <span class="font-mono-num" :class="stage.noCls">{{ stage.no }}</span>
            <span class="text-slate-500 truncate">{{ stage.sub }}</span>
          </div>
          <div class="text-xs font-medium text-slate-300 truncate">{{ stage.name }}</div>
          <div
            class="text-xl font-bold font-mono-num text-slate-50 truncate"
            :title="fmtExact(stage.val, '/min')"
          >
            {{ fmtRate(stage.val) }}<span class="text-[11px] font-normal text-slate-500">/m</span>
          </div>
          <div class="h-1.5 rounded-full bg-slate-800 overflow-hidden">
            <div
              class="h-full rounded-full transition-all duration-500"
              :class="stage.barCls"
              :style="{ width: `${Math.max(4, stage.pct)}%` }"
            ></div>
          </div>
          <div class="flex justify-between text-[11px] font-mono-num text-slate-500">
            <span>{{ stage.foot }}</span>
            <span class="text-slate-300">{{ stage.pct }}%</span>
          </div>
        </div>
      </div>

      <div v-else class="py-10 text-center text-xs text-slate-500 font-mono-num">
        正在建立实时遥测管道连接…
      </div>
    </section>

    <!-- 趋势图表 -->
    <section class="obs-card rounded-xl p-5 space-y-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <div class="flex items-center gap-2">
            <h2 class="text-sm font-semibold text-slate-100">实时运行趋势</h2>
            <span class="text-[10px] font-mono-num px-2 py-0.5 rounded bg-slate-800/80 border border-slate-700/70 text-slate-300">
              {{ historyGranularity === "hour" ? "小时切片" : "分钟切片" }}
            </span>
            <RefreshCw v-if="historyLoading" class="w-3.5 h-3.5 text-emerald-400 animate-spin" />
          </div>
          <p class="text-[11px] text-slate-500 mt-0.5">
            纵轴自动升级单位 (k / M · KB/s → GB/s) · 移动鼠标查看切片详情
          </p>
        </div>

        <div class="inline-flex p-0.5 rounded-lg bg-[#090c12] border border-slate-800 text-xs font-mono-num">
          <button
            v-for="opt in RANGE_OPTIONS"
            :key="opt.value"
            type="button"
            class="px-2.5 py-1 rounded-md cursor-pointer transition"
            :class="
              range === opt.value
                ? 'bg-emerald-500/20 text-emerald-300'
                : 'text-slate-400 hover:text-slate-200'
            "
            @click="selectRange(opt.value)"
          >
            {{ opt.label }}
          </button>
        </div>
      </div>

      <div class="grid grid-cols-1 xl:grid-cols-2 gap-4">
        <div class="rounded-xl bg-[#0a0e16] border border-slate-800/80 p-4 space-y-3">
          <div class="flex flex-wrap items-center justify-between gap-2 text-xs">
            <span class="font-medium text-slate-300">采集与入库 (/min)</span>
            <div class="flex items-center gap-3 text-[11px] text-slate-400 font-mono-num">
              <span class="flex items-center gap-1"><i class="inline-block w-2 h-2 rounded-full bg-sky-400"></i>Metadata</span>
              <span class="flex items-center gap-1"><i class="inline-block w-2 h-2 rounded-full bg-emerald-400"></i>入库</span>
              <span class="flex items-center gap-1"><i class="inline-block w-2 h-2 rounded-full bg-amber-400"></i>拦截</span>
            </div>
          </div>
          <ObsTrendChart
            :labels="chartLabels"
            :series="ingestSeries"
            height-class="h-56"
            empty-text="当前时间窗口暂无采集与入库趋势采样"
          />
        </div>

        <div class="rounded-xl bg-[#0a0e16] border border-slate-800/80 p-4 space-y-3">
          <div class="flex flex-wrap items-center justify-between gap-2 text-xs">
            <span class="font-medium text-slate-300">QPS、队列积压与带宽</span>
            <div class="flex items-center gap-3 text-[11px] text-slate-400 font-mono-num">
              <span class="flex items-center gap-1"><i class="inline-block w-2 h-2 rounded-full bg-cyan-400"></i>DHT QPS</span>
              <span class="flex items-center gap-1"><i class="inline-block w-2 h-2 rounded-full bg-rose-400"></i>Lag</span>
              <span class="flex items-center gap-1"><i class="inline-block w-2 h-2 rounded-full bg-violet-400"></i>带宽</span>
            </div>
          </div>
          <ObsTrendChart
            :labels="chartLabels"
            :series="opsSeries"
            :max-val="qpsMaxAxis"
            height-class="h-56"
            empty-text="当前时间窗口暂无 QPS 与队列积压趋势采样"
          />
        </div>
      </div>
    </section>

    <!-- 受管 Worker 子进程表 -->
    <section class="obs-card rounded-xl overflow-hidden">
      <div class="px-5 py-3.5 border-b border-slate-800/80 flex items-center justify-between">
        <div class="flex items-center gap-2">
          <Activity class="w-4 h-4 text-emerald-400" />
          <h2 class="text-sm font-semibold text-slate-100">受管 Worker 子进程</h2>
          <span class="font-mono-num text-[11px] px-1.5 py-0.5 rounded bg-slate-800 text-slate-400">
            {{ telemetry.runningWorkerCount }} ACTIVE · LOOPBACK IPC
          </span>
        </div>

        <RouterLink
          to="/workers"
          class="text-xs text-emerald-400 hover:text-emerald-300 font-medium cursor-pointer"
        >
          管理子进程 →
        </RouterLink>
      </div>

      <div v-if="workers.length > 0" class="overflow-x-auto">
        <table class="w-full text-left text-xs">
          <thead>
            <tr class="border-b border-slate-800/80 bg-[#0b0f17] text-[11px] font-mono-num uppercase text-slate-500">
              <th class="py-2.5 px-5">Worker ID</th>
              <th class="py-2.5 px-3">角色</th>
              <th class="py-2.5 px-3">状态</th>
              <th class="py-2.5 px-3">运行状态摘要</th>
              <th class="py-2.5 px-3">PID</th>
              <th class="py-2.5 px-3">RSS</th>
              <th class="py-2.5 px-5 text-right">快捷操作</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-slate-800/60">
            <tr
              v-for="w in workers"
              :key="w.id"
              class="hover:bg-slate-800/30 transition"
            >
              <td class="py-3 px-5 font-mono-num font-semibold text-slate-100">
                {{ w.id }}
                <span class="block text-[11px] font-sans font-normal text-slate-500">{{ w.display_name }}</span>
              </td>
              <td class="py-3 px-3">
                <span
                  class="px-2 py-0.5 rounded font-mono-num text-[11px] border"
                  :class="
                    w.role === 'crawler'
                      ? 'bg-cyan-500/10 text-cyan-300 border-cyan-500/25'
                      : 'bg-emerald-500/10 text-emerald-300 border-emerald-500/25'
                  "
                >
                  {{ w.role === "crawler" ? "Crawler" : "Indexer" }}
                </span>
              </td>
              <td class="py-3 px-3">
                <span
                  class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-[11px] border"
                  :class="stateBadge(w.runtime_state).cls"
                >
                  <span class="h-1.5 w-1.5 rounded-full" :class="stateBadge(w.runtime_state).dot"></span>
                  {{ stateBadge(w.runtime_state).text }}
                </span>
              </td>
              <td class="py-3 px-3 font-mono-num max-w-xs">
                <span
                  v-if="w.last_error"
                  class="text-rose-300 truncate block"
                  :title="w.last_error"
                >
                  {{ w.last_error }}
                </span>
                <span v-else-if="w.restart_count > 0" class="text-amber-300">
                  自动拉起 · 已重启 {{ w.restart_count }} 次
                </span>
                <span v-else class="text-slate-400">
                  {{ w.runtime_state === "running" ? "运行正常 · IPC 心跳稳态" : "等待拉起" }}
                </span>
              </td>
              <td class="py-3 px-3 font-mono-num text-slate-400">
                {{ w.pid ?? "-" }}
              </td>
              <td class="py-3 px-3 font-mono-num text-slate-200">
                {{ w.rss_bytes > 0 ? fmtBytes(w.rss_bytes) : "-" }}
              </td>
              <td class="py-3 px-5 text-right whitespace-nowrap space-x-1.5">
                <RouterLink
                  :to="`/workers/${encodeURIComponent(w.id)}/diagnostics`"
                  class="obs-btn-ghost px-2.5 py-1 rounded text-[11px] inline-block cursor-pointer"
                >
                  诊断
                </RouterLink>
                <RouterLink
                  :to="`/workers?tune=${encodeURIComponent(w.id)}`"
                  class="obs-btn-ghost px-2.5 py-1 rounded text-[11px] inline-block cursor-pointer"
                >
                  调优
                </RouterLink>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <div v-else class="p-8 text-center">
        <HardDrive class="w-7 h-7 text-slate-600 mx-auto mb-2" />
        <div class="text-xs text-slate-300">尚未创建任何受管 Worker 子进程</div>
        <div class="text-[11px] text-slate-500 mt-1 mb-3">
          前往进程控制台创建 DHT Crawler 或 Manticore Indexer 实例即可开始采集与写入
        </div>
        <RouterLink to="/workers" class="obs-btn obs-btn-primary px-3.5 py-1.5 text-xs inline-flex items-center gap-1.5">
          创建 Worker 实例
          <ArrowRight class="w-3.5 h-3.5" />
        </RouterLink>
      </div>
    </section>
  </div>
</template>
