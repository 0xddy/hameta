<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { RouterLink } from "vue-router";
import { ElMessage } from "element-plus";
import {
  Pause,
  Play,
  Terminal,
  Table2,
} from "lucide-vue-next";
import {
  buildConsoleStreamUrl,
  getConsoleRecent,
  type ConsoleIngestEvent,
  type IngestHubStats,
} from "@/api/console";
import TorrentDetailDrawer from "@/components/TorrentDetailDrawer.vue";
import ObsSelect from "@/components/ui/ObsSelect.vue";
import { useTelemetryStore } from "@/stores/telemetry";
import {
  copyToClipboard,
  fmtBytes,
  fmtCount,
  fmtDurMs,
  fmtExact,
  fmtRate,
  formatTimeMs,
} from "@/utils/format";

const telemetry = useTelemetryStore();

const streamStatus = ref<"connecting" | "live" | "paused" | "reconnecting" | "disconnected">("connecting");
const userDisconnected = ref(false);
const eventTypeFilter = ref<"all" | "indexed" | "hot_updated" | "cleaned">("all");
const workerFilter = ref<string>("all");
const keywordFilter = ref("");
const maxBuffer = ref<number>(300);
const viewMode = ref<"table" | "terminal">("table");
const paused = ref(false);

const MAX_BUFFER_OPTIONS = [
  { label: "缓冲 100 条", value: 100 },
  { label: "缓冲 300 条", value: 300 },
  { label: "缓冲 500 条", value: 500 },
];

const TYPE_FILTER_PILLS: Array<{ key: "all" | "indexed" | "hot_updated" | "cleaned"; label: string }> = [
  { key: "all", label: "全部" },
  { key: "indexed", label: "新入库" },
  { key: "hot_updated", label: "热度更新" },
  { key: "cleaned", label: "已清洗标题" },
];

const events = ref<ConsoleIngestEvent[]>([]);
const pendingBuffer = ref<ConsoleIngestEvent[]>([]);
const stats = ref<IngestHubStats>({
  latest_seq: 0,
  ring_len: 0,
  ring_capacity: 256,
  subscribers: 0,
  total_published: 0,
});
const laggedSkipped = ref(0);
const newestSeq = ref<number | null>(null);
const selectedSeq = ref<number | null>(null);

const drawerVisible = ref(false);
const drawerTorrentId = ref<number | null>(null);
const drawerFallbackEvent = ref<ConsoleIngestEvent | null>(null);

// Sliding timestamps (ms) for real-time EPS rate calculation
const arrivalTimestamps = ref<number[]>([]);
const nowTick = ref(Date.now());

let es: EventSource | null = null;
let reconnectTimer: number | null = null;
let reconnectAttempt = 0;
let tickTimer: number | null = null;

const workerOptions = computed(() => {
  const ids = new Set<string>();
  for (const w of telemetry.snapshot?.workers ?? []) {
    if (w.role === "indexer") ids.add(w.id);
  }
  for (const ev of events.value) {
    if (ev.worker_id) ids.add(ev.worker_id);
  }
  const opts = [{ label: "全部 Indexer", value: "all" }];
  for (const id of Array.from(ids).sort()) {
    opts.push({ label: id, value: id });
  }
  return opts;
});

const serverEventTypeFilter = computed(() =>
  eventTypeFilter.value === "cleaned" ? "all" : eventTypeFilter.value,
);

const filteredEvents = computed(() => {
  const kw = keywordFilter.value.trim().toLowerCase();
  return events.value.filter((ev) => {
    if (eventTypeFilter.value === "cleaned" && !ev.cleaned) return false;
    if (
      (eventTypeFilter.value === "indexed" || eventTypeFilter.value === "hot_updated") &&
      ev.event_type !== eventTypeFilter.value
    ) {
      return false;
    }
    if (workerFilter.value !== "all" && ev.worker_id !== workerFilter.value) return false;
    if (kw) {
      const hay = `${ev.info_hash} ${ev.raw_name} ${ev.refine_name} ${ev.worker_id}`.toLowerCase();
      if (!hay.includes(kw)) return false;
    }
    return true;
  });
});

const selectedEvent = computed<ConsoleIngestEvent | null>(() => {
  if (selectedSeq.value !== null) {
    const found = events.value.find((e) => e.seq === selectedSeq.value);
    if (found) return found;
  }
  return filteredEvents.value[0] ?? events.value[0] ?? null;
});

const epsRate = computed(() => {
  const cutoff = nowTick.value - 5000;
  const recent = arrivalTimestamps.value.filter((t) => t >= cutoff);
  return recent.length / 5;
});

const indexedCount = computed(() => events.value.filter((e) => e.event_type === "indexed").length);
const hotUpdatedCount = computed(() => events.value.filter((e) => e.event_type === "hot_updated").length);
const cleanedCount = computed(() => events.value.filter((e) => e.cleaned).length);

const avgLatencyMs = computed(() => {
  if (events.value.length === 0) return 0;
  const sum = events.value.reduce((acc, e) => acc + (e.latency_ms || 0), 0);
  return Math.round(sum / events.value.length);
});

const p95LatencyMs = computed(() => {
  if (events.value.length === 0) return 0;
  const sorted = events.value.map((e) => e.latency_ms || 0).sort((a, b) => a - b);
  const idx = Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95));
  return sorted[idx];
});

const selectedSseFrame = computed(() => {
  const ev = selectedEvent.value;
  if (!ev) return "";
  const payload = {
    info_hash: ev.info_hash,
    event_type: ev.event_type,
    worker_id: ev.worker_id,
    manticore_id: ev.manticore_id,
    batch_id: ev.batch_id,
    raw_name: ev.raw_name,
    refine_name: ev.refine_name,
    cleaned: ev.cleaned,
    total_size: ev.total_size,
    file_count: ev.file_count,
    hot: ev.hot,
    hot_delta: ev.hot_delta,
    latency_ms: ev.latency_ms,
    updated_at: ev.updated_at,
  };
  return `id: ${ev.seq}\nevent: ingest\ndata: ${JSON.stringify(payload, null, 2)}`;
});

function closeStream() {
  if (reconnectTimer !== null) {
    window.clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  if (es) {
    es.close();
    es = null;
  }
}

function pushIncomingEvent(ev: ConsoleIngestEvent) {
  stats.value.latest_seq = Math.max(stats.value.latest_seq, ev.seq);
  stats.value.total_published += 1;
  arrivalTimestamps.value = [...arrivalTimestamps.value.slice(-199), Date.now()];

  if (paused.value) {
    pendingBuffer.value = [ev, ...pendingBuffer.value].slice(0, maxBuffer.value);
    return;
  }

  if (events.value.some((x) => x.seq === ev.seq)) return;
  events.value = [ev, ...events.value].slice(0, maxBuffer.value);
  newestSeq.value = ev.seq;
  if (selectedSeq.value === null) selectedSeq.value = ev.seq;
}

async function fetchInitialRecent() {
  try {
    const res = await getConsoleRecent({
      limit: Math.min(200, maxBuffer.value),
      event_type: serverEventTypeFilter.value,
      worker_id: workerFilter.value,
    });
    stats.value = res.stats;
    // API may return ascending or descending; keep newest first
    const sorted = [...res.events].sort((a, b) => b.seq - a.seq).slice(0, maxBuffer.value);
    events.value = sorted;
    if (sorted.length > 0 && selectedSeq.value === null) {
      selectedSeq.value = sorted[0].seq;
    }
  } catch {
    // stream will still connect
  }
}

function openStream(replay = 40) {
  closeStream();
  if (userDisconnected.value) {
    streamStatus.value = "disconnected";
    telemetry.consoleBadge = "OFF";
    return;
  }
  streamStatus.value = paused.value ? "paused" : reconnectAttempt > 0 ? "reconnecting" : "connecting";

  const url = buildConsoleStreamUrl({
    replay,
    event_type: serverEventTypeFilter.value,
    worker_id: workerFilter.value,
  });

  const conn = new EventSource(url, { withCredentials: true });
  es = conn;

  conn.addEventListener("ready", (msg: MessageEvent<string>) => {
    try {
      const payload = JSON.parse(msg.data) as {
        latest_seq: number;
        ring_len: number;
        ring_capacity: number;
      };
      stats.value.latest_seq = payload.latest_seq;
      stats.value.ring_len = payload.ring_len;
      stats.value.ring_capacity = payload.ring_capacity;
    } catch {
      // ignore
    }
    reconnectAttempt = 0;
    streamStatus.value = paused.value ? "paused" : "live";
    telemetry.consoleBadge = paused.value ? "PAUSE" : "SSE";
  });

  conn.addEventListener("ingest", (msg: MessageEvent<string>) => {
    try {
      const ev = JSON.parse(msg.data) as ConsoleIngestEvent;
      pushIncomingEvent(ev);
    } catch {
      // ignore malformed SSE frame
    }
  });

  conn.addEventListener("lagged", (msg: MessageEvent<string>) => {
    try {
      const payload = JSON.parse(msg.data) as { skipped?: number };
      laggedSkipped.value += payload.skipped ?? 0;
    } catch {
      laggedSkipped.value += 1;
    }
  });

  conn.onerror = () => {
    conn.close();
    if (es === conn) es = null;
    if (userDisconnected.value) {
      streamStatus.value = "disconnected";
      telemetry.consoleBadge = "OFF";
      return;
    }
    streamStatus.value = "reconnecting";
    telemetry.consoleBadge = "OFF";
    const delayMs = Math.min(12_000, 1500 * Math.pow(2, Math.min(reconnectAttempt, 3)));
    reconnectAttempt += 1;
    if (reconnectTimer !== null) window.clearTimeout(reconnectTimer);
    reconnectTimer = window.setTimeout(() => {
      openStream(0);
    }, delayMs);
  };
}

function toggleConnection() {
  if (userDisconnected.value) {
    userDisconnected.value = false;
    reconnectAttempt = 0;
    void fetchInitialRecent();
    openStream(40);
  } else {
    userDisconnected.value = true;
    closeStream();
    streamStatus.value = "disconnected";
    telemetry.consoleBadge = "OFF";
  }
}

function togglePause() {
  paused.value = !paused.value;
  if (!paused.value) {
    if (pendingBuffer.value.length > 0) {
      const seen = new Set(events.value.map((e) => e.seq));
      const fresh = pendingBuffer.value.filter((e) => !seen.has(e.seq));
      events.value = [...fresh, ...events.value].slice(0, maxBuffer.value);
      pendingBuffer.value = [];
    }
    streamStatus.value = userDisconnected.value ? "disconnected" : es ? "live" : "connecting";
    telemetry.consoleBadge = userDisconnected.value ? "OFF" : "SSE";
  } else {
    streamStatus.value = userDisconnected.value ? "disconnected" : "paused";
    telemetry.consoleBadge = userDisconnected.value ? "OFF" : "PAUSE";
  }
}

function clearFeed() {
  events.value = [];
  pendingBuffer.value = [];
  selectedSeq.value = null;
}

function resetFilters() {
  eventTypeFilter.value = "all";
  workerFilter.value = "all";
  keywordFilter.value = "";
}

async function manualReload() {
  if (userDisconnected.value) return;
  await fetchInitialRecent();
  openStream(20);
}

function exportNdjson() {
  if (filteredEvents.value.length === 0) {
    ElMessage.warning("当前视图没有可导出的事件");
    return;
  }
  const body = filteredEvents.value.map((e) => JSON.stringify(e)).join("\n") + "\n";
  const blob = new Blob([body], { type: "application/x-ndjson;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `hameta-console-${Date.now()}.ndjson`;
  a.click();
  URL.revokeObjectURL(url);
  ElMessage.success(`已导出 ${filteredEvents.value.length} 条 NDJSON 事件`);
}

function openTorrentDetail(ev: ConsoleIngestEvent) {
  drawerTorrentId.value = ev.manticore_id > 0 ? ev.manticore_id : null;
  drawerFallbackEvent.value = ev;
  drawerVisible.value = true;
}

async function handleCopy(text: string, label: string) {
  const ok = await copyToClipboard(text);
  if (ok) ElMessage.success(`已复制 ${label}`);
  else ElMessage.warning("剪贴板复制失败");
}

watch([serverEventTypeFilter, workerFilter], () => {
  void manualReload();
});

watch(maxBuffer, (next) => {
  events.value = events.value.slice(0, next);
  pendingBuffer.value = pendingBuffer.value.slice(0, next);
  if (events.value.length < next && !userDisconnected.value) {
    void fetchInitialRecent();
  }
});

onMounted(async () => {
  await fetchInitialRecent();
  openStream(50);
  tickTimer = window.setInterval(() => {
    nowTick.value = Date.now();
  }, 1000);
});

onUnmounted(() => {
  closeStream();
  telemetry.consoleBadge = "SSE";
  if (tickTimer !== null) {
    window.clearInterval(tickTimer);
    tickTimer = null;
  }
});
</script>

<template>
  <div class="space-y-4">
    <!-- Zone 1: Top SSE Status & Telemetry Cards -->
    <div class="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-4 gap-4">
      <div class="obs-card rounded-xl p-4 space-y-2 min-w-0">
        <div class="flex items-center justify-between gap-2 text-xs text-slate-400">
          <span>SSE 实时通道</span>
          <button
            type="button"
            class="obs-btn-ghost px-2 py-0.5 rounded text-[11px] cursor-pointer"
            @click="toggleConnection"
          >
            {{ userDisconnected ? "重连 SSE" : "断开 SSE" }}
          </button>
        </div>
        <div class="flex items-center gap-2.5">
          <span
            class="h-2 w-2 rounded-full shrink-0"
            :class="
              streamStatus === 'live'
                ? 'bg-emerald-400 text-emerald-400 pulse-ring'
                : streamStatus === 'paused' || streamStatus === 'reconnecting' || streamStatus === 'connecting'
                  ? 'bg-amber-400'
                  : 'bg-rose-500'
            "
          ></span>
          <span
            class="text-base font-semibold"
            :class="
              streamStatus === 'live'
                ? 'text-emerald-300'
                : streamStatus === 'paused' || streamStatus === 'reconnecting' || streamStatus === 'connecting'
                  ? 'text-amber-300'
                  : 'text-rose-300'
            "
          >
            {{
              streamStatus === "live"
                ? "已连接"
                : streamStatus === "paused"
                  ? "已暂停"
                  : streamStatus === "reconnecting"
                    ? "重连中"
                    : streamStatus === "connecting"
                      ? "连接中"
                      : "已断开"
            }}
          </span>
        </div>
        <div class="pt-1 border-t border-slate-800/80 flex items-center justify-between gap-2 font-mono-num text-[10px] text-slate-500">
          <span class="truncate">GET /api/v1/console/ingest-stream</span>
          <span class="shrink-0 text-slate-400">#{{ fmtExact(stats.latest_seq) }}</span>
        </div>
      </div>

      <div class="obs-card rounded-xl p-4 space-y-2 min-w-0">
        <div class="flex items-center justify-between gap-2 text-xs text-slate-400">
          <span>实时入库速率</span>
          <span class="font-mono-num text-[11px] text-cyan-300">{{ fmtRate(epsRate * 60) }}/min</span>
        </div>
        <div class="text-xl font-bold font-mono-num text-slate-100 truncate">
          {{ epsRate.toFixed(1) }}
          <span class="text-xs font-normal text-slate-400">ev/s</span>
        </div>
        <div class="pt-1 border-t border-slate-800/80 font-mono-num text-[11px] text-slate-500 truncate">
          新入库 {{ indexedCount }} · 热度 {{ hotUpdatedCount }} · 内核 {{ stats.ring_len }}/{{ stats.ring_capacity }}
        </div>
      </div>

      <div class="obs-card rounded-xl p-4 space-y-2 min-w-0">
        <div class="flex items-center justify-between gap-2 text-xs text-slate-400">
          <span>会话捕获与环形缓冲</span>
          <span class="font-mono-num text-[11px] text-slate-400">{{ events.length }} / {{ maxBuffer }}</span>
        </div>
        <div class="text-xl font-bold font-mono-num text-slate-100">
          {{ fmtCount(stats.total_published) }}
        </div>
        <div class="pt-1 space-y-1">
          <div class="h-1.5 w-full rounded-full bg-slate-800 overflow-hidden">
            <div
              class="h-full rounded-full bg-gradient-to-r from-emerald-500 to-cyan-400 transition-all duration-200"
              :style="{ width: `${Math.min(100, Math.max(4, Math.round((events.length / maxBuffer) * 100)))}%` }"
            ></div>
          </div>
        </div>
      </div>

      <div class="obs-card rounded-xl p-4 space-y-2 min-w-0">
        <div class="flex items-center justify-between gap-2 text-xs text-slate-400">
          <span>批写延迟 &amp; 清洗命中</span>
          <span class="font-mono-num text-[10px] text-slate-500">Manticore RT</span>
        </div>
        <div class="text-xl font-bold font-mono-num text-emerald-300">
          {{ fmtDurMs(avgLatencyMs) }}
        </div>
        <div class="pt-1 border-t border-slate-800/80 font-mono-num text-[11px] text-slate-400 truncate">
          P95 {{ fmtDurMs(p95LatencyMs) }} · 清洗命中 {{ cleanedCount }} 条<template v-if="laggedSkipped > 0"> · 跳过 {{ laggedSkipped }}</template>
        </div>
      </div>
    </div>

    <!-- Zone 2: Interactive Control & Filter Toolbar -->
    <div class="obs-card rounded-xl p-3.5 space-y-3">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="flex flex-wrap items-center gap-2">
          <button
            type="button"
            class="obs-btn-ghost px-3 py-1.5 rounded-lg text-xs cursor-pointer inline-flex items-center gap-1.5"
            :class="paused ? 'border-amber-500/40 text-amber-300' : ''"
            @click="togglePause"
          >
            <Play v-if="paused" class="w-3.5 h-3.5 text-emerald-400" />
            <Pause v-else class="w-3.5 h-3.5 text-amber-400" />
            <span>{{ paused ? "恢复实时" : "暂停刷屏" }}</span>
          </button>
          <span
            v-if="paused && pendingBuffer.length > 0"
            class="px-2 py-1 rounded-lg bg-amber-500/15 border border-amber-500/35 text-amber-300 font-mono-num text-[11px]"
          >
            +{{ pendingBuffer.length }} 积压
          </span>
          <button
            type="button"
            class="obs-btn-ghost px-3 py-1.5 rounded-lg text-xs cursor-pointer"
            @click="clearFeed"
          >
            清空画面
          </button>
          <button
            type="button"
            class="obs-btn-ghost px-3 py-1.5 rounded-lg text-xs cursor-pointer"
            @click="manualReload"
          >
            重连同步
          </button>
          <button
            type="button"
            class="obs-btn-ghost px-3 py-1.5 rounded-lg text-xs cursor-pointer"
            @click="exportNdjson"
          >
            导出 NDJSON
          </button>
        </div>

        <div class="inline-flex items-center p-0.5 rounded-lg bg-[#090c12] border border-slate-800 text-xs shrink-0">
          <button
            type="button"
            class="px-2.5 py-1 rounded-md cursor-pointer transition inline-flex items-center gap-1"
            :class="viewMode === 'table' ? 'bg-emerald-500/20 text-emerald-300 font-semibold' : 'text-slate-400 hover:text-slate-200'"
            @click="viewMode = 'table'"
          >
            <Table2 class="w-3.5 h-3.5" />
            表格流
          </button>
          <button
            type="button"
            class="px-2.5 py-1 rounded-md cursor-pointer transition inline-flex items-center gap-1"
            :class="viewMode === 'terminal' ? 'bg-emerald-500/20 text-emerald-300 font-semibold' : 'text-slate-400 hover:text-slate-200'"
            @click="viewMode = 'terminal'"
          >
            <Terminal class="w-3.5 h-3.5" />
            终端日志
          </button>
        </div>
      </div>

      <div class="flex flex-wrap items-center justify-between gap-2.5 pt-2.5 border-t border-slate-800/75">
        <div class="flex flex-wrap items-center gap-1.5">
          <button
            v-for="pill in TYPE_FILTER_PILLS"
            :key="pill.key"
            type="button"
            class="px-2.5 py-1 rounded-lg border text-xs cursor-pointer transition"
            :class="
              eventTypeFilter === pill.key
                ? 'obs-tab-active font-semibold'
                : 'border-slate-800 bg-[#0a0e16] text-slate-400 hover:text-slate-200'
            "
            @click="eventTypeFilter = pill.key"
          >
            {{ pill.label }}
          </button>
        </div>

        <div class="flex flex-wrap items-center gap-2 flex-1 justify-end min-w-[260px]">
          <ObsSelect
            v-model="workerFilter"
            :options="workerOptions"
            mono
            class="w-36"
            trigger-class="px-2.5! py-1.5! text-xs!"
          />
          <ObsSelect
            :model-value="maxBuffer"
            :options="MAX_BUFFER_OPTIONS"
            mono
            class="w-32"
            trigger-class="px-2.5! py-1.5! text-xs!"
            @update:model-value="maxBuffer = Number($event) || 300"
          />
          <input
            v-model="keywordFilter"
            type="text"
            placeholder="搜索 InfoHash 或标题..."
            class="obs-input w-56 max-w-full rounded-lg px-3 py-1.5 text-xs font-mono-num"
          />
        </div>
      </div>
    </div>

    <!-- Zone 3: Split Live Feed + Event Inspector -->
    <div class="grid grid-cols-1 xl:grid-cols-12 gap-4 items-start">
      <!-- Left 8 cols: Live Stream Viewport -->
      <section class="xl:col-span-8 obs-card rounded-xl overflow-hidden flex flex-col min-w-0">
        <div class="px-4 py-2.5 border-b border-slate-800/80 bg-[#0b0f17] flex flex-wrap items-center justify-between gap-2 text-xs">
          <div class="flex items-center gap-2">
            <span class="font-mono-num text-[10px] px-1.5 py-0.5 rounded bg-cyan-500/15 text-cyan-300 border border-cyan-500/25">
              text/event-stream
            </span>
          </div>
          <div class="flex flex-wrap items-center gap-3 text-[11px]">
            <span class="font-mono-num text-slate-400">
              显示 {{ filteredEvents.length }} / 缓冲 {{ events.length }} 条
            </span>
            <span class="hidden sm:inline-flex items-center gap-2 text-slate-500">
              <span class="inline-flex items-center gap-1"><span class="h-1.5 w-1.5 rounded-full bg-emerald-400"></span>NEW 新入库</span>
              <span class="inline-flex items-center gap-1"><span class="h-1.5 w-1.5 rounded-full bg-amber-400"></span>HOT 热度更新</span>
              <span class="inline-flex items-center gap-1"><span class="h-1.5 w-1.5 rounded-full bg-cyan-400"></span>已清洗</span>
            </span>
          </div>
        </div>

        <!-- Table mode -->
        <div v-if="viewMode === 'table'" class="h-[560px] overflow-auto obs-scroll">
          <table class="w-full min-w-[780px] table-fixed text-left text-xs border-collapse">
            <colgroup>
              <col class="w-[100px]" />
              <col class="w-[74px]" />
              <col class="w-[132px]" />
              <col />
              <col class="w-[108px]" />
              <col class="w-[90px]" />
              <col class="w-[116px]" />
            </colgroup>
            <thead class="sticky top-0 z-10 bg-[#0b0f17] border-b border-slate-800/80 text-[11px] font-medium text-slate-500">
              <tr>
                <th class="py-2.5 pl-4 pr-2 whitespace-nowrap">时间</th>
                <th class="py-2.5 px-2 whitespace-nowrap">类型</th>
                <th class="py-2.5 px-2 whitespace-nowrap">InfoHash</th>
                <th class="py-2.5 px-2 whitespace-nowrap">入库标题 (refine_name)</th>
                <th class="py-2.5 px-2 whitespace-nowrap">体积 / 文件</th>
                <th class="py-2.5 px-2 whitespace-nowrap">来源</th>
                <th class="py-2.5 pl-2 pr-4 text-right whitespace-nowrap">操作</th>
              </tr>
            </thead>
            <tbody class="divide-y divide-slate-800/60">
              <tr
                v-for="ev in filteredEvents"
                :key="ev.seq"
                class="cursor-pointer transition"
                :class="[
                  selectedEvent?.seq === ev.seq ? 'console-row-selected' : 'hover:bg-slate-800/35',
                  ev.seq === newestSeq ? 'console-row-new' : '',
                ]"
                @click="selectedSeq = ev.seq"
                @dblclick="openTorrentDetail(ev)"
              >
                <td
                  class="py-2.5 pl-4 pr-2 font-mono-num text-[11px] text-slate-400 whitespace-nowrap"
                  :title="`#${ev.seq}`"
                >
                  {{ formatTimeMs(ev.updated_at > 1e12 ? ev.updated_at : ev.updated_at * 1000) }}
                </td>
                <td class="py-2.5 px-2 whitespace-nowrap">
                  <span
                    class="px-1.5 py-0.5 rounded font-mono-num text-[10px] border whitespace-nowrap"
                    :class="
                      ev.event_type === 'indexed'
                        ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/25'
                        : 'bg-amber-500/15 text-amber-300 border-amber-500/25'
                    "
                  >
                    {{ ev.event_type === "indexed" ? "NEW" : `HOT +${ev.hot_delta}` }}
                  </span>
                </td>
                <td
                  class="py-2.5 px-2 font-mono-num text-[11px] text-cyan-300 whitespace-nowrap truncate"
                  :title="ev.info_hash"
                >
                  {{ ev.info_hash.slice(0, 10) }}…{{ ev.info_hash.slice(-4) }}
                </td>
                <td class="py-2.5 px-2 overflow-hidden">
                  <div class="flex items-center gap-1.5 min-w-0">
                    <span class="text-slate-100 font-medium truncate" :title="ev.raw_name">
                      {{ ev.refine_name || ev.raw_name }}
                    </span>
                    <span
                      v-if="ev.cleaned"
                      class="shrink-0 px-1.5 py-0.5 rounded bg-emerald-500/15 text-emerald-300 border border-emerald-500/25 text-[10px]"
                    >
                      已清洗
                    </span>
                  </div>
                  <div
                    v-if="ev.cleaned && ev.raw_name && ev.raw_name !== ev.refine_name"
                    class="text-[10px] text-slate-500 truncate mt-0.5"
                    :title="ev.raw_name"
                  >
                    原: {{ ev.raw_name }}
                  </div>
                </td>
                <td class="py-2.5 px-2 font-mono-num text-[11px] text-slate-200 whitespace-nowrap truncate">
                  <span>{{ fmtBytes(ev.total_size) }}</span>
                  <span class="text-slate-600 mx-1">·</span>
                  <span class="text-slate-400">{{ fmtCount(ev.file_count) }}</span>
                </td>
                <td class="py-2.5 px-2 font-mono-num text-[11px] whitespace-nowrap">
                  <div class="text-slate-300 truncate" :title="ev.worker_id">{{ ev.worker_id }}</div>
                  <div class="text-[10px] text-slate-500">{{ fmtDurMs(ev.latency_ms) }}</div>
                </td>
                <td class="py-2.5 pl-2 pr-4 text-right whitespace-nowrap" @click.stop>
                  <div class="inline-flex items-center gap-1">
                    <button
                      type="button"
                      class="obs-btn-ghost px-2 py-0.5 rounded text-[11px] cursor-pointer"
                      title="复制 40 位 InfoHash"
                      @click="handleCopy(ev.info_hash, 'InfoHash')"
                    >
                      Hash
                    </button>
                    <button
                      type="button"
                      class="obs-btn-ghost px-2 py-0.5 rounded text-[11px] text-emerald-300 cursor-pointer"
                      title="复制 Magnet 链接"
                      @click="handleCopy(`magnet:?xt=urn:btih:${ev.info_hash}`, 'Magnet')"
                    >
                      Magnet
                    </button>
                  </div>
                </td>
              </tr>

              <tr v-if="filteredEvents.length === 0">
                <td colspan="7" class="py-24 px-6 text-center text-xs">
                  <div class="max-w-md mx-auto flex flex-col items-center justify-center space-y-3">
                    <div class="h-10 w-10 rounded-xl bg-slate-900 border border-slate-800 flex items-center justify-center text-slate-500">
                      <Terminal class="w-5 h-5" />
                    </div>
                    <div class="text-slate-300 font-medium">
                      {{ events.length > 0 ? "当前筛选条件无匹配的入库事件" : "暂无入库 SSE 脉冲事件" }}
                    </div>
                    <p class="text-[11px] text-slate-500 leading-relaxed">
                      {{
                        events.length > 0
                          ? "请调整事件类型、Indexer 筛选条件或清空搜索关键字后查看"
                          : "当 Indexer 子进程批量写入或更新热度至 Manticore 时，毫秒级推送将自动出现在此面板"
                      }}
                    </p>
                    <div>
                      <button
                        v-if="events.length > 0"
                        type="button"
                        class="obs-btn-ghost px-3 py-1.5 rounded-lg text-xs cursor-pointer"
                        @click="resetFilters"
                      >
                        重置筛选条件
                      </button>
                      <button
                        v-else
                        type="button"
                        class="obs-btn-ghost px-3 py-1.5 rounded-lg text-xs cursor-pointer"
                        :disabled="userDisconnected"
                        @click="manualReload"
                      >
                        重连同步
                      </button>
                    </div>
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </div>

        <!-- Terminal mode -->
        <div v-else class="console-term-viewport h-[560px] overflow-y-auto obs-scroll font-mono-num text-[11px] leading-6 p-3 space-y-0.5">
          <div
            v-for="ev in filteredEvents"
            :key="ev.seq"
            class="px-2 py-0.5 rounded flex flex-wrap items-center gap-x-2 cursor-pointer transition"
            :class="[
              selectedEvent?.seq === ev.seq ? 'console-row-selected text-cyan-100' : 'hover:bg-slate-800/45 text-slate-300',
              ev.seq === newestSeq ? 'console-row-new' : '',
            ]"
            @click="selectedSeq = ev.seq"
            @dblclick="openTorrentDetail(ev)"
          >
            <span class="text-slate-500 shrink-0">
              {{ formatTimeMs(ev.updated_at > 1e12 ? ev.updated_at : ev.updated_at * 1000) }}
            </span>
            <span class="text-slate-600 shrink-0">#{{ ev.seq }}</span>
            <span
              class="shrink-0"
              :class="ev.event_type === 'indexed' ? 'text-emerald-400' : 'text-amber-300'"
            >
              [{{ ev.event_type === "indexed" ? "NEW" : `HOT +${ev.hot_delta}` }}]
            </span>
            <span class="text-cyan-400 shrink-0">{{ ev.worker_id }}</span>
            <span class="text-slate-300 shrink-0">
              hash=<span class="text-cyan-300">{{ ev.info_hash.slice(0, 12) }}…</span>
            </span>
            <span class="text-slate-400 shrink-0">size={{ fmtBytes(ev.total_size) }}</span>
            <span class="text-slate-400 shrink-0">files={{ fmtCount(ev.file_count) }}</span>
            <span class="text-slate-500 shrink-0">{{ fmtDurMs(ev.latency_ms) }}</span>
            <span class="text-slate-600 shrink-0">|</span>
            <span class="text-slate-100 font-sans truncate max-w-[440px]">{{ ev.refine_name || ev.raw_name }}</span>
            <span
              v-if="ev.cleaned"
              class="px-1 py-0 rounded bg-emerald-500/15 text-emerald-300 text-[10px] shrink-0"
            >
              已清洗
            </span>
          </div>

          <div v-if="filteredEvents.length === 0" class="py-24 text-center text-slate-500">
            Waiting for /api/v1/console/ingest-stream SSE frames...
          </div>
        </div>
      </section>

      <!-- Right 4 cols: Event Inspector -->
      <aside class="xl:col-span-4 obs-card rounded-xl overflow-hidden flex flex-col min-w-0">
        <div class="px-4 py-3 border-b border-slate-800/80 bg-[#0b0f17] flex flex-wrap items-center justify-between gap-2 text-xs">
          <div class="flex items-center gap-2">
            <span class="font-semibold text-slate-100">事件检视器</span>
            <span v-if="selectedEvent" class="font-mono-num text-[11px] text-slate-400">
              #{{ selectedEvent.seq }}
            </span>
          </div>
          <div v-if="selectedEvent" class="flex items-center gap-2">
            <span class="font-mono-num text-[11px] text-slate-500">
              {{ formatTimeMs(selectedEvent.updated_at > 1e12 ? selectedEvent.updated_at : selectedEvent.updated_at * 1000) }}
            </span>
            <span
              class="px-2 py-0.5 rounded font-mono-num text-[10px] border"
              :class="
                selectedEvent.event_type === 'hot_updated'
                  ? 'bg-amber-500/15 text-amber-300 border-amber-500/30'
                  : 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30'
              "
            >
              {{ selectedEvent.event_type === "hot_updated" ? `HOT +${selectedEvent.hot_delta}` : "NEW 新入库" }}
            </span>
          </div>
          <span v-else class="font-mono-num text-[11px] text-slate-500">IDLE</span>
        </div>

        <div
          v-if="selectedEvent"
          class="p-4 space-y-3.5 text-xs flex-1 overflow-y-auto obs-scroll max-h-[560px]"
        >
          <!-- InfoHash Box & Quick Actions -->
          <div class="obs-inset rounded-xl p-3 space-y-2.5">
            <div class="flex items-center justify-between text-[11px] text-slate-400">
              <span>InfoHash (40 hex)</span>
              <span class="font-mono-num text-slate-500">ID {{ selectedEvent.manticore_id }}</span>
            </div>
            <div class="font-mono-num text-[11px] text-cyan-300 break-all select-all bg-[#070a10] border border-slate-800/80 rounded-lg px-2.5 py-2">
              {{ selectedEvent.info_hash }}
            </div>
            <div class="grid grid-cols-2 sm:grid-cols-4 xl:grid-cols-2 gap-1.5 pt-0.5">
              <button
                type="button"
                class="obs-btn-ghost px-2.5 py-1.5 rounded-lg text-[11px] cursor-pointer"
                @click="handleCopy(selectedEvent.info_hash, 'InfoHash')"
              >
                复制 Hash
              </button>
              <button
                type="button"
                class="obs-btn-ghost px-2.5 py-1.5 rounded-lg text-[11px] text-emerald-300 cursor-pointer"
                @click="handleCopy(`magnet:?xt=urn:btih:${selectedEvent.info_hash}`, 'Magnet')"
              >
                复制 Magnet
              </button>
              <RouterLink
                :to="`/redis?hash=${encodeURIComponent(selectedEvent.info_hash)}`"
                class="obs-btn-ghost px-2.5 py-1.5 rounded-lg text-[11px] text-cyan-300 text-center cursor-pointer"
              >
                Redis 诊断
              </RouterLink>
              <RouterLink
                :to="`/filter?tab=playground&title=${encodeURIComponent(selectedEvent.raw_name)}`"
                class="obs-btn-ghost px-2.5 py-1.5 rounded-lg text-[11px] text-amber-300 text-center cursor-pointer"
              >
                清洗沙盒
              </RouterLink>
            </div>
          </div>

          <!-- Title Diff -->
          <div class="obs-inset rounded-xl p-3 space-y-2">
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-slate-400">标题对比</span>
              <span
                v-if="selectedEvent.cleaned"
                class="px-1.5 py-0.5 rounded bg-emerald-500/15 text-emerald-300 text-[10px]"
              >
                8 阶段清洗已生效
              </span>
              <span
                v-else
                class="px-1.5 py-0.5 rounded bg-slate-800 text-slate-400 text-[10px]"
              >
                原始直通
              </span>
            </div>
            <div>
              <div class="text-[10px] text-slate-500 font-mono-num">refine_name</div>
              <div class="text-emerald-300 font-medium break-all mt-0.5">
                {{ selectedEvent.refine_name || selectedEvent.raw_name }}
              </div>
            </div>
            <div class="pt-1.5 border-t border-slate-800/70">
              <div class="text-[10px] text-slate-500 font-mono-num">raw_name</div>
              <div
                class="break-all mt-0.5 text-[11px]"
                :class="selectedEvent.cleaned ? 'text-slate-400 line-through' : 'text-slate-300'"
              >
                {{ selectedEvent.raw_name }}
              </div>
            </div>
          </div>

          <!-- 2x2 Metrics Grid -->
          <div class="grid grid-cols-2 gap-2.5 font-mono-num text-[11px]">
            <div class="obs-inset rounded-xl p-2.5">
              <div class="text-[10px] text-slate-500 font-sans">总体积</div>
              <div class="text-slate-100 font-semibold mt-0.5">{{ fmtBytes(selectedEvent.total_size) }}</div>
            </div>
            <div class="obs-inset rounded-xl p-2.5">
              <div class="text-[10px] text-slate-500 font-sans">文件数</div>
              <div class="text-slate-100 font-semibold mt-0.5">{{ fmtCount(selectedEvent.file_count) }} 个文件</div>
            </div>
            <div class="obs-inset rounded-xl p-2.5">
              <div class="text-[10px] text-slate-500 font-sans">热度 hot</div>
              <div class="text-amber-300 font-semibold mt-0.5">
                {{ fmtExact(selectedEvent.hot) }}<template v-if="selectedEvent.hot_delta > 0"> (+{{ selectedEvent.hot_delta }})</template>
              </div>
            </div>
            <div class="obs-inset rounded-xl p-2.5">
              <div class="text-[10px] text-slate-500 font-sans">写入耗时 / 批次</div>
              <div
                class="text-cyan-300 font-semibold mt-0.5 truncate"
                :title="`${selectedEvent.worker_id} · batch #${selectedEvent.batch_id} · ${selectedEvent.latency_ms}ms`"
              >
                {{ selectedEvent.worker_id }} · #{{ selectedEvent.batch_id }} · {{ fmtDurMs(selectedEvent.latency_ms) }}
              </div>
            </div>
          </div>

          <!-- Files Preview -->
          <div class="obs-inset rounded-xl p-3 space-y-2">
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-slate-300 font-medium">
                包含文件预览 ({{ selectedEvent.files_preview.length }})
              </span>
              <button
                type="button"
                class="text-cyan-400 hover:underline cursor-pointer"
                @click="openTorrentDetail(selectedEvent)"
              >
                在抽屉中查看完整目录树
              </button>
            </div>
            <div v-if="selectedEvent.files_preview.length > 0" class="divide-y divide-slate-800/60 font-mono-num text-[11px]">
              <div
                v-for="(fp, idx) in selectedEvent.files_preview.slice(0, 5)"
                :key="idx"
                class="py-1.5 flex items-center justify-between gap-2"
              >
                <span class="text-slate-300 truncate" :title="fp.path">{{ fp.path }}</span>
                <span class="text-slate-500 shrink-0">{{ fmtBytes(fp.size) }}</span>
              </div>
            </div>
            <div v-else class="py-3 text-center text-[11px] text-slate-500 font-mono-num">
              无文件摘要（点击右上角在抽屉中查询完整记录）
            </div>
          </div>

          <!-- Raw SSE Frame -->
          <div class="space-y-1.5">
            <div class="flex items-center justify-between text-[11px] text-slate-400">
              <span>SSE 原始事件帧</span>
              <span class="font-mono-num text-[10px] text-slate-500">text/event-stream</span>
            </div>
            <pre class="rounded-xl bg-[#06080d] border border-slate-800/90 p-3 font-mono-num text-[10px] leading-relaxed text-cyan-200/95 overflow-x-auto obs-scroll">{{ selectedSseFrame }}</pre>
          </div>
        </div>

        <div
          v-else
          class="flex-1 p-6 flex flex-col items-center justify-center text-center space-y-2 text-xs text-slate-500 min-h-[420px]"
        >
          <div class="text-slate-400 font-medium">未选中任何入库事件</div>
          <p class="text-[11px] text-slate-500">
            点击左侧事件行查看详情与原始帧
          </p>
        </div>
      </aside>
    </div>

    <TorrentDetailDrawer
      v-model="drawerVisible"
      :torrent-id="drawerTorrentId"
      :fallback-event="drawerFallbackEvent"
    />
  </div>
</template>
