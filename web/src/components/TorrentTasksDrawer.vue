<script setup lang="ts">
import { computed, onUnmounted, reactive, ref, watch } from "vue";
import { RouterLink } from "vue-router";
import { ElMessage } from "element-plus";
import {
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  HardDrive,
  Pause,
  Play,
  RefreshCw,
  ShieldAlert,
  Sparkles,
  Square,
  Trash2,
} from "lucide-vue-next";
import { ApiError } from "@/api/client";
import {
  cancelTorrentTask,
  createTorrentTask,
  deleteTorrentTask,
  listTorrentTasks,
  pauseTorrentTask,
  resumeTorrentTask,
  type GarbledCheckMode,
  type RecleanScope,
  type TorrentTaskItem,
  type TorrentTaskKind,
  type TorrentTasksOverview,
} from "@/api/torrents";
import ObsDrawer from "@/components/ui/ObsDrawer.vue";
import ObsSelect from "@/components/ui/ObsSelect.vue";
import { fmtCount, fmtExact, formatDateTime } from "@/utils/format";

const props = defineProps<{
  modelValue: boolean;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: boolean): void;
  (e: "changed", overview: TorrentTasksOverview): void;
}>();

const open = computed({
  get: () => props.modelValue,
  set: (v: boolean) => emit("update:modelValue", v),
});

const loading = ref(false);
const submitting = ref(false);
const actionBusyId = ref<number | null>(null);
const overview = ref<TorrentTasksOverview | null>(null);
const selectedKind = ref<TorrentTaskKind>("sync_total_size");
const expandedDiffIds = ref<Set<number>>(new Set());

const form = reactive({
  batch_size: 500,
  batch_delay_ms: 100,
  max_docs_text: "",
  dry_run: false,
  only_missing: true,
  clean_scope: "all" as RecleanScope,
  skip_empty_result: true,
  garbled_mode: "all" as GarbledCheckMode,
  evict_redis_indexed: true,
});

const BATCH_SIZE_OPTIONS = [
  { label: "200 条 / 批（最温和）", value: 200 },
  { label: "500 条 / 批（推荐）", value: 500 },
  { label: "1000 条 / 批（高速）", value: 1000 },
  { label: "2000 条 / 批（极速）", value: 2000 },
];

const BATCH_DELAY_OPTIONS = [
  { label: "0 ms（全速）", value: 0 },
  { label: "50 ms（高速）", value: 50 },
  { label: "100 ms（推荐）", value: 100 },
  { label: "300 ms（低负载）", value: 300 },
  { label: "1000 ms（极低负载）", value: 1000 },
];

const CLEAN_SCOPE_OPTIONS: Array<{ label: string; value: RecleanScope }> = [
  { label: "全量按最新规则重新清洗", value: "all" },
  { label: "仅清洗空标题或未清洗记录", value: "empty_or_unchanged" },
];

const GARBLED_MODE_OPTIONS: Array<{ label: string; value: GarbledCheckMode }> = [
  { label: "全面检测（U+FFFD 替换符 + 控制符 + Mojibake / GBK 错解）", value: "all" },
  { label: "保守模式（仅含 U+FFFD 替换符 / 非法控制符 / 私有区）", value: "strict_replacement" },
];

let pollTimer: number | null = null;

const runningTask = computed(() =>
  (overview.value?.tasks ?? []).find((t) => t.status === "running") ?? null,
);

const canStart = computed(() => {
  if (submitting.value || runningTask.value !== null) return false;
  if (selectedKind.value === "sync_total_size" && overview.value && !overview.value.has_total_size_column) {
    return false;
  }
  return true;
});

function stopPoll() {
  if (pollTimer !== null) {
    window.clearInterval(pollTimer);
    pollTimer = null;
  }
}

function ensurePoll() {
  stopPoll();
  if (!props.modelValue) return;
  pollTimer = window.setInterval(() => {
    if (props.modelValue) {
      void fetchOverview(true);
    }
  }, 2000);
}

async function fetchOverview(silent = false) {
  if (!silent) loading.value = true;
  try {
    const res = await listTorrentTasks(30);
    overview.value = res;
    emit("changed", res);
  } catch (e) {
    if (!silent) {
      ElMessage.error(e instanceof Error ? e.message : "加载异步维护任务失败");
    }
  } finally {
    if (!silent) loading.value = false;
  }
}

function parseMaxDocs(): number | null {
  const s = form.max_docs_text.trim();
  if (!s) return null;
  const n = Number(s);
  if (!Number.isFinite(n) || n <= 0) return null;
  return Math.floor(n);
}

async function startTask() {
  if (!canStart.value) return;
  submitting.value = true;
  try {
    const task = await createTorrentTask({
      kind: selectedKind.value,
      batch_size: form.batch_size,
      batch_delay_ms: form.batch_delay_ms,
      max_docs: parseMaxDocs(),
      dry_run: form.dry_run,
      only_missing: form.only_missing,
      clean_scope: form.clean_scope,
      skip_empty_result: form.skip_empty_result,
      garbled_mode: form.garbled_mode,
      evict_redis_indexed: form.evict_redis_indexed,
    });
    ElMessage.success(
      `已启动任务 #${task.id}（${taskKindLabel(task.kind)}${task.params.dry_run ? " · 预演模式" : ""}）`,
    );
    await fetchOverview(true);
  } catch (e) {
    ElMessage.error(e instanceof ApiError ? e.message : e instanceof Error ? e.message : "启动任务失败");
  } finally {
    submitting.value = false;
  }
}

async function handlePause(task: TorrentTaskItem) {
  actionBusyId.value = task.id;
  try {
    await pauseTorrentTask(task.id);
    ElMessage.info(`已暂停任务 #${task.id}，进度游标已保存`);
    await fetchOverview(true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "暂停失败");
  } finally {
    actionBusyId.value = null;
  }
}

async function handleResume(task: TorrentTaskItem) {
  actionBusyId.value = task.id;
  try {
    await resumeTorrentTask(task.id);
    ElMessage.success(`已从断点恢复任务 #${task.id}`);
    await fetchOverview(true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "恢复失败");
  } finally {
    actionBusyId.value = null;
  }
}

async function handleCancel(task: TorrentTaskItem) {
  actionBusyId.value = task.id;
  try {
    await cancelTorrentTask(task.id);
    ElMessage.warning(`已取消任务 #${task.id}`);
    await fetchOverview(true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "取消失败");
  } finally {
    actionBusyId.value = null;
  }
}

async function handleDelete(task: TorrentTaskItem) {
  actionBusyId.value = task.id;
  try {
    await deleteTorrentTask(task.id);
    ElMessage.success(`已删除任务记录 #${task.id}`);
    await fetchOverview(true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "删除失败");
  } finally {
    actionBusyId.value = null;
  }
}

function toggleDiffExpand(id: number) {
  const next = new Set(expandedDiffIds.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  expandedDiffIds.value = next;
}

function taskKindLabel(kind: TorrentTaskKind): string {
  if (kind === "sync_total_size") return "同步文件大小";
  if (kind === "clean_garbled") return "清洗乱码";
  return "清洗标题";
}

function taskKindBadgeClass(kind: TorrentTaskKind): string {
  if (kind === "sync_total_size") return "bg-cyan-500/15 text-cyan-300 border-cyan-500/30";
  if (kind === "clean_garbled") return "bg-rose-500/15 text-rose-300 border-rose-500/30";
  return "bg-violet-500/15 text-violet-300 border-violet-500/30";
}

function taskStatusMeta(status: TorrentTaskItem["status"]): { label: string; cls: string } {
  switch (status) {
    case "running":
      return { label: "运行中", cls: "bg-emerald-500/15 text-emerald-300 border-emerald-500/35" };
    case "paused":
      return { label: "已暂停", cls: "bg-amber-500/15 text-amber-300 border-amber-500/35" };
    case "completed":
      return { label: "已完成", cls: "bg-cyan-500/15 text-cyan-300 border-cyan-500/35" };
    case "cancelled":
      return { label: "已取消", cls: "bg-slate-700/40 text-slate-400 border-slate-700" };
    case "failed":
      return { label: "失败", cls: "bg-rose-500/15 text-rose-300 border-rose-500/35" };
  }
}

function fmtEta(secs: number | null): string {
  if (secs === null || secs <= 0) return "--";
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  if (m < 60) return `${m}m ${s}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

watch(
  () => props.modelValue,
  (visible) => {
    if (visible) {
      void fetchOverview(false);
      ensurePoll();
    } else {
      stopPoll();
    }
  },
);

onUnmounted(() => {
  stopPoll();
});
</script>

<template>
  <ObsDrawer
    v-model="open"
    title="磁力库异步批量任务"
    subtitle="基于 63 位主键游标分批扫表，支持千万级存量断点暂停 / 恢复、预演抽样与流控限速"
    max-width="880px"
  >
    <div class="space-y-5 text-xs">
      <!-- 新建任务区 -->
      <section class="obs-inset rounded-xl p-4 space-y-4">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div class="inline-flex items-center p-0.5 rounded-lg bg-[#070a10] border border-slate-800">
            <button
              type="button"
              class="px-3 py-1.5 rounded-md text-xs transition inline-flex items-center gap-1.5 cursor-pointer"
              :class="
                selectedKind === 'sync_total_size'
                  ? 'bg-cyan-500/20 text-cyan-200 font-semibold'
                  : 'text-slate-400 hover:text-slate-200'
              "
              @click="selectedKind = 'sync_total_size'"
            >
              <HardDrive class="w-3.5 h-3.5" />
              同步文件大小
            </button>
            <button
              type="button"
              class="px-3 py-1.5 rounded-md text-xs transition inline-flex items-center gap-1.5 cursor-pointer"
              :class="
                selectedKind === 'reclean_titles'
                  ? 'bg-emerald-500/20 text-emerald-200 font-semibold'
                  : 'text-slate-400 hover:text-slate-200'
              "
              @click="selectedKind = 'reclean_titles'"
            >
              <Sparkles class="w-3.5 h-3.5" />
              清洗标题
            </button>
            <button
              type="button"
              class="px-3 py-1.5 rounded-md text-xs transition inline-flex items-center gap-1.5 cursor-pointer"
              :class="
                selectedKind === 'clean_garbled'
                  ? 'bg-rose-500/20 text-rose-200 font-semibold'
                  : 'text-slate-400 hover:text-slate-200'
              "
              @click="selectedKind = 'clean_garbled'"
            >
              <ShieldAlert class="w-3.5 h-3.5" />
              清洗乱码
            </button>
          </div>

          <div class="flex items-center gap-2 font-mono-num text-[11px]">
            <span
              v-if="selectedKind === 'sync_total_size' && overview"
              class="px-2 py-0.5 rounded border"
              :class="
                overview.has_total_size_column
                  ? 'bg-emerald-500/12 text-emerald-300 border-emerald-500/30'
                  : 'bg-amber-500/12 text-amber-300 border-amber-500/30'
              "
            >
              {{ overview.has_total_size_column ? "total_size 列已就绪" : "尚未添加 total_size 列" }}
            </span>
            <RouterLink
              v-if="selectedKind === 'sync_total_size' && overview && !overview.has_total_size_column"
              to="/sql"
              class="text-cyan-300 hover:underline"
              @click="open = false"
            >
              去 SQL 终端执行 ALTER →
            </RouterLink>
            <span
              v-if="selectedKind === 'reclean_titles' && overview"
              class="px-2 py-0.5 rounded bg-emerald-500/12 text-emerald-300 border border-emerald-500/30"
            >
              生效规则 v{{ overview.published_rule_version ?? 1 }}
            </span>
            <RouterLink
              v-if="selectedKind === 'reclean_titles'"
              to="/filter"
              class="text-cyan-300 hover:underline"
              @click="open = false"
            >
              编辑清洗规则 →
            </RouterLink>
          </div>
        </div>

        <p class="text-[11px] text-slate-400 leading-relaxed">
          <template v-if="selectedKind === 'sync_total_size'">
            分批扫描 Manticore <code class="text-cyan-300">torrents</code> 表，从 <code class="text-cyan-300">file_info</code> 精确累加总字节数并回写 <code class="text-cyan-300">total_size</code>（保持原 <code class="text-slate-300">created_at / updated_at</code> 不变，不打乱入库排序）。
          </template>
          <template v-else-if="selectedKind === 'reclean_titles'">
            使用当前已发布的 11 阶段智能标题清洗规则，分批对存量种子重新清洗 <code class="text-cyan-300">raw_name → refine_name</code>，仅回写有变化的记录（同时保留原始时间戳，顺带补全空 <code class="text-cyan-300">total_size</code>）。
          </template>
          <template v-else>
            轻量扫描存量 <code class="text-cyan-300">raw_name / refine_name</code>（跳过大体积 <code class="text-slate-300">file_info</code> 以提速），批量删除历史编码损坏/乱码记录；勾选释放 Redis 缓存后，新版爬虫在下次遇见该 Hash 时会自动按 GBK/Big5/Shift_JIS 正确解码重抓。
          </template>
        </p>

        <!-- 参数表单 -->
        <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
          <div>
            <label class="text-[11px] text-slate-400 block mb-1">每批扫描条数</label>
            <ObsSelect
              :model-value="form.batch_size"
              :options="BATCH_SIZE_OPTIONS"
              mono
              trigger-class="px-2.5! py-1.5! text-xs!"
              @update:model-value="form.batch_size = Number($event) || 500"
            />
          </div>
          <div>
            <label class="text-[11px] text-slate-400 block mb-1">批次流控间隔</label>
            <ObsSelect
              :model-value="form.batch_delay_ms"
              :options="BATCH_DELAY_OPTIONS"
              mono
              trigger-class="px-2.5! py-1.5! text-xs!"
              @update:model-value="form.batch_delay_ms = Number($event) || 0"
            />
          </div>
          <div>
            <label class="text-[11px] text-slate-400 block mb-1">最大扫描条数（留空全表）</label>
            <input
              v-model="form.max_docs_text"
              type="text"
              inputmode="numeric"
              placeholder="不限（全量扫完）"
              class="obs-input w-full rounded-lg px-3 py-1.5 text-xs font-mono-num"
            />
          </div>
        </div>

        <div v-if="selectedKind === 'sync_total_size'" class="flex flex-wrap items-center gap-x-6 gap-y-2 text-[11px] text-slate-300">
          <label class="inline-flex items-center gap-1.5 cursor-pointer select-none">
            <input v-model="form.only_missing" type="checkbox" class="obs-checkbox" />
            <span>仅扫描 <code class="text-cyan-300">total_size = 0</code> 的旧记录（推荐，跳过已同步数据）</span>
          </label>
          <label class="inline-flex items-center gap-1.5 cursor-pointer select-none">
            <input v-model="form.dry_run" type="checkbox" class="obs-checkbox" />
            <span class="text-amber-300">预演模式 (Dry Run · 只统计与抽样对比，不写库)</span>
          </label>
        </div>

        <div v-else-if="selectedKind === 'reclean_titles'" class="space-y-2.5">
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3 items-end">
            <div>
              <label class="text-[11px] text-slate-400 block mb-1">标题清洗范围</label>
              <ObsSelect
                v-model="form.clean_scope"
                :options="CLEAN_SCOPE_OPTIONS"
                trigger-class="px-2.5! py-1.5! text-xs!"
              />
            </div>
            <div class="flex flex-wrap items-center gap-x-5 gap-y-2 text-[11px] text-slate-300 pb-1">
              <label class="inline-flex items-center gap-1.5 cursor-pointer select-none">
                <input v-model="form.skip_empty_result" type="checkbox" class="obs-checkbox" />
                <span>洗空时保留原标题</span>
              </label>
              <label class="inline-flex items-center gap-1.5 cursor-pointer select-none">
                <input v-model="form.dry_run" type="checkbox" class="obs-checkbox" />
                <span class="text-amber-300">预演模式 (Dry Run)</span>
              </label>
            </div>
          </div>
        </div>

        <div v-else class="space-y-2.5">
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3 items-end">
            <div>
              <label class="text-[11px] text-slate-400 block mb-1">乱码判定严格度</label>
              <ObsSelect
                v-model="form.garbled_mode"
                :options="GARBLED_MODE_OPTIONS"
                trigger-class="px-2.5! py-1.5! text-xs!"
              />
            </div>
            <div class="flex flex-wrap items-center gap-x-5 gap-y-2 text-[11px] text-slate-300 pb-1">
              <label class="inline-flex items-center gap-1.5 cursor-pointer select-none">
                <input v-model="form.evict_redis_indexed" type="checkbox" class="obs-checkbox" />
                <span>释放 Redis 已索引缓存（允许重抓正常名）</span>
              </label>
              <label class="inline-flex items-center gap-1.5 cursor-pointer select-none">
                <input v-model="form.dry_run" type="checkbox" class="obs-checkbox" />
                <span class="text-amber-300">预演模式 (Dry Run · 只抽样不删库)</span>
              </label>
            </div>
          </div>
        </div>

        <div class="pt-2 border-t border-slate-800/70 flex flex-wrap items-center justify-between gap-2">
          <span v-if="runningTask" class="text-[11px] text-amber-300 inline-flex items-center gap-1.5">
            <AlertTriangle class="w-3.5 h-3.5 shrink-0" />
            当前有运行中任务 #{{ runningTask.id }}，为避免并发整行写库互相覆盖，请先暂停或等待其完成
          </span>
          <span v-else class="text-[11px] text-slate-500">
            任务运行在后台异步协程，关闭弹窗或刷新页面均不会中断；支持随时暂停并从精确游标恢复。
          </span>
          <button
            type="button"
            class="obs-btn obs-btn-primary px-4! py-1.5! text-xs! inline-flex items-center gap-1.5 cursor-pointer"
            :disabled="!canStart"
            @click="startTask"
          >
            <Play class="w-3.5 h-3.5" />
            {{
              submitting
                ? "启动中..."
                : `启动「${taskKindLabel(selectedKind)}」${form.dry_run ? "预演" : "任务"}`
            }}
          </button>
        </div>
      </section>

      <!-- 任务队列与历史记录 -->
      <section class="space-y-3">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-2">
            <h3 class="text-xs font-semibold text-slate-200">异步任务管理与历史记录</h3>
            <span class="font-mono-num text-[11px] text-slate-500">
              共 {{ overview?.tasks.length ?? 0 }} 条
            </span>
          </div>
          <button
            type="button"
            class="obs-btn-ghost px-2.5 py-1 rounded-lg text-[11px] inline-flex items-center gap-1.5 cursor-pointer"
            :disabled="loading"
            @click="fetchOverview(false)"
          >
            <RefreshCw class="w-3 h-3" :class="{ 'animate-spin text-emerald-400': loading }" />
            刷新状态
          </button>
        </div>

        <div
          v-if="!overview || overview.tasks.length === 0"
          class="obs-inset rounded-xl py-12 text-center text-slate-500 text-xs"
        >
          暂无异步维护任务记录，可在上方按需启动「同步文件大小」、「清洗标题」或「清洗乱码」任务
        </div>

        <div v-else class="space-y-3">
          <div
            v-for="task in overview.tasks"
            :key="task.id"
            class="obs-inset rounded-xl p-3.5 space-y-2.5 border"
            :class="task.status === 'running' ? 'border-emerald-500/35' : 'border-slate-800/80'"
          >
            <!-- 顶行：类型 + 状态 + 操作按钮 -->
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="flex flex-wrap items-center gap-1.5 font-mono-num">
                <span class="text-slate-400 font-semibold">#{{ task.id }}</span>
                <span
                  class="px-2 py-0.5 rounded text-[10px] border"
                  :class="taskKindBadgeClass(task.kind)"
                >
                  {{ taskKindLabel(task.kind) }}
                </span>
                <span
                  class="px-2 py-0.5 rounded text-[10px] border inline-flex items-center gap-1"
                  :class="taskStatusMeta(task.status).cls"
                >
                  <span v-if="task.status === 'running'" class="h-1.5 w-1.5 rounded-full bg-emerald-400 pulse-ring" />
                  {{ taskStatusMeta(task.status).label }}
                </span>
                <span
                  v-if="task.params.dry_run"
                  class="px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-300 border border-amber-500/30 text-[10px]"
                >
                  DRY RUN
                </span>
                <span
                  v-if="task.kind === 'reclean_titles' && task.params.rule_version"
                  class="text-[10px] text-slate-500"
                >
                  规则 v{{ task.params.rule_version }}
                </span>
                <span class="text-[10px] text-slate-500">
                  批={{ task.params.batch_size }} · 间隔={{ task.params.batch_delay_ms }}ms
                </span>
              </div>

              <div class="flex flex-wrap items-center gap-1.5">
                <button
                  v-if="task.status === 'running'"
                  type="button"
                  class="obs-btn-ghost px-2.5 py-1 rounded text-[11px] text-amber-300 inline-flex items-center gap-1 cursor-pointer"
                  :disabled="actionBusyId === task.id"
                  @click="handlePause(task)"
                >
                  <Pause class="w-3 h-3" />
                  暂停
                </button>
                <button
                  v-if="task.status === 'paused' || task.status === 'failed'"
                  type="button"
                  class="obs-btn-ghost px-2.5 py-1 rounded text-[11px] text-emerald-300 inline-flex items-center gap-1 cursor-pointer"
                  :disabled="actionBusyId === task.id || runningTask !== null"
                  @click="handleResume(task)"
                >
                  <Play class="w-3 h-3" />
                  恢复继续
                </button>
                <button
                  v-if="task.status === 'running' || task.status === 'paused'"
                  type="button"
                  class="obs-btn-ghost px-2.5 py-1 rounded text-[11px] text-rose-300 inline-flex items-center gap-1 cursor-pointer"
                  :disabled="actionBusyId === task.id"
                  @click="handleCancel(task)"
                >
                  <Square class="w-3 h-3" />
                  取消
                </button>
                <button
                  v-if="task.status !== 'running'"
                  type="button"
                  class="obs-btn-ghost px-2 py-1 rounded text-[11px] text-slate-400 hover:text-rose-300 inline-flex items-center gap-1 cursor-pointer"
                  :disabled="actionBusyId === task.id"
                  title="删除任务历史记录"
                  @click="handleDelete(task)"
                >
                  <Trash2 class="w-3 h-3" />
                </button>
              </div>
            </div>

            <!-- 进度条 -->
            <div class="space-y-1">
              <div class="flex items-center justify-between text-[11px] font-mono-num">
                <span class="text-slate-300">
                  已扫描 <strong class="text-slate-100">{{ fmtExact(task.scanned_count) }}</strong>
                  <template v-if="task.estimated_total > 0"> / {{ fmtExact(task.estimated_total) }}</template>
                  · {{ task.kind === "clean_garbled" ? (task.params.dry_run ? "预演命中" : "已删除") : "已更新" }}
                  <strong :class="task.kind === 'clean_garbled' ? 'text-rose-300' : 'text-emerald-300'">{{ fmtExact(task.updated_count) }}</strong>
                  · 跳过 {{ fmtCount(task.skipped_count) }}
                </span>
                <span class="text-cyan-300 font-semibold">
                  {{ task.progress_percent.toFixed(1) }}%
                </span>
              </div>
              <div class="h-1.5 w-full rounded-full bg-slate-800/90 overflow-hidden">
                <div
                  class="h-full rounded-full transition-all duration-300"
                  :class="
                    task.status === 'failed'
                      ? 'bg-rose-500'
                      : task.status === 'paused'
                        ? 'bg-amber-400'
                        : 'bg-gradient-to-r from-emerald-500 to-cyan-400'
                  "
                  :style="{ width: `${Math.min(100, Math.max(task.status === 'running' ? 2 : 0, task.progress_percent))}%` }"
                />
              </div>
            </div>

            <!-- 细粒度遥测信息 -->
            <div class="flex flex-wrap items-center justify-between gap-2 text-[10px] font-mono-num text-slate-500">
              <span>
                速率 {{ task.rate_per_sec.toFixed(1) }} 条/s · 批次 {{ task.batches_done }}
                <template v-if="task.eta_secs !== null"> · 预计剩余 {{ fmtEta(task.eta_secs) }}</template>
                · 游标 {{ task.cursor_id }}
              </span>
              <span>
                创建 {{ formatDateTime(task.created_at) }}
                <template v-if="task.finished_at"> · 结束 {{ formatDateTime(task.finished_at) }}</template>
              </span>
            </div>

            <!-- 错误或自动暂停提示 -->
            <div
              v-if="task.last_error"
              class="rounded-lg border px-2.5 py-1.5 text-[11px] font-mono break-all"
              :class="
                task.status === 'failed'
                  ? 'border-rose-500/35 bg-rose-950/35 text-rose-200'
                  : 'border-amber-500/30 bg-amber-950/25 text-amber-200'
              "
            >
              {{ task.last_error }}
            </div>

            <!-- 改动对比抽样展开 -->
            <div v-if="task.sample_diffs.length > 0" class="pt-1 border-t border-slate-800/60">
              <button
                type="button"
                class="text-[11px] text-cyan-300 hover:text-cyan-200 inline-flex items-center gap-1 cursor-pointer"
                @click="toggleDiffExpand(task.id)"
              >
                <ChevronDown v-if="expandedDiffIds.has(task.id)" class="w-3.5 h-3.5" />
                <ChevronRight v-else class="w-3.5 h-3.5" />
                查看变动抽样对比（前 {{ task.sample_diffs.length }} 条）
              </button>

              <div
                v-if="expandedDiffIds.has(task.id)"
                class="mt-2 rounded-lg border border-slate-800 bg-[#070a10] overflow-x-auto obs-scroll"
              >
                <table class="w-full text-left border-collapse text-[11px] font-mono">
                  <thead class="bg-[#0b0f17] border-b border-slate-800 text-[10px] text-slate-500">
                    <tr>
                      <th class="px-2.5 py-1.5 whitespace-nowrap">
                        {{ task.kind === "clean_garbled" ? "InfoHash / 命中原因" : "InfoHash / 标题" }}
                      </th>
                      <th class="px-2.5 py-1.5 whitespace-nowrap">
                        {{ task.kind === "clean_garbled" ? "乱码标题" : "修改前" }}
                      </th>
                      <th class="px-2.5 py-1.5 whitespace-nowrap">
                        {{ task.kind === "clean_garbled" ? "处理结果" : "修改后" }}
                      </th>
                    </tr>
                  </thead>
                  <tbody class="divide-y divide-slate-800/50">
                    <tr v-for="(diff, idx) in task.sample_diffs" :key="idx">
                      <td class="px-2.5 py-1.5 max-w-[200px]">
                        <div class="text-cyan-300 text-[10px] truncate">{{ diff.info_hash.slice(0, 12) }}…</div>
                        <div class="text-slate-300 font-sans truncate" :title="diff.label">{{ diff.label }}</div>
                      </td>
                      <td class="px-2.5 py-1.5 max-w-[220px] text-slate-400 truncate" :title="diff.before">
                        {{ diff.before || "（空）" }}
                      </td>
                      <td class="px-2.5 py-1.5 max-w-[220px] text-emerald-300 truncate" :title="diff.after">
                        {{ diff.after }}
                      </td>
                    </tr>
                  </tbody>
                </table>
              </div>
            </div>
          </div>
        </div>
      </section>
    </div>
  </ObsDrawer>
</template>
