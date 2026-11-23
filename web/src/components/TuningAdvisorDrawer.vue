<script setup lang="ts">
import { computed, onUnmounted, reactive, ref, watch } from "vue";
import { ElMessage } from "element-plus";
import {
  Activity,
  AlertTriangle,
  ArrowDownRight,
  ArrowUpRight,
  CheckCircle2,
  Play,
  RefreshCw,
  SlidersHorizontal,
  Sparkles,
  Square,
  Wrench,
} from "lucide-vue-next";
import {
  applyWorkerTuning,
  cancelWorkerTuning,
  finishWorkerTuning,
  getWorkerTuningStatus,
  startWorkerTuning,
  type TuningAction,
  type TuningConfidenceLevel,
  type TuningGoal,
  type TuningRecommendation,
  type TuningSeverity,
  type TuningWorkerOverview,
} from "@/api/tuning";
import ObsDrawer from "@/components/ui/ObsDrawer.vue";
import ObsSelect from "@/components/ui/ObsSelect.vue";
import { fmtBytes, fmtCount, fmtDurMs, fmtDurSecs } from "@/utils/format";

const props = defineProps<{
  modelValue: boolean;
  workerId: string;
  workerName?: string;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: boolean): void;
  (e: "applied", version: number): void;
  (e: "session-changed"): void;
}>();

const open = computed({
  get: () => props.modelValue,
  set: (v: boolean) => emit("update:modelValue", v),
});

const loading = ref(false);
const actionBusy = ref(false);
const applying = ref(false);
const overview = ref<TuningWorkerOverview | null>(null);
const sourceMode = ref<"session" | "ring">("session");
const actionFilter = ref<"all" | "decrease" | "increase" | "system">("all");
const selectedIds = ref<Set<string>>(new Set());
const applyRestart = ref(true);

const form = reactive({
  duration_secs: 3600,
  up_mbps_text: "",
  down_mbps_text: "",
  goal: "balanced" as TuningGoal,
});

const DURATION_OPTIONS = [
  { label: "5 分钟（快速验证）", value: 300 },
  { label: "15 分钟（短周期）", value: 900 },
  { label: "30 分钟（中周期）", value: 1800 },
  { label: "60 分钟（推荐 · 完整 1 小时稳态）", value: 3600 },
];

const GOAL_OPTIONS: Array<{ label: string; value: TuningGoal }> = [
  { label: "稳态高产（防 95% 降速）", value: "balanced" },
  { label: "极致吞吐（充分挖潜）", value: "throughput" },
  { label: "低资源节能（控内存/PPS）", value: "low_resource" },
];

let pollTimer: number | null = null;

function parseMbps(raw: string): number | null {
  const t = raw.trim();
  if (!t) return null;
  const n = Number(t);
  return Number.isFinite(n) && n > 0 ? n : null;
}

const activeSession = computed(() => overview.value?.session ?? null);
const report = computed(() => overview.value?.report ?? null);

const applicableRecs = computed(() =>
  (report.value?.recommendations ?? []).filter((r) => r.apply_supported),
);

const filteredRecs = computed(() => {
  const list = report.value?.recommendations ?? [];
  if (actionFilter.value === "all") return list;
  return list.filter((r) => r.action === actionFilter.value);
});

const selectedApplicableCount = computed(() =>
  applicableRecs.value.filter((r) => selectedIds.value.has(r.id)).length,
);

function syncDefaultSelection(recs: TuningRecommendation[]) {
  const next = new Set<string>();
  for (const r of recs) {
    if (r.apply_supported && (r.severity === "critical" || r.severity === "warning" || r.severity === "opportunity")) {
      next.add(r.id);
    }
  }
  selectedIds.value = next;
}

async function fetchStatus(silent = false, autoSelect = false) {
  if (!props.workerId) return;
  if (!silent) loading.value = true;
  try {
    const useRingOverrides = sourceMode.value === "ring";
    const up = useRingOverrides ? parseMbps(form.up_mbps_text) : null;
    const down = useRingOverrides ? parseMbps(form.down_mbps_text) : null;
    const res = await getWorkerTuningStatus(props.workerId, {
      source: sourceMode.value,
      ...(useRingOverrides ? { up_mbps: up, down_mbps: down, goal: form.goal } : {}),
    });
    overview.value = res;
    if (!res.session && sourceMode.value === "session") {
      sourceMode.value = "ring";
    } else if (res.session && !silent) {
      form.goal = res.session.constraints.goal;
      form.up_mbps_text = res.session.constraints.up_mbps ? String(res.session.constraints.up_mbps) : "";
      form.down_mbps_text = res.session.constraints.down_mbps ? String(res.session.constraints.down_mbps) : "";
    }
    if (autoSelect && res.report) {
      syncDefaultSelection(res.report.recommendations);
    } else if (res.report) {
      const valid = new Set(res.report.recommendations.filter((r) => r.apply_supported).map((r) => r.id));
      const kept = new Set<string>();
      for (const id of selectedIds.value) {
        if (valid.has(id)) kept.add(id);
      }
      if (kept.size === 0 && selectedIds.value.size === 0) {
        syncDefaultSelection(res.report.recommendations);
      } else {
        selectedIds.value = kept;
      }
    }
  } catch (e) {
    if (!silent) ElMessage.error(e instanceof Error ? e.message : "读取智能调优数据失败");
  } finally {
    loading.value = false;
  }
}

async function handleStartSession() {
  if (!props.workerId) return;
  actionBusy.value = true;
  try {
    await startWorkerTuning(props.workerId, {
      duration_secs: form.duration_secs,
      sample_interval_secs: 5,
      up_mbps: parseMbps(form.up_mbps_text),
      down_mbps: parseMbps(form.down_mbps_text),
      goal: form.goal,
    });
    sourceMode.value = "session";
    ElMessage.success(`已启动内存采集任务（目标 ${Math.round(form.duration_secs / 60)} 分钟）`);
    emit("session-changed");
    await fetchStatus(false, true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "启动采集任务失败");
  } finally {
    actionBusy.value = false;
  }
}

async function handleFinishEarly() {
  if (!props.workerId) return;
  actionBusy.value = true;
  try {
    await finishWorkerTuning(props.workerId);
    ElMessage.success("已结束采集并定格最终优化报告");
    emit("session-changed");
    await fetchStatus(false, true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "结束任务失败");
  } finally {
    actionBusy.value = false;
  }
}

async function handleCancelSession() {
  if (!props.workerId) return;
  actionBusy.value = true;
  try {
    await cancelWorkerTuning(props.workerId);
    sourceMode.value = "ring";
    ElMessage.info("已取消并清理内存采集任务");
    emit("session-changed");
    await fetchStatus(false, true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "取消任务失败");
  } finally {
    actionBusy.value = false;
  }
}

function toggleRec(id: string) {
  const next = new Set(selectedIds.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  selectedIds.value = next;
}

function selectAllApplicable() {
  selectedIds.value = new Set(applicableRecs.value.map((r) => r.id));
}

function clearSelection() {
  selectedIds.value = new Set();
}

async function handleApplySelected() {
  if (!props.workerId || selectedApplicableCount.value === 0) return;
  applying.value = true;
  try {
    const ids = applicableRecs.value
      .filter((r) => selectedIds.value.has(r.id))
      .map((r) => r.id);
    const useRingOverrides = sourceMode.value === "ring";
    const res = await applyWorkerTuning(props.workerId, {
      recommendation_ids: ids,
      source: sourceMode.value,
      ...(useRingOverrides
        ? {
            up_mbps: parseMbps(form.up_mbps_text),
            down_mbps: parseMbps(form.down_mbps_text),
            goal: form.goal,
          }
        : {}),
      apply_restart: applyRestart.value,
    });
    if (res.restart_error) {
      ElMessage.warning(
        `已应用 ${res.applied.length} 项建议生成配置 v${res.version}，但平滑重启失败: ${res.restart_error}`,
      );
    } else if (res.restarted) {
      ElMessage.success(
        `已应用 ${res.applied.length} 项建议，保存为 v${res.version} 并完成平滑重启（可随时回滚）`,
      );
    } else {
      ElMessage.success(`已应用 ${res.applied.length} 项建议并保存为配置 v${res.version}（重启生效）`);
    }
    emit("applied", res.version);
    await fetchStatus(true, true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "应用建议失败");
  } finally {
    applying.value = false;
  }
}

function scoreColorClass(score: number): string {
  if (score >= 82) return "text-emerald-300 border-emerald-500/35 bg-emerald-500/12";
  if (score >= 60) return "text-amber-300 border-amber-500/35 bg-amber-500/12";
  return "text-rose-300 border-rose-500/35 bg-rose-500/12";
}

function confidenceMeta(c: TuningConfidenceLevel): { label: string; cls: string } {
  switch (c) {
    case "high":
      return { label: "高置信度", cls: "bg-emerald-500/15 text-emerald-300 border-emerald-500/30" };
    case "medium":
      return { label: "中置信度", cls: "bg-cyan-500/15 text-cyan-300 border-cyan-500/30" };
    case "low":
      return { label: "低置信度", cls: "bg-amber-500/15 text-amber-300 border-amber-500/30" };
    default:
      return { label: "样本不足", cls: "bg-slate-800 text-slate-400 border-slate-700" };
  }
}

function severityMeta(s: TuningSeverity): { label: string; cls: string } {
  switch (s) {
    case "critical":
      return { label: "严重瓶颈", cls: "bg-rose-500/18 text-rose-300 border-rose-500/35" };
    case "warning":
      return { label: "重点告警", cls: "bg-amber-500/18 text-amber-300 border-amber-500/35" };
    case "opportunity":
      return { label: "提效机会", cls: "bg-cyan-500/15 text-cyan-300 border-cyan-500/30" };
    default:
      return { label: "精细微调", cls: "bg-slate-800 text-slate-300 border-slate-700" };
  }
}

function actionMeta(a: TuningAction): { label: string; cls: string } {
  switch (a) {
    case "decrease":
      return { label: "建议调低", cls: "bg-rose-500/15 text-rose-300 border-rose-500/30" };
    case "increase":
      return { label: "建议调高", cls: "bg-emerald-500/15 text-emerald-300 border-emerald-500/30" };
    case "system":
      return { label: "系统调优", cls: "bg-indigo-500/18 text-indigo-300 border-indigo-500/35" };
    default:
      return { label: "参数调整", cls: "bg-sky-500/15 text-sky-300 border-sky-500/30" };
  }
}

function startPolling() {
  stopPolling();
  pollTimer = window.setInterval(() => {
    if (open.value && props.workerId) {
      void fetchStatus(true, false);
    }
  }, 5000);
}

function stopPolling() {
  if (pollTimer !== null) {
    window.clearInterval(pollTimer);
    pollTimer = null;
  }
}

watch(
  () => [open.value, props.workerId] as const,
  ([isOpen, id]) => {
    if (isOpen && id) {
      void fetchStatus(false, true);
      startPolling();
    } else {
      stopPolling();
    }
  },
  { immediate: true },
);

onUnmounted(() => {
  stopPolling();
});
</script>

<template>
  <ObsDrawer v-model="open" max-width="960px">
    <template #header>
      <div>
        <div class="flex flex-wrap items-center gap-2">
          <span class="px-2 py-0.5 rounded bg-cyan-500/15 border border-cyan-500/30 text-[11px] font-mono-num text-cyan-300 inline-flex items-center gap-1">
            <Sparkles class="w-3 h-3" />
            IN-MEMORY 1H OPTIMIZER
          </span>
          <span class="text-xs font-mono-num text-slate-400">{{ workerId }}</span>
        </div>
        <h3 class="mt-1 text-sm font-semibold text-slate-100">
          {{ workerName || workerId }} · 智能参数诊断与优化顾问
        </h3>
      </div>
    </template>

    <div class="space-y-4 text-xs">
      <!-- Data Source Switcher & Rolling Buffer Summary -->
      <div class="obs-inset rounded-xl p-3.5 space-y-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div class="inline-flex items-center gap-1 p-0.5 rounded-lg bg-[#080b12] border border-slate-800">
            <button
              type="button"
              class="px-2.5 py-1 rounded-md transition cursor-pointer"
              :class="sourceMode === 'session' ? 'bg-cyan-500/18 text-cyan-300 font-medium border border-cyan-500/30' : 'text-slate-400 hover:text-slate-200 border border-transparent'"
              :disabled="!activeSession"
              @click="sourceMode = 'session'; fetchStatus(false, true)"
            >
              专项采集任务
              <span v-if="activeSession?.state === 'collecting'" class="ml-1 inline-block w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse"></span>
            </button>
            <button
              type="button"
              class="px-2.5 py-1 rounded-md transition cursor-pointer"
              :class="sourceMode === 'ring' ? 'bg-emerald-500/18 text-emerald-300 font-medium border border-emerald-500/30' : 'text-slate-400 hover:text-slate-200 border border-transparent'"
              @click="sourceMode = 'ring'; fetchStatus(false, true)"
            >
              近 1 小时自动滚存
              <span v-if="overview" class="ml-1 font-mono-num text-[10px] opacity-80">
                ({{ fmtDurSecs(overview.rolling.window_secs) }})
              </span>
            </button>
          </div>

          <div v-if="overview" class="flex flex-wrap items-center gap-2 font-mono-num text-[11px] text-slate-400">
            <span>
              滚存缓冲 {{ overview.rolling.sample_count }}/{{ overview.rolling.max_samples }} 点 ·
              内存 {{ fmtBytes(overview.rolling.memory_bytes) }}
            </span>
            <button
              type="button"
              class="obs-btn-ghost px-2 py-1 rounded-md inline-flex items-center gap-1 cursor-pointer"
              :disabled="loading"
              @click="fetchStatus(false, false)"
            >
              <RefreshCw class="w-3 h-3" :class="loading ? 'animate-spin text-cyan-400' : ''" />
              刷新
            </button>
          </div>
        </div>

        <!-- Active Collecting Banner -->
        <div
          v-if="activeSession && activeSession.state === 'collecting'"
          class="rounded-lg bg-cyan-950/30 border border-cyan-500/30 p-3 space-y-2.5"
        >
          <div class="flex flex-wrap items-center justify-between gap-2">
            <div class="flex items-center gap-2">
              <Activity class="w-4 h-4 text-cyan-400 animate-pulse" />
              <span class="font-semibold text-cyan-200">
                正在内存中采集运行时指标…
              </span>
              <span class="font-mono-num text-cyan-300">
                {{ fmtDurSecs(activeSession.elapsed_secs) }} / {{ fmtDurSecs(activeSession.target_duration_secs) }}
                ({{ Math.round(activeSession.progress_pct) }}%)
              </span>
            </div>
            <div class="flex items-center gap-2">
              <button
                type="button"
                class="obs-btn-primary px-2.5 py-1 rounded-md text-[11px] inline-flex items-center gap-1 cursor-pointer"
                :disabled="actionBusy || activeSession.sample_count < 2"
                @click="handleFinishEarly"
              >
                <CheckCircle2 class="w-3 h-3" />
                提前结束并定格报告
              </button>
              <button
                type="button"
                class="obs-btn-ghost px-2.5 py-1 rounded-md text-[11px] text-rose-300 inline-flex items-center gap-1 cursor-pointer"
                :disabled="actionBusy"
                @click="handleCancelSession"
              >
                <Square class="w-3 h-3" />
                取消
              </button>
            </div>
          </div>
          <div class="h-1.5 w-full rounded-full bg-slate-900 overflow-hidden">
            <div
              class="h-full rounded-full bg-gradient-to-r from-cyan-500 to-emerald-400 transition-all duration-500"
              :style="{ width: `${Math.max(2, activeSession.progress_pct)}%` }"
            ></div>
          </div>
          <div class="flex flex-wrap items-center justify-between gap-2 text-[11px] text-slate-400 font-mono-num">
            <span>
              已采 {{ activeSession.sample_count }} 样本（每 {{ activeSession.sample_interval_secs }}s 采样）·
              内存占用 {{ fmtBytes(activeSession.memory_bytes) }}
            </span>
            <span class="font-sans text-slate-400">
              下方实时预览当前阶段诊断；跑满 45–60 分钟进入高置信度稳态结论
            </span>
          </div>
        </div>

        <!-- Completed Session Banner -->
        <div
          v-else-if="activeSession && activeSession.state === 'completed'"
          class="rounded-lg bg-emerald-950/25 border border-emerald-500/30 px-3 py-2.5 flex flex-wrap items-center justify-between gap-2"
        >
          <div class="flex items-center gap-2 text-emerald-200">
            <CheckCircle2 class="w-4 h-4 text-emerald-400 shrink-0" />
            <span>
              已完成专项采集（{{ fmtDurSecs(activeSession.elapsed_secs) }} · {{ activeSession.sample_count }} 样本 · 内存 {{ fmtBytes(activeSession.memory_bytes) }}）
            </span>
          </div>
          <button
            type="button"
            class="text-[11px] text-slate-400 hover:text-rose-300 cursor-pointer"
            :disabled="actionBusy"
            @click="handleCancelSession"
          >
            清除本次任务
          </button>
        </div>

        <!-- Start New Collection / Analysis Constraints Controls -->
        <div
          v-if="!activeSession || activeSession.state !== 'collecting'"
          class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-5 gap-2.5 pt-1"
        >
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">采集时长</label>
            <ObsSelect
              :model-value="form.duration_secs"
              :options="DURATION_OPTIONS"
              class="w-full"
              @update:model-value="(v) => (form.duration_secs = Number(v))"
            />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">优化目标</label>
            <ObsSelect
              :model-value="form.goal"
              :options="GOAL_OPTIONS"
              class="w-full"
              @update:model-value="(v) => { form.goal = String(v) as TuningGoal; void fetchStatus(true, true); }"
            />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">服务器上行上限 (Mbps · 可选)</label>
            <input
              v-model="form.up_mbps_text"
              type="number"
              min="1"
              max="100000"
              placeholder="如 500（留空不限）"
              class="obs-input px-2.5 py-1.5 text-xs font-mono-num"
              @change="fetchStatus(true, true)"
            />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">服务器下行上限 (Mbps · 可选)</label>
            <input
              v-model="form.down_mbps_text"
              type="number"
              min="1"
              max="100000"
              placeholder="如 500（留空不限）"
              class="obs-input px-2.5 py-1.5 text-xs font-mono-num"
              @change="fetchStatus(true, true)"
            />
          </div>
          <div class="flex items-end">
            <button
              type="button"
              class="obs-btn-primary w-full px-3 py-1.5 rounded-lg text-xs inline-flex items-center justify-center gap-1.5 cursor-pointer"
              :disabled="actionBusy"
              @click="handleStartSession"
            >
              <Play class="w-3.5 h-3.5" />
              {{ activeSession ? "开始新一轮采集" : "启动采集任务" }}
            </button>
          </div>
        </div>
      </div>

      <!-- Empty / Waiting State -->
      <div
        v-if="!report"
        class="obs-card rounded-xl p-10 text-center space-y-2"
      >
        <SlidersHorizontal class="w-8 h-8 text-slate-600 mx-auto" />
        <div class="text-sm font-medium text-slate-200">正在等待采样数据积累…</div>
        <div class="text-xs text-slate-500">
          后台每 5 秒自动采集一次运行指标到内存（最多保留 1 小时，零磁盘写入）。保持实例运行 10 秒以上即可生成初步诊断。
        </div>
      </div>

      <!-- Diagnostic Report -->
      <template v-else>
        <!-- Regime & Health Score Header -->
        <div class="obs-card rounded-xl p-4 space-y-3">
          <div class="flex flex-wrap items-start justify-between gap-3">
            <div class="flex items-start gap-3.5 min-w-0">
              <div
                class="h-14 w-14 rounded-xl border flex flex-col items-center justify-center shrink-0 font-mono-num"
                :class="scoreColorClass(report.health_score)"
              >
                <span class="text-xl font-bold leading-none">{{ report.health_score }}</span>
                <span class="text-[10px] opacity-80 mt-0.5">健康分</span>
              </div>
              <div class="min-w-0 space-y-1">
                <div class="flex flex-wrap items-center gap-2">
                  <h4 class="text-sm font-semibold text-slate-100">{{ report.regime_title }}</h4>
                  <span
                    class="px-2 py-0.5 rounded border text-[10px] font-mono-num"
                    :class="confidenceMeta(report.confidence).cls"
                  >
                    {{ confidenceMeta(report.confidence).label }}
                  </span>
                  <span class="px-1.5 py-0.5 rounded bg-slate-800 text-slate-300 text-[10px] font-mono-num">
                    cfg v{{ report.config_version }}
                  </span>
                  <span
                    v-if="report.summary.restart_detected"
                    class="px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-300 text-[10px]"
                  >
                    期间发生重启/改配，已自动取最近连续段
                  </span>
                </div>
                <p class="text-xs text-slate-300 leading-relaxed">{{ report.regime_summary }}</p>
                <p class="text-[11px] text-slate-500">{{ report.confidence_note }}</p>
              </div>
            </div>

            <div class="flex flex-wrap items-center gap-1.5 font-mono-num text-[11px] shrink-0">
              <span class="px-2 py-1 rounded-lg bg-rose-500/12 border border-rose-500/25 text-rose-300">
                ↓ 调低 {{ report.decrease_count }}
              </span>
              <span class="px-2 py-1 rounded-lg bg-emerald-500/12 border border-emerald-500/25 text-emerald-300">
                ↑ 调高 {{ report.increase_count }}
              </span>
              <span
                v-if="report.system_count > 0"
                class="px-2 py-1 rounded-lg bg-indigo-500/12 border border-indigo-500/25 text-indigo-300"
              >
                ⚙ 系统 {{ report.system_count }}
              </span>
            </div>
          </div>

          <!-- 6-Cell Observed Telemetry Summary -->
          <div class="grid grid-cols-2 lg:grid-cols-3 gap-2.5 pt-2 border-t border-slate-800/80 font-mono-num text-[11px]">
            <div class="obs-inset rounded-lg px-3 py-2">
              <div class="text-slate-500 font-sans">实测预估线速带宽 / PPS</div>
              <div class="text-slate-100 font-semibold mt-0.5">
                TX {{ report.summary.est_wire_tx_mbps }} / RX {{ report.summary.est_wire_rx_mbps }} Mbps
              </div>
              <div class="text-slate-400">
                UDP {{ fmtCount(report.summary.udp_total_pps) }} pps · 回包限流 {{ report.summary.response_rate_limited_pct }}%
              </div>
            </div>

            <div class="obs-inset rounded-lg px-3 py-2">
              <div class="text-slate-500 font-sans">find_node 速率与反压降速</div>
              <div class="text-slate-100 font-semibold mt-0.5">
                均 {{ fmtCount(report.summary.actual_find_node_qps) }}/s · 配置 {{ fmtCount(report.summary.configured_find_node_qps) }}/s
              </div>
              <div :class="report.summary.throttled_time_pct >= 20 ? 'text-amber-300' : 'text-slate-400'">
                降速占比 {{ report.summary.throttled_time_pct }}% (严重 {{ report.summary.severe_throttled_time_pct }}%)
              </div>
            </div>

            <div class="obs-inset rounded-lg px-3 py-2">
              <div class="text-slate-500 font-sans">BEP-9 队列水位与等待</div>
              <div class="text-slate-100 font-semibold mt-0.5">
                P95 {{ report.summary.meta_queue_fill_p95_pct }}% · 等待 P95 {{ fmtDurMs(report.summary.meta_queue_wait_p95_ms) }}
              </div>
              <div :class="report.summary.meta_queue_evict_pct >= 5 ? 'text-rose-300' : 'text-slate-400'">
                驱逐 {{ report.summary.meta_queue_evict_pct }}% · 陈旧 {{ report.summary.meta_queue_stale_pct }}%
              </div>
            </div>

            <div class="obs-inset rounded-lg px-3 py-2">
              <div class="text-slate-500 font-sans">BEP-9 并发利用率与产率</div>
              <div class="text-slate-100 font-semibold mt-0.5">
                P95 {{ report.summary.worker_util_p95_pct }}% · {{ report.summary.torrents_per_min }} 种子/分
              </div>
              <div class="text-slate-400">
                成功率 {{ report.summary.job_success_rate_pct }}% · 耗时 P95 {{ fmtDurMs(report.summary.fetch_duration_p95_ms) }}
              </div>
            </div>

            <div class="obs-inset rounded-lg px-3 py-2">
              <div class="text-slate-500 font-sans">超时阶段与 Lookup/采样限流</div>
              <div class="text-slate-100 font-semibold mt-0.5">
                建连 {{ report.summary.timeout_tcp_connect_share_pct }}% · 分片 {{ report.summary.timeout_piece_recv_share_pct }}%
              </div>
              <div class="text-slate-400">
                Lookup 限流 {{ report.summary.peer_lookup_rate_limited_pct }}% · 采样限流 {{ report.summary.discovery_sample_hash_limited_pct }}%
              </div>
            </div>

            <div class="obs-inset rounded-lg px-3 py-2">
              <div class="text-slate-500 font-sans">内存趋势与生效并发 (FD)</div>
              <div class="text-slate-100 font-semibold mt-0.5">
                RSS P95 {{ report.summary.rss_p95_mb }} MB ({{ report.summary.rss_growth_mb_per_hour >= 0 ? "+" : "" }}{{ report.summary.rss_growth_mb_per_hour }}/h)
              </div>
              <div :class="report.summary.fd_clamped ? 'text-rose-300' : 'text-slate-400'">
                生效并发 {{ report.summary.effective_workers }}/{{ report.summary.configured_workers }} · 本地缓存 {{ report.summary.local_cache_fill_pct }}%
              </div>
            </div>
          </div>
        </div>

        <!-- Recommendations Toolbar -->
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div class="inline-flex items-center gap-1 p-0.5 rounded-lg bg-[#080b12] border border-slate-800">
            <button
              type="button"
              class="px-2.5 py-1 rounded-md cursor-pointer transition"
              :class="actionFilter === 'all' ? 'bg-slate-800 text-slate-100 font-medium' : 'text-slate-400 hover:text-slate-200'"
              @click="actionFilter = 'all'"
            >
              全部 ({{ report.recommendations.length }})
            </button>
            <button
              type="button"
              class="px-2.5 py-1 rounded-md cursor-pointer transition"
              :class="actionFilter === 'decrease' ? 'bg-rose-500/18 text-rose-300 font-medium' : 'text-slate-400 hover:text-slate-200'"
              @click="actionFilter = 'decrease'"
            >
              ↓ 建议调低 ({{ report.decrease_count }})
            </button>
            <button
              type="button"
              class="px-2.5 py-1 rounded-md cursor-pointer transition"
              :class="actionFilter === 'increase' ? 'bg-emerald-500/18 text-emerald-300 font-medium' : 'text-slate-400 hover:text-slate-200'"
              @click="actionFilter = 'increase'"
            >
              ↑ 建议调高 ({{ report.increase_count }})
            </button>
            <button
              type="button"
              class="px-2.5 py-1 rounded-md cursor-pointer transition"
              :class="actionFilter === 'system' ? 'bg-indigo-500/18 text-indigo-300 font-medium' : 'text-slate-400 hover:text-slate-200'"
              @click="actionFilter = 'system'"
            >
              ⚙ 系统建议 ({{ report.system_count }})
            </button>
          </div>

          <div v-if="applicableRecs.length > 0" class="flex items-center gap-3 text-[11px]">
            <button type="button" class="text-cyan-400 hover:underline cursor-pointer" @click="selectAllApplicable">
              全选可应用 ({{ applicableRecs.length }})
            </button>
            <button type="button" class="text-slate-400 hover:text-slate-200 cursor-pointer" @click="clearSelection">
              清空
            </button>
          </div>
        </div>

        <!-- Recommendations List -->
        <div v-if="filteredRecs.length === 0" class="obs-inset rounded-xl p-8 text-center space-y-1.5">
          <CheckCircle2 class="w-7 h-7 text-emerald-400 mx-auto" />
          <div class="text-sm font-medium text-emerald-200">
            {{ report.recommendations.length === 0 ? "当前参数与运行负载高度匹配，暂无需要调整的项" : "当前筛选分类下没有建议项" }}
          </div>
        </div>

        <div v-else class="space-y-2.5">
          <div
            v-for="rec in filteredRecs"
            :key="rec.id"
            class="obs-inset rounded-xl p-3.5 transition border"
            :class="
              rec.apply_supported && selectedIds.has(rec.id)
                ? 'border-cyan-500/45 bg-cyan-950/15'
                : 'border-slate-800/80'
            "
          >
            <div class="flex items-start gap-3">
              <div class="pt-0.5 shrink-0">
                <input
                  v-if="rec.apply_supported"
                  type="checkbox"
                  class="obs-checkbox cursor-pointer"
                  :checked="selectedIds.has(rec.id)"
                  @change="toggleRec(rec.id)"
                />
                <Wrench v-else class="w-4 h-4 text-indigo-400" />
              </div>

              <div class="flex-1 min-w-0 space-y-2">
                <div class="flex flex-wrap items-center justify-between gap-2">
                  <div class="flex flex-wrap items-center gap-1.5">
                    <span
                      class="px-1.5 py-0.5 rounded border text-[10px] font-medium inline-flex items-center gap-0.5"
                      :class="actionMeta(rec.action).cls"
                    >
                      <ArrowDownRight v-if="rec.action === 'decrease'" class="w-3 h-3" />
                      <ArrowUpRight v-else-if="rec.action === 'increase'" class="w-3 h-3" />
                      <AlertTriangle v-else class="w-3 h-3" />
                      {{ actionMeta(rec.action).label }}
                    </span>
                    <span
                      class="px-1.5 py-0.5 rounded border text-[10px]"
                      :class="severityMeta(rec.severity).cls"
                    >
                      {{ severityMeta(rec.severity).label }}
                    </span>
                    <span class="font-semibold text-slate-100">{{ rec.param_label }}</span>
                    <code class="px-1.5 py-0.5 rounded bg-slate-900 text-slate-400 font-mono-num text-[11px]">
                      {{ rec.param_path }}
                    </code>
                  </div>

                  <div class="flex items-center gap-1.5 font-mono-num text-xs shrink-0">
                    <span class="text-slate-400">{{ rec.current_display }}</span>
                    <span class="text-slate-600">→</span>
                    <span
                      class="font-semibold"
                      :class="
                        rec.action === 'decrease'
                          ? 'text-rose-300'
                          : rec.action === 'increase'
                            ? 'text-emerald-300'
                            : 'text-indigo-300'
                      "
                    >
                      {{ rec.recommended_display }}
                    </span>
                    <span
                      v-if="rec.delta_pct !== null"
                      class="text-[10px] px-1 py-0.5 rounded bg-slate-900"
                      :class="rec.delta_pct < 0 ? 'text-rose-300' : 'text-emerald-300'"
                    >
                      {{ rec.delta_pct > 0 ? "+" : "" }}{{ rec.delta_pct }}%
                    </span>
                  </div>
                </div>

                <div class="text-[11px] text-slate-300 leading-relaxed">
                  <span class="text-slate-500">实测依据：</span>{{ rec.evidence }}
                </div>
                <div class="text-[11px] text-slate-400 leading-relaxed">
                  <span class="text-cyan-400/90">优化原理：</span>{{ rec.reason }}
                </div>
              </div>
            </div>
          </div>
        </div>
      </template>
    </div>

    <template #footer>
      <div class="flex flex-wrap items-center justify-between gap-3 w-full text-xs">
        <div class="flex flex-wrap items-center gap-3">
          <span class="font-mono-num text-slate-400">
            已选 <strong class="text-cyan-300">{{ selectedApplicableCount }}</strong> / {{ applicableRecs.length }} 项可自动应用
          </span>
          <label class="inline-flex items-center gap-1.5 text-slate-300 cursor-pointer select-none">
            <input v-model="applyRestart" type="checkbox" class="obs-checkbox" />
            <span>保存新版本后自动平滑重启</span>
          </label>
        </div>

        <div class="flex items-center gap-2">
          <button
            type="button"
            class="obs-btn-ghost px-3 py-1.5 rounded-lg cursor-pointer"
            @click="open = false"
          >
            关闭
          </button>
          <button
            type="button"
            class="obs-btn-primary px-4 py-1.5 rounded-lg inline-flex items-center gap-1.5 cursor-pointer"
            :disabled="applying || selectedApplicableCount === 0"
            @click="handleApplySelected"
          >
            <Sparkles class="w-3.5 h-3.5" />
            {{ applying ? "应用中…" : `一键应用所选建议 (${selectedApplicableCount})` }}
          </button>
        </div>
      </div>
    </template>
  </ObsDrawer>
</template>
