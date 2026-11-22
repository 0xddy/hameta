<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import * as echarts from "echarts";
import {
  Activity,
  AlertTriangle,
  ArrowRight,
  CheckCircle2,
  Cpu,
  Database,
  HardDrive,
  RefreshCw,
  ServerCog,
  Wifi,
} from "lucide-vue-next";
import { ElMessage } from "element-plus";
import {
  getOverviewHistory,
  getOverviewSnapshot,
  type HistoryPoint,
  type HistoryRange,
  type OverviewSnapshot,
  type WorkerRuntimeState,
} from "@/api/overview";

const router = useRouter();

const loadingSnapshot = ref(true);
const loadingHistory = ref(false);
const snapshot = ref<OverviewSnapshot | null>(null);
const historyPoints = ref<HistoryPoint[]>([]);
const historyGranularity = ref<"minute" | "hour">("minute");
const selectedRange = ref<HistoryRange>("1h");
const sseStatus = ref<"connecting" | "connected" | "reconnecting">("connecting");

const throughputChartRef = ref<HTMLDivElement | null>(null);
const capacityChartRef = ref<HTMLDivElement | null>(null);
let throughputChart: echarts.ECharts | null = null;
let capacityChart: echarts.ECharts | null = null;
let eventSource: EventSource | null = null;
let reconnectTimer: number | null = null;
let reconnectAttempt = 0;

const rangeOptions: Array<{ label: string; value: HistoryRange }> = [
  { label: "1 小时", value: "1h" },
  { label: "6 小时", value: "6h" },
  { label: "24 小时", value: "24h" },
  { label: "7 天", value: "7d" },
  { label: "30 天", value: "30d" },
];

const hasCriticalAlert = computed(() => {
  const h = snapshot.value?.health;
  if (!h) return false;
  return !h.redis_ok || !h.manticore_ok || h.crashed_workers > 0;
});

const funnelStages = computed(() => {
  const f = snapshot.value?.funnel;
  if (!f) return [];
  const base = Math.max(f.hashes_per_min, 1);
  return [
    {
      key: "hashes",
      label: "DHT Hash 发现",
      value: f.hashes_per_min,
      sub: "get_peers / announce",
      pct: 100,
      color: "bg-indigo-500",
    },
    {
      key: "dedup",
      label: "去重与占坑拦截",
      value: f.dedup_blocked_per_min,
      sub: "本地/Redis 命中跳过",
      pct: Math.min(100, Math.round((f.dedup_blocked_per_min / base) * 100)),
      color: "bg-slate-400",
    },
    {
      key: "metadata",
      label: "Metadata 下载成功",
      value: f.metadata_succeeded_per_min,
      sub: "BEP-9 元数据抓取",
      pct: Math.min(100, Math.round((f.metadata_succeeded_per_min / base) * 100)),
      color: "bg-sky-500",
    },
    {
      key: "stream",
      label: "推入 Redis Streams",
      value: f.stream_pushed_per_min,
      sub: "待 Indexer 消费",
      pct: Math.min(100, Math.round((f.stream_pushed_per_min / base) * 100)),
      color: "bg-cyan-500",
    },
    {
      key: "filter",
      label: "过滤规则判定",
      value: f.filter_accepted_per_min,
      sub: `拦截 ${formatRate(f.filter_rejected_per_min)} /min`,
      pct: Math.min(100, Math.round((f.filter_accepted_per_min / base) * 100)),
      color: "bg-amber-500",
    },
    {
      key: "indexed",
      label: "Manticore 实际入库",
      value: f.indexed_written_per_min,
      sub: `Hot 更新 ${formatRate(f.hot_updated_per_min)} /min`,
      pct: Math.min(100, Math.round((f.indexed_written_per_min / base) * 100)),
      color: "bg-emerald-500",
    },
  ];
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

function formatBps(bps: number | null | undefined): string {
  if (bps === null || bps === undefined || Number.isNaN(bps)) return "0 B/s";
  return `${formatBytes(bps)}/s`;
}

function formatRate(val: number | null | undefined): string {
  if (val === null || val === undefined || Number.isNaN(val)) return "0";
  if (val >= 1000) return `${(val / 1000).toFixed(2)}k`;
  return val >= 10 ? val.toFixed(1) : val.toFixed(2);
}

function formatCount(val: number | null | undefined): string {
  if (val === null || val === undefined || Number.isNaN(val)) return "-";
  return Math.round(val).toLocaleString("zh-CN");
}

function formatTs(ts: number): string {
  if (!ts) return "-";
  const d = new Date(ts * 1000);
  return d.toLocaleTimeString("zh-CN", { hour12: false });
}

function formatChartTs(ts: number): string {
  const d = new Date(ts * 1000);
  if (selectedRange.value === "7d" || selectedRange.value === "30d") {
    return `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")} ${String(d.getHours()).padStart(2, "0")}:00`;
  }
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
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

function renderCharts() {
  const pts = historyPoints.value;
  const labels = pts.map((p) => formatChartTs(p.ts));

  if (throughputChartRef.value) {
    if (!throughputChart) {
      throughputChart = echarts.init(throughputChartRef.value);
    }
    throughputChart.setOption(
      {
        tooltip: { trigger: "axis" },
        legend: {
          top: 0,
          textStyle: { fontSize: 11, color: "#475569" },
          data: ["Hash/min", "Metadata/min", "Stream/min", "入库/min", "拦截/min"],
        },
        grid: { left: 48, right: 16, top: 36, bottom: 26 },
        xAxis: {
          type: "category",
          data: labels,
          axisLabel: { fontSize: 11, color: "#64748b" },
        },
        yAxis: {
          type: "value",
          splitLine: { lineStyle: { color: "#f1f5f9" } },
          axisLabel: { fontSize: 11, color: "#64748b" },
        },
        series: [
          {
            name: "Hash/min",
            type: "line",
            smooth: true,
            showSymbol: false,
            data: pts.map((p) => Number(p.hashes_per_min.toFixed(1))),
          },
          {
            name: "Metadata/min",
            type: "line",
            smooth: true,
            showSymbol: false,
            data: pts.map((p) => Number(p.metadata_succeeded_per_min.toFixed(1))),
          },
          {
            name: "Stream/min",
            type: "line",
            smooth: true,
            showSymbol: false,
            data: pts.map((p) => Number(p.stream_pushed_per_min.toFixed(1))),
          },
          {
            name: "入库/min",
            type: "line",
            smooth: true,
            showSymbol: false,
            itemStyle: { color: "#10b981" },
            data: pts.map((p) => Number(p.indexed_written_per_min.toFixed(1))),
          },
          {
            name: "拦截/min",
            type: "line",
            smooth: true,
            showSymbol: false,
            lineStyle: { type: "dashed" },
            itemStyle: { color: "#f59e0b" },
            data: pts.map((p) => Number(p.filtered_per_min.toFixed(1))),
          },
        ],
      },
      { notMerge: true },
    );
  }

  if (capacityChartRef.value) {
    if (!capacityChart) {
      capacityChart = echarts.init(capacityChartRef.value);
    }
    capacityChart.setOption(
      {
        tooltip: { trigger: "axis" },
        legend: {
          top: 0,
          textStyle: { fontSize: 11, color: "#475569" },
          data: ["DHT QPS", "Stream 积压(Lag)", "Stream 长度", "元数据速率(KB/s)"],
        },
        grid: { left: 48, right: 48, top: 36, bottom: 26 },
        xAxis: {
          type: "category",
          data: labels,
          axisLabel: { fontSize: 11, color: "#64748b" },
        },
        yAxis: [
          {
            type: "value",
            name: "QPS / 队列",
            nameTextStyle: { fontSize: 10, color: "#94a3b8" },
            splitLine: { lineStyle: { color: "#f1f5f9" } },
            axisLabel: { fontSize: 11, color: "#64748b" },
          },
          {
            type: "value",
            name: "KB/s",
            nameTextStyle: { fontSize: 10, color: "#94a3b8" },
            splitLine: { show: false },
            axisLabel: { fontSize: 11, color: "#64748b" },
          },
        ],
        series: [
          {
            name: "DHT QPS",
            type: "line",
            smooth: true,
            showSymbol: false,
            itemStyle: { color: "#6366f1" },
            data: pts.map((p) => Number(p.actual_qps.toFixed(1))),
          },
          {
            name: "Stream 积压(Lag)",
            type: "line",
            smooth: true,
            showSymbol: false,
            itemStyle: { color: "#ef4444" },
            data: pts.map((p) => p.stream_new_lag),
          },
          {
            name: "Stream 长度",
            type: "line",
            smooth: true,
            showSymbol: false,
            itemStyle: { color: "#06b6d4" },
            data: pts.map((p) => p.stream_new_len),
          },
          {
            name: "元数据速率(KB/s)",
            type: "line",
            yAxisIndex: 1,
            smooth: true,
            showSymbol: false,
            itemStyle: { color: "#8b5cf6" },
            data: pts.map((p) => Number((p.metadata_rx_bps / 1024).toFixed(1))),
          },
        ],
      },
      { notMerge: true },
    );
  }
}

async function loadSnapshot() {
  loadingSnapshot.value = true;
  try {
    snapshot.value = await getOverviewSnapshot();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "加载实时总览快照失败");
  } finally {
    loadingSnapshot.value = false;
  }
}

async function loadHistory() {
  loadingHistory.value = true;
  try {
    const resp = await getOverviewHistory(selectedRange.value);
    historyPoints.value = resp.points;
    historyGranularity.value = resp.granularity;
    await nextTick();
    renderCharts();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "加载历史趋势失败");
  } finally {
    loadingHistory.value = false;
  }
}

async function refreshAll() {
  await Promise.all([loadSnapshot(), loadHistory()]);
}

function connectSse() {
  if (eventSource) {
    eventSource.close();
    eventSource = null;
  }
  sseStatus.value = reconnectAttempt > 0 ? "reconnecting" : "connecting";

  const es = new EventSource("/api/v1/overview/stream", { withCredentials: true });
  eventSource = es;

  es.addEventListener("snapshot", (ev: MessageEvent<string>) => {
    try {
      const parsed = JSON.parse(ev.data) as OverviewSnapshot;
      snapshot.value = parsed;
      loadingSnapshot.value = false;
      sseStatus.value = "connected";
      reconnectAttempt = 0;
    } catch {
      // 忽略格式异常帧
    }
  });

  es.onerror = () => {
    sseStatus.value = "reconnecting";
    es.close();
    if (eventSource === es) {
      eventSource = null;
    }
    const delayMs = Math.min(15_000, 1500 * Math.pow(2, Math.min(reconnectAttempt, 4)));
    reconnectAttempt += 1;
    if (reconnectTimer !== null) {
      window.clearTimeout(reconnectTimer);
    }
    reconnectTimer = window.setTimeout(() => {
      connectSse();
    }, delayMs);
  };
}

function handleResize() {
  throughputChart?.resize();
  capacityChart?.resize();
}

watch(selectedRange, () => {
  void loadHistory();
});

onMounted(async () => {
  window.addEventListener("resize", handleResize);
  await Promise.all([loadSnapshot(), loadHistory()]);
  connectSse();
});

onUnmounted(() => {
  window.removeEventListener("resize", handleResize);
  if (reconnectTimer !== null) {
    window.clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  if (eventSource) {
    eventSource.close();
    eventSource = null;
  }
  throughputChart?.dispose();
  capacityChart?.dispose();
  throughputChart = null;
  capacityChart = null;
});
</script>

<template>
  <div class="max-w-7xl mx-auto space-y-5">
    <!-- 顶部健康状态条 -->
    <div class="rounded-xl border border-slate-200 bg-white px-5 py-3.5 shadow-sm flex flex-wrap items-center justify-between gap-3">
      <div class="flex flex-wrap items-center gap-3 text-xs">
        <div class="flex items-center gap-1.5 font-semibold text-slate-800 text-sm mr-1">
          <Activity class="h-4 w-4 text-indigo-600" />
          集群健康概览
        </div>

        <el-tag :type="snapshot?.health.panel_ok ? 'success' : 'info'" size="small" effect="light">
          Panel: {{ snapshot?.health.panel_ok ? "正常" : "加载中" }}
        </el-tag>
        <el-tag
          :type="snapshot?.health.redis_ok ? 'success' : 'danger'"
          size="small"
          effect="light"
        >
          Redis: {{ snapshot?.health.redis_ok ? "已连通" : "异常" }}
        </el-tag>
        <el-tag
          :type="snapshot?.health.manticore_ok ? 'success' : 'danger'"
          size="small"
          effect="light"
        >
          Manticore: {{ snapshot?.health.manticore_ok ? "已连通" : "异常" }}
        </el-tag>
        <el-tag type="info" size="small" effect="plain">
          Crawler: {{ snapshot?.health.running_crawlers ?? 0 }}/{{ snapshot?.health.total_crawlers ?? 0 }} 运行
        </el-tag>
        <el-tag type="info" size="small" effect="plain">
          Indexer: {{ snapshot?.health.running_indexers ?? 0 }}/{{ snapshot?.health.total_indexers ?? 0 }} 运行
        </el-tag>
        <el-tag
          v-if="(snapshot?.health.crashed_workers ?? 0) > 0"
          type="danger"
          size="small"
          effect="dark"
        >
          熔断节点: {{ snapshot?.health.crashed_workers }}
        </el-tag>
      </div>

      <div class="flex items-center gap-3">
        <span class="inline-flex items-center gap-1.5 text-xs text-slate-500">
          <span
            class="h-2 w-2 rounded-full"
            :class="
              sseStatus === 'connected'
                ? 'bg-emerald-500 animate-pulse'
                : sseStatus === 'connecting'
                  ? 'bg-amber-400'
                  : 'bg-rose-500'
            "
          />
          <Wifi class="h-3.5 w-3.5" />
          {{
            sseStatus === "connected"
              ? "SSE 实时推送中"
              : sseStatus === "connecting"
                ? "正在建立 SSE..."
                : "SSE 断线重连中"
          }}
        </span>
        <span v-if="snapshot?.captured_at" class="text-[11px] text-slate-400 font-mono">
          {{ formatTs(snapshot.captured_at) }}
        </span>
        <el-button size="small" :loading="loadingSnapshot || loadingHistory" @click="refreshAll">
          <RefreshCw class="h-3.5 w-3.5 mr-1" />
          刷新
        </el-button>
      </div>
    </div>

    <!-- 异常告警条 -->
    <div
      v-if="hasCriticalAlert"
      class="rounded-xl border border-rose-200 bg-rose-50 px-5 py-3.5 flex flex-wrap items-center justify-between gap-3"
    >
      <div class="flex items-start gap-2.5 text-xs text-rose-800">
        <AlertTriangle class="h-4 w-4 text-rose-600 shrink-0 mt-0.5" />
        <div class="space-y-1">
          <div class="font-semibold">检测到依赖服务或采集节点异常</div>
          <div class="flex flex-wrap gap-x-4 gap-y-1">
            <span v-if="snapshot && !snapshot.health.redis_ok">
              Redis: {{ snapshot.health.redis_message || "连接异常" }}
            </span>
            <span v-if="snapshot && !snapshot.health.manticore_ok">
              Manticore: {{ snapshot.health.manticore_message || "连接异常" }}
            </span>
            <span v-if="snapshot && snapshot.health.crashed_workers > 0">
              {{ snapshot.health.crashed_workers }} 个 Worker 处于 Crashed 状态，请查看 stderr 日志
            </span>
          </div>
        </div>
      </div>
      <div class="flex items-center gap-2">
        <el-button size="small" plain @click="router.push('/settings/services')">
          <ServerCog class="h-3.5 w-3.5 mr-1" />
          检查服务配置
        </el-button>
        <el-button size="small" type="danger" plain @click="router.push('/workers')">
          排查节点状态
        </el-button>
      </div>
    </div>

    <!-- KPI 核心指标卡 -->
    <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
      <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-2">
        <div class="flex items-center justify-between text-xs text-slate-500">
          <span>实时 DHT 探测 QPS</span>
          <Activity class="h-4 w-4 text-indigo-500" />
        </div>
        <div class="text-2xl font-bold font-mono text-slate-900">
          {{ snapshot ? snapshot.kpi.actual_qps.toFixed(1) : "-" }}
          <span class="text-xs font-normal text-slate-500">req/s</span>
        </div>
        <div class="text-[11px] text-slate-500 flex items-center justify-between">
          <span>当前系统软句柄限制 (nofile)</span>
          <span class="font-mono text-slate-700">
            {{ snapshot?.kpi.system_soft_nofile ?? "未受限 / N/A" }}
          </span>
        </div>
      </div>

      <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-2">
        <div class="flex items-center justify-between text-xs text-slate-500">
          <span>实时入库速度 / 推算日产出</span>
          <CheckCircle2 class="h-4 w-4 text-emerald-500" />
        </div>
        <div class="text-2xl font-bold font-mono text-slate-900">
          {{ snapshot ? formatRate(snapshot.kpi.indexed_written_per_min) : "-" }}
          <span class="text-xs font-normal text-slate-500">条/min</span>
        </div>
        <div class="text-[11px] text-slate-500 flex items-center justify-between">
          <span>今日已累计: <strong class="font-mono text-slate-800">{{ formatCount(snapshot?.kpi.today_indexed_docs) }}</strong></span>
          <span>日预估: <strong class="font-mono text-emerald-700">{{ formatCount(snapshot?.kpi.estimated_daily_docs) }}</strong></span>
        </div>
      </div>

      <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-2">
        <div class="flex items-center justify-between text-xs text-slate-500">
          <span>Manticore 存量 `torrents` 规模</span>
          <HardDrive class="h-4 w-4 text-sky-500" />
        </div>
        <div class="text-2xl font-bold font-mono text-slate-900">
          {{ formatCount(snapshot?.kpi.torrents_total_docs) }}
          <span class="text-xs font-normal text-slate-500">docs</span>
        </div>
        <div class="text-[11px] text-slate-500 flex items-center justify-between">
          <span>磁盘占用: <strong class="font-mono text-slate-800">{{ formatBytes(snapshot?.kpi.torrents_disk_bytes) }}</strong></span>
          <span>60s 状态缓存</span>
        </div>
      </div>

      <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-2">
        <div class="flex items-center justify-between text-xs text-slate-500">
          <span>Redis Streams 队列积压</span>
          <Database class="h-4 w-4 text-rose-500" />
        </div>
        <div class="text-2xl font-bold font-mono text-slate-900">
          {{ formatCount(snapshot?.kpi.stream_new_lag) }}
          <span class="text-xs font-normal text-slate-500">Lag</span>
        </div>
        <div class="text-[11px] text-slate-500 flex items-center justify-between font-mono">
          <span>new: {{ snapshot?.kpi.stream_new_len ?? 0 }}</span>
          <span>hot: {{ snapshot?.kpi.stream_hot_len ?? 0 }}</span>
          <span>pending: {{ snapshot?.kpi.stream_new_pending ?? 0 }}</span>
        </div>
      </div>

      <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-2">
        <div class="flex items-center justify-between text-xs text-slate-500">
          <span>网络 I/O 与元数据抓取带宽</span>
          <Wifi class="h-4 w-4 text-violet-500" />
        </div>
        <div class="text-lg font-bold font-mono text-slate-900">
          Meta {{ formatBps(snapshot?.kpi.metadata_rx_bps) }}
        </div>
        <div class="text-[11px] text-slate-500 flex items-center justify-between font-mono">
          <span>UDP RX: {{ formatBps(snapshot?.kpi.udp_rx_bps) }}</span>
          <span>UDP TX: {{ formatBps(snapshot?.kpi.udp_tx_bps) }}</span>
        </div>
      </div>

      <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-2">
        <div class="flex items-center justify-between text-xs text-slate-500">
          <span>Worker 总内存 (RSS) & Hot 热度</span>
          <Cpu class="h-4 w-4 text-amber-500" />
        </div>
        <div class="text-2xl font-bold font-mono text-slate-900">
          {{ formatBytes(snapshot?.kpi.total_rss_bytes ?? 0) }}
        </div>
        <div class="text-[11px] text-slate-500 flex items-center justify-between font-mono">
          <span>Hot 推送: {{ formatRate(snapshot?.funnel.hot_pushed_per_min) }}/m</span>
          <span>Hot 落库: {{ formatRate(snapshot?.funnel.hot_updated_per_min) }}/m</span>
        </div>
      </div>
    </div>

    <!-- 流水线实时漏斗 -->
    <div class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-4">
      <div class="flex items-center justify-between">
        <div>
          <h3 class="text-sm font-semibold text-slate-900">采集 → 过滤 → 入库 实时吞吐漏斗</h3>
          <p class="text-xs text-slate-500 mt-0.5">
            基于多节点秒级增量快照动态换算每分钟流量（节点重启自动重置基线）
          </p>
        </div>
      </div>

      <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        <div
          v-for="stage in funnelStages"
          :key="stage.key"
          class="rounded-lg border border-slate-100 bg-slate-50/70 p-3.5 space-y-2"
        >
          <div class="flex items-center justify-between text-xs">
            <span class="font-medium text-slate-700">{{ stage.label }}</span>
            <span class="font-mono font-semibold text-slate-900">
              {{ formatRate(stage.value) }} /min
            </span>
          </div>
          <div class="h-2 w-full rounded-full bg-slate-200 overflow-hidden">
            <div
              class="h-full rounded-full transition-all duration-300"
              :class="stage.color"
              :style="{ width: `${Math.max(stage.value > 0 ? 4 : 0, stage.pct)}%` }"
            />
          </div>
          <div class="flex items-center justify-between text-[11px] text-slate-500">
            <span>{{ stage.sub }}</span>
            <span class="font-mono">{{ stage.pct }}%</span>
          </div>
        </div>
      </div>
    </div>

    <!-- 历史趋势图表 -->
    <div class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h3 class="text-sm font-semibold text-slate-900">历史运行趋势</h3>
          <p class="text-xs text-slate-500 mt-0.5">
            当前聚合粒度：{{ historyGranularity === "minute" ? "分钟级 (metrics_minute)" : "小时级 (metrics_hour)" }}
          </p>
        </div>
        <el-radio-group v-model="selectedRange" size="small">
          <el-radio-button
            v-for="opt in rangeOptions"
            :key="opt.value"
            :value="opt.value"
          >
            {{ opt.label }}
          </el-radio-button>
        </el-radio-group>
      </div>

      <div class="grid grid-cols-1 lg:grid-cols-2 gap-5" v-loading="loadingHistory">
        <div class="rounded-lg border border-slate-100 p-3">
          <div class="text-xs font-medium text-slate-700 mb-2">采集漏斗与入库吞吐 (条/min)</div>
          <div ref="throughputChartRef" class="h-64 w-full" />
        </div>
        <div class="rounded-lg border border-slate-100 p-3">
          <div class="text-xs font-medium text-slate-700 mb-2">DHT QPS、Stream 积压与元数据带宽</div>
          <div ref="capacityChartRef" class="h-64 w-full" />
        </div>
      </div>
    </div>

    <!-- Worker 节点健康速览 -->
    <div class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-3">
      <div class="flex items-center justify-between">
        <div>
          <h3 class="text-sm font-semibold text-slate-900">Worker 进程健康状态</h3>
          <p class="text-xs text-slate-500 mt-0.5">共 {{ snapshot?.workers.length ?? 0 }} 个已配置节点</p>
        </div>
        <el-button size="small" type="primary" plain @click="router.push('/workers')">
          管理与参数调优
          <ArrowRight class="h-3.5 w-3.5 ml-1" />
        </el-button>
      </div>

      <el-table :data="snapshot?.workers ?? []" size="small" border stripe>
        <el-table-column prop="id" label="实例 ID" min-width="130">
          <template #default="{ row }">
            <span class="font-mono font-medium text-slate-900">{{ row.id }}</span>
          </template>
        </el-table-column>
        <el-table-column prop="display_name" label="显示名称" min-width="140" />
        <el-table-column prop="role" label="角色" width="100">
          <template #default="{ row }">
            <el-tag size="small" :type="row.role === 'crawler' ? 'primary' : 'success'" effect="plain">
              {{ row.role === "crawler" ? "Crawler" : "Indexer" }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column prop="runtime_state" label="运行状态" width="120">
          <template #default="{ row }">
            <el-tag size="small" :type="stateTagType(row.runtime_state)">
              {{ stateLabel(row.runtime_state) }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column prop="pid" label="PID" width="90">
          <template #default="{ row }">
            <span class="font-mono text-xs text-slate-600">{{ row.pid ?? "-" }}</span>
          </template>
        </el-table-column>
        <el-table-column prop="restart_count" label="重启次数" width="90" />
        <el-table-column label="内存 RSS" width="110">
          <template #default="{ row }">
            <span class="font-mono text-xs">{{ row.rss_bytes > 0 ? formatBytes(row.rss_bytes) : "-" }}</span>
          </template>
        </el-table-column>
        <el-table-column prop="last_error" label="最近错误" min-width="180" show-overflow-tooltip>
          <template #default="{ row }">
            <span v-if="row.last_error" class="text-xs text-rose-600">{{ row.last_error }}</span>
            <span v-else class="text-xs text-slate-400">-</span>
          </template>
        </el-table-column>
      </el-table>
    </div>
  </div>
</template>
