<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { ElMessage } from "element-plus";
import {
  ChevronLeft,
  ChevronRight,
  Database,
  Layers,
  Pause,
  Play,
  Plus,
  RefreshCw,
  Search,
  Square,
  Trash2,
} from "lucide-vue-next";
import {
  batchDeleteTorrents,
  cancelTorrentTask,
  createTorrent,
  deleteTorrent,
  listTorrentTasks,
  listTorrents,
  pauseTorrentTask,
  resumeTorrentTask,
  type ListTorrentsQuery,
  type TorrentDetail,
  type TorrentFileInfo,
  type TorrentSummary,
  type TorrentTaskItem,
  type TorrentTasksOverview,
} from "@/api/torrents";
import TorrentDetailDrawer from "@/components/TorrentDetailDrawer.vue";
import TorrentTasksDrawer from "@/components/TorrentTasksDrawer.vue";
import ObsModal from "@/components/ui/ObsModal.vue";
import ObsNumberInput from "@/components/ui/ObsNumberInput.vue";
import ObsSelect from "@/components/ui/ObsSelect.vue";
import {
  copyToClipboard,
  fmtBytes,
  fmtCount,
  fmtExact,
  formatDateTime,
} from "@/utils/format";

const route = useRoute();
const router = useRouter();

const loading = ref(false);
const items = ref<TorrentSummary[]>([]);
const total = ref(0);
const totalRelation = ref("eq");
const tookMs = ref(0);
const hasTotalSizeColumn = ref(true);

const q = ref("");
const sortBy = ref<"updated_at" | "created_at" | "hot" | "file_count" | "total_size">("updated_at");
const sortDir = ref<"desc" | "asc">("desc");
const minHotText = ref("");
const maxHotText = ref("");
const minFilesText = ref("");
const minSizeText = ref("");
const maxSizeText = ref("");
const sizeUnit = ref<"MB" | "GB">("GB");
const emptyRefineOnly = ref(false);
const page = ref(1);
const pageSize = ref(20);

const selectedIds = ref<Set<string>>(new Set());

// Detail drawer
const detailVisible = ref(false);
const detailId = ref<string | null>(null);
const detailTab = ref<"files" | "edit">("files");

// Async maintenance tasks drawer & live banner
const tasksDrawerVisible = ref(false);
const activeTask = ref<TorrentTaskItem | null>(null);
const taskActionBusy = ref(false);
let activeTaskTimer: ReturnType<typeof setInterval> | null = null;
let lastTrackedRunningId: number | null = null;

// Create Modal
const createVisible = ref(false);
const createSubmitting = ref(false);
const createForm = ref<{
  info_hash: string;
  raw_name: string;
  refine_name: string;
  hot: number;
  sync_redis_indexed: boolean;
  files: TorrentFileInfo[];
}>({
  info_hash: "",
  raw_name: "",
  refine_name: "",
  hot: 1,
  sync_redis_indexed: true,
  files: [{ path: "", size: 0 }],
});

// Delete Confirm Modal
const deleteVisible = ref(false);
const deleteMode = ref<"single" | "batch">("single");
const deleteTarget = ref<TorrentSummary | null>(null);
const keepRedisIndexed = ref(true);
const deleteSubmitting = ref(false);

const SORT_OPTIONS = [
  { label: "更新时间", value: "updated_at" },
  { label: "入库时间", value: "created_at" },
  { label: "总体积", value: "total_size" },
  { label: "热度", value: "hot" },
  { label: "文件数", value: "file_count" },
];

const SIZE_UNIT_OPTIONS = [
  { label: "MB", value: "MB" },
  { label: "GB", value: "GB" },
];

const SORT_DIR_OPTIONS = [
  { label: "降序", value: "desc" },
  { label: "升序", value: "asc" },
];

const PAGE_SIZE_OPTIONS = [
  { label: "20 / 页", value: 20 },
  { label: "50 / 页", value: 50 },
  { label: "100 / 页", value: 100 },
];

function parseOptionalInt(raw: string): number | undefined {
  const s = raw.trim();
  if (!s) return undefined;
  const n = Number(s);
  return Number.isFinite(n) && n >= 0 ? Math.floor(n) : undefined;
}

function parseOptionalSizeBytes(raw: string): number | undefined {
  const s = raw.trim();
  if (!s) return undefined;
  const n = Number(s);
  if (!Number.isFinite(n) || n < 0) return undefined;
  const mul = sizeUnit.value === "GB" ? 1024 * 1024 * 1024 : 1024 * 1024;
  return Math.floor(n * mul);
}

const hasActiveFilters = computed(
  () =>
    q.value.trim() !== "" ||
    minHotText.value.trim() !== "" ||
    maxHotText.value.trim() !== "" ||
    minFilesText.value.trim() !== "" ||
    minSizeText.value.trim() !== "" ||
    maxSizeText.value.trim() !== "" ||
    emptyRefineOnly.value,
);

const totalPages = computed(() => Math.max(1, Math.ceil(total.value / pageSize.value)));

const allPageSelected = computed(() => {
  if (items.value.length === 0) return false;
  return items.value.every((row) => selectedIds.value.has(row.id));
});

const selectedCount = computed(() => selectedIds.value.size);

function toggleRowSelect(id: string) {
  const next = new Set(selectedIds.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  selectedIds.value = next;
}

function toggleAllOnPage() {
  const next = new Set(selectedIds.value);
  if (allPageSelected.value) {
    for (const row of items.value) next.delete(row.id);
  } else {
    for (const row of items.value) next.add(row.id);
  }
  selectedIds.value = next;
}

async function fetchList(preserveSelection = false) {
  loading.value = true;
  if (!preserveSelection) selectedIds.value = new Set();
  try {
    const query: ListTorrentsQuery = {
      q: q.value.trim() || undefined,
      sort_by: sortBy.value,
      sort_dir: sortDir.value,
      min_hot: parseOptionalInt(minHotText.value),
      max_hot: parseOptionalInt(maxHotText.value),
      min_files: parseOptionalInt(minFilesText.value),
      min_size: parseOptionalSizeBytes(minSizeText.value),
      max_size: parseOptionalSizeBytes(maxSizeText.value),
      empty_refine_only: emptyRefineOnly.value || undefined,
      page: page.value,
      page_size: pageSize.value,
    };
    const res = await listTorrents(query);
    items.value = res.items;
    total.value = res.total;
    totalRelation.value = res.total_relation;
    tookMs.value = res.took_ms;
    if (res.has_total_size_column !== undefined) {
      hasTotalSizeColumn.value = res.has_total_size_column;
    }
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "查询种子列表失败");
  } finally {
    loading.value = false;
  }
}

function triggerSearch() {
  page.value = 1;
  void fetchList();
}

function resetFilters() {
  q.value = "";
  sortBy.value = "updated_at";
  sortDir.value = "desc";
  minHotText.value = "";
  maxHotText.value = "";
  minFilesText.value = "";
  minSizeText.value = "";
  maxSizeText.value = "";
  emptyRefineOnly.value = false;
  page.value = 1;
  void fetchList();
}

function openDetail(row: TorrentSummary, tab: "files" | "edit" = "files") {
  detailTab.value = tab;
  detailId.value = row.id;
  detailVisible.value = true;
}

function openCreateModal() {
  createForm.value = {
    info_hash: "",
    raw_name: "",
    refine_name: "",
    hot: 1,
    sync_redis_indexed: true,
    files: [{ path: "", size: 0 }],
  };
  createVisible.value = true;
}

async function submitCreate() {
  const hash = createForm.value.info_hash.trim().toLowerCase();
  if (!/^[0-9a-f]{40}$/.test(hash)) {
    ElMessage.warning("请输入合法的 40 位十六进制 InfoHash");
    return;
  }
  const rawName = createForm.value.raw_name.trim();
  if (!rawName) {
    ElMessage.warning("请填写原始标题 raw_name");
    return;
  }
  const files = createForm.value.files
    .map((f) => ({ path: f.path.trim(), size: Math.max(0, Number(f.size) || 0) }))
    .filter((f) => f.path.length > 0);

  createSubmitting.value = true;
  try {
    await createTorrent({
      info_hash: hash,
      raw_name: rawName,
      refine_name: createForm.value.refine_name.trim() || undefined,
      files,
      hot: Math.max(0, Math.round(createForm.value.hot || 0)),
      sync_redis_indexed: createForm.value.sync_redis_indexed,
    });
    ElMessage.success("新种子记录已写入 Manticore");
    createVisible.value = false;
    await fetchList();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "写入失败");
  } finally {
    createSubmitting.value = false;
  }
}

function openSingleDelete(target: TorrentSummary | TorrentDetail) {
  deleteMode.value = "single";
  deleteTarget.value = target;
  keepRedisIndexed.value = true;
  deleteVisible.value = true;
}

function openBatchDelete() {
  if (selectedIds.value.size === 0) return;
  deleteMode.value = "batch";
  deleteTarget.value = null;
  keepRedisIndexed.value = true;
  deleteVisible.value = true;
}

async function confirmDelete() {
  deleteSubmitting.value = true;
  try {
    if (deleteMode.value === "single" && deleteTarget.value) {
      await deleteTorrent(deleteTarget.value.id, keepRedisIndexed.value);
      ElMessage.success("已删除种子记录");
      if (detailId.value === deleteTarget.value.id) {
        detailVisible.value = false;
      }
    } else if (deleteMode.value === "batch") {
      const ids = Array.from(selectedIds.value);
      const res = await batchDeleteTorrents(ids, keepRedisIndexed.value);
      ElMessage.success(`已批量删除 ${res.deleted} 条记录`);
      selectedIds.value = new Set();
    }
    deleteVisible.value = false;
    await fetchList(true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "删除操作失败");
  } finally {
    deleteSubmitting.value = false;
  }
}

async function handleCopy(text: string, label: string) {
  const ok = await copyToClipboard(text);
  if (ok) ElMessage.success(`已复制 ${label}`);
  else ElMessage.warning("剪贴板复制失败");
}

function focusSearchInputIfRequested() {
  if (route.query.focus !== undefined) {
    void nextTick(() => {
      document.getElementById("torrentSearchInput")?.focus();
      void router.replace({ path: "/torrents", query: {} });
    });
  }
}

function taskKindLabel(kind: TorrentTaskItem["kind"]): string {
  return kind === "sync_total_size" ? "同步文件大小" : "清洗标题";
}

function formatTaskEta(secs: number | null): string {
  if (secs == null || !Number.isFinite(secs) || secs < 0) return "估算中";
  if (secs < 60) return `${Math.ceil(secs)}s`;
  if (secs < 3600) {
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}m ${s}s`;
  }
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return `${h}h ${m}m`;
}

function stopActiveTaskPolling() {
  if (activeTaskTimer) {
    clearInterval(activeTaskTimer);
    activeTaskTimer = null;
  }
}

function updateActiveTaskPolling() {
  const shouldPoll = activeTask.value?.status === "running" && !tasksDrawerVisible.value;
  if (shouldPoll && !activeTaskTimer) {
    activeTaskTimer = setInterval(() => {
      void fetchActiveTask();
    }, 3000);
  } else if (!shouldPoll) {
    stopActiveTaskPolling();
  }
}

function syncActiveTaskFromOverview(overview: TorrentTasksOverview) {
  const prevRunningId = lastTrackedRunningId;
  const current =
    overview.active_task ??
    overview.tasks.find((t) => t.status === "paused") ??
    null;

  activeTask.value = current;

  if (overview.active_task?.status === "running") {
    lastTrackedRunningId = overview.active_task.id;
  } else if (prevRunningId !== null) {
    const finished = overview.tasks.find((t) => t.id === prevRunningId);
    if (finished && finished.status !== "running") {
      lastTrackedRunningId = null;
      if (finished.status === "completed") {
        ElMessage.success(
          `异步任务 #${finished.id}（${taskKindLabel(finished.kind)}）已完成，更新 ${fmtExact(finished.updated_count)} 条`,
        );
        void fetchList(true);
      } else if (finished.status === "failed") {
        ElMessage.error(`异步任务 #${finished.id} 异常中止`);
      }
    }
  }

  updateActiveTaskPolling();
}

async function fetchActiveTask() {
  try {
    const overview = await listTorrentTasks(10);
    syncActiveTaskFromOverview(overview);
  } catch {
    // Silent: avoid spamming toasts during background poll
  }
}

function onTasksChanged(overview: TorrentTasksOverview) {
  syncActiveTaskFromOverview(overview);
}

async function handleBannerPause() {
  if (!activeTask.value || taskActionBusy.value) return;
  taskActionBusy.value = true;
  try {
    await pauseTorrentTask(activeTask.value.id);
    ElMessage.success(`已发送暂停指令 (#${activeTask.value.id})`);
    await fetchActiveTask();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "暂停任务失败");
  } finally {
    taskActionBusy.value = false;
  }
}

async function handleBannerResume() {
  if (!activeTask.value || taskActionBusy.value) return;
  taskActionBusy.value = true;
  try {
    await resumeTorrentTask(activeTask.value.id);
    ElMessage.success(`已继续执行任务 #${activeTask.value.id}`);
    await fetchActiveTask();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "恢复任务失败");
  } finally {
    taskActionBusy.value = false;
  }
}

async function handleBannerCancel() {
  if (!activeTask.value || taskActionBusy.value) return;
  taskActionBusy.value = true;
  try {
    await cancelTorrentTask(activeTask.value.id);
    ElMessage.success(`已取消任务 #${activeTask.value.id}`);
    await fetchActiveTask();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "取消任务失败");
  } finally {
    taskActionBusy.value = false;
  }
}

watch(tasksDrawerVisible, (visible) => {
  updateActiveTaskPolling();
  if (!visible) {
    void fetchActiveTask();
  }
});

watch(
  () => route.query.focus,
  () => {
    focusSearchInputIfRequested();
  },
);

watch([page, pageSize], () => {
  void fetchList();
});

onMounted(() => {
  void fetchList();
  void fetchActiveTask();
  focusSearchInputIfRequested();
});

onUnmounted(() => {
  stopActiveTaskPolling();
});
</script>

<template>
  <div class="space-y-4">
    <!-- Search & Facet Toolbar -->
    <section class="obs-card rounded-xl p-4 space-y-3">
      <div class="flex flex-wrap items-center gap-2.5">
        <div class="relative flex-1 min-w-[260px]">
          <Search class="w-3.5 h-3.5 text-slate-500 absolute left-3 top-1/2 -translate-y-1/2 pointer-events-none" />
          <input
            id="torrentSearchInput"
            v-model="q"
            type="text"
            placeholder="全文检索标题、文件名，或精确输入 40 位 InfoHash…"
            class="obs-input w-full rounded-lg pl-9 pr-3 py-1.5 text-xs"
            @keyup.enter="triggerSearch"
          />
        </div>

        <button type="button" class="obs-btn obs-btn-primary px-3.5! py-1.5! text-xs!" @click="triggerSearch">
          检索
        </button>

        <ObsSelect
          v-model="sortBy"
          :options="SORT_OPTIONS"
          class="w-32"
          trigger-class="px-2.5! py-1.5! text-xs!"
          @change="triggerSearch"
        />

        <ObsSelect
          v-model="sortDir"
          :options="SORT_DIR_OPTIONS"
          mono
          class="w-24"
          trigger-class="px-2.5! py-1.5! text-xs!"
          @change="triggerSearch"
        />

        <div class="flex flex-wrap items-center gap-2 ml-auto">
          <button
            v-if="selectedCount > 0"
            type="button"
            class="obs-btn obs-btn-danger px-3! py-1.5! text-xs! inline-flex items-center gap-1"
            @click="openBatchDelete"
          >
            <Trash2 class="w-3.5 h-3.5" />
            批量删除 ({{ selectedCount }})
          </button>

          <button type="button" class="obs-btn obs-btn-ghost px-2.5! py-1.5! text-xs! inline-flex items-center gap-1" @click="fetchList()">
            <RefreshCw class="w-3.5 h-3.5" :class="loading ? 'animate-spin text-emerald-400' : ''" />
            刷新
          </button>

          <button
            type="button"
            class="obs-btn px-3! py-1.5! text-xs! inline-flex items-center gap-1.5"
            :class="
              activeTask?.status === 'running'
                ? 'bg-emerald-500/15 border border-emerald-500/40 text-emerald-300 hover:bg-emerald-500/25'
                : activeTask?.status === 'paused'
                  ? 'bg-amber-500/15 border border-amber-500/40 text-amber-300 hover:bg-amber-500/25'
                  : 'obs-btn-ghost'
            "
            @click="tasksDrawerVisible = true"
          >
            <Layers class="w-3.5 h-3.5" />
            <span>异步任务</span>
            <span
              v-if="activeTask?.status === 'running'"
              class="inline-flex items-center gap-1 ml-0.5 text-[10px] font-mono text-emerald-300"
            >
              <span class="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse" />
              {{ Math.min(100, Math.max(0, activeTask.progress_percent)).toFixed(0) }}%
            </span>
            <span
              v-else-if="activeTask?.status === 'paused'"
              class="inline-flex items-center gap-1 ml-0.5 text-[10px] font-mono text-amber-300"
            >
              <span class="w-1.5 h-1.5 rounded-full bg-amber-400" />
              已暂停
            </span>
          </button>

          <button type="button" class="obs-btn obs-btn-primary px-3! py-1.5! text-xs! inline-flex items-center gap-1" @click="openCreateModal">
            <Plus class="w-3.5 h-3.5" />
            手动录入种子
          </button>
        </div>
      </div>

      <!-- Always-visible compact inline filter strip -->
      <div class="flex flex-wrap items-center justify-between gap-2 pt-2 border-t border-slate-800/70 text-[11px] font-mono">
        <div class="flex flex-wrap items-center gap-2 text-slate-400">
          <span>热度</span>
          <input
            v-model="minHotText"
            type="text"
            inputmode="numeric"
            placeholder="最小"
            class="obs-input w-16! rounded px-2! py-1! text-[11px]!"
            @keyup.enter="triggerSearch"
          />
          <span class="text-slate-600">-</span>
          <input
            v-model="maxHotText"
            type="text"
            inputmode="numeric"
            placeholder="最大"
            class="obs-input w-16! rounded px-2! py-1! text-[11px]!"
            @keyup.enter="triggerSearch"
          />
          <span class="text-slate-700">|</span>
          <span>文件 ≥</span>
          <input
            v-model="minFilesText"
            type="text"
            inputmode="numeric"
            placeholder="不限"
            class="obs-input w-16! rounded px-2! py-1! text-[11px]!"
            @keyup.enter="triggerSearch"
          />
          <span class="text-slate-700">|</span>
          <span>体积</span>
          <input
            v-model="minSizeText"
            type="text"
            inputmode="decimal"
            placeholder="最小"
            class="obs-input w-16! rounded px-2! py-1! text-[11px]!"
            @keyup.enter="triggerSearch"
          />
          <span class="text-slate-600">-</span>
          <input
            v-model="maxSizeText"
            type="text"
            inputmode="decimal"
            placeholder="最大"
            class="obs-input w-16! rounded px-2! py-1! text-[11px]!"
            @keyup.enter="triggerSearch"
          />
          <ObsSelect
            v-model="sizeUnit"
            :options="SIZE_UNIT_OPTIONS"
            mono
            class="w-16"
            trigger-class="px-2! py-1! text-[11px]!"
            @change="(minSizeText.trim() || maxSizeText.trim()) && triggerSearch()"
          />
          <span class="text-slate-700">|</span>
          <label class="inline-flex items-center gap-1.5 cursor-pointer text-slate-300 select-none">
            <input v-model="emptyRefineOnly" type="checkbox" class="obs-checkbox" @change="triggerSearch" />
            <span>空清洗标题</span>
          </label>
          <button type="button" class="obs-btn obs-btn-ghost px-2! py-0.5! text-[11px]!" @click="triggerSearch">
            应用
          </button>
          <button
            v-if="hasActiveFilters"
            type="button"
            class="obs-btn obs-btn-ghost px-2! py-0.5! text-[11px]! text-amber-300!"
            @click="resetFilters"
          >
            重置
          </button>
        </div>

        <div class="flex items-center gap-2.5">
          <button
            v-if="!hasTotalSizeColumn"
            type="button"
            class="px-2 py-0.5 rounded bg-amber-500/10 border border-amber-500/30 text-[10px] text-amber-300 hover:bg-amber-500/20 transition cursor-pointer"
            title="前往 SQL 终端执行 ALTER TABLE torrents ADD COLUMN total_size bigint; 以开启体积排序/筛选"
            @click="router.push('/sql')"
          >
            未启用 total_size 列 · 去 SQL 添加
          </button>
          <span class="text-slate-400 font-mono-num">
            命中 {{ totalRelation === "gte" ? "≥ " : "" }}{{ fmtExact(total) }} 条 · {{ tookMs }}ms
          </span>
        </div>
      </div>
    </section>

    <!-- Active Async Task Inline Progress Banner -->
    <section
      v-if="activeTask && (activeTask.status === 'running' || activeTask.status === 'paused')"
      class="obs-card rounded-xl px-4 py-3 space-y-2 border"
      :class="
        activeTask.status === 'running'
          ? 'border-emerald-500/35 bg-emerald-500/[0.04]'
          : 'border-amber-500/35 bg-amber-500/[0.04]'
      "
    >
      <div class="flex flex-wrap items-center justify-between gap-2 text-xs">
        <div class="flex flex-wrap items-center gap-2">
          <span
            class="w-2 h-2 rounded-full shrink-0"
            :class="activeTask.status === 'running' ? 'bg-emerald-400 animate-pulse' : 'bg-amber-400'"
          />
          <span class="font-medium text-slate-100">
            {{ taskKindLabel(activeTask.kind) }}
          </span>
          <span class="font-mono text-[11px] text-slate-500">#{{ activeTask.id }}</span>
          <span
            class="px-1.5 py-0.5 rounded text-[10px] font-mono border"
            :class="
              activeTask.status === 'running'
                ? 'bg-emerald-500/15 border-emerald-500/35 text-emerald-300'
                : 'bg-amber-500/15 border-amber-500/35 text-amber-300'
            "
          >
            {{ activeTask.status === "running" ? "分批处理中" : "已暂停（可断点续跑）" }}
          </span>
          <span
            v-if="activeTask.params.dry_run"
            class="px-1.5 py-0.5 rounded text-[10px] font-mono bg-sky-500/15 border border-sky-500/35 text-sky-300"
          >
            演练预览
          </span>
          <span class="text-[11px] font-mono text-slate-400">
            已扫描 {{ fmtExact(activeTask.scanned_count) }}
            <template v-if="activeTask.estimated_total > 0">
              / {{ fmtExact(activeTask.estimated_total) }}
            </template>
            · 更新 <span class="text-emerald-300">{{ fmtExact(activeTask.updated_count) }}</span>
            <template v-if="activeTask.error_count > 0">
              · 错误 <span class="text-rose-300">{{ fmtExact(activeTask.error_count) }}</span>
            </template>
            <template v-if="activeTask.status === 'running'">
              · {{ activeTask.rate_per_sec.toFixed(0) }} 条/s · 预计剩余 {{ formatTaskEta(activeTask.eta_secs) }}
            </template>
          </span>
        </div>

        <div class="flex items-center gap-1.5">
          <button
            v-if="activeTask.status === 'running'"
            type="button"
            class="obs-btn obs-btn-ghost px-2.5! py-1! text-[11px]! inline-flex items-center gap-1"
            :disabled="taskActionBusy"
            @click="handleBannerPause"
          >
            <Pause class="w-3 h-3 text-amber-300" />
            暂停
          </button>
          <button
            v-else
            type="button"
            class="obs-btn obs-btn-primary px-2.5! py-1! text-[11px]! inline-flex items-center gap-1"
            :disabled="taskActionBusy"
            @click="handleBannerResume"
          >
            <Play class="w-3 h-3" />
            继续
          </button>
          <button
            type="button"
            class="obs-btn obs-btn-danger px-2.5! py-1! text-[11px]! inline-flex items-center gap-1"
            :disabled="taskActionBusy"
            @click="handleBannerCancel"
          >
            <Square class="w-3 h-3" />
            取消
          </button>
          <button
            type="button"
            class="obs-btn obs-btn-ghost px-2.5! py-1! text-[11px]! inline-flex items-center gap-1"
            @click="tasksDrawerVisible = true"
          >
            <Layers class="w-3 h-3 text-slate-300" />
            管理详情
          </button>
        </div>
      </div>

      <div class="flex items-center gap-3">
        <div class="flex-1 h-1.5 rounded-full bg-slate-800/90 overflow-hidden">
          <div
            class="h-full rounded-full transition-all duration-300"
            :class="activeTask.status === 'running' ? 'bg-emerald-400' : 'bg-amber-400'"
            :style="{ width: `${Math.min(100, Math.max(0, activeTask.progress_percent))}%` }"
          />
        </div>
        <span class="text-[11px] font-mono font-semibold text-slate-300 w-12 text-right">
          {{ Math.min(100, Math.max(0, activeTask.progress_percent)).toFixed(1) }}%
        </span>
      </div>
    </section>

    <!-- Results Table -->
    <section class="obs-card rounded-xl overflow-hidden">
      <div class="overflow-x-auto obs-scroll">
        <table class="w-full text-left border-collapse text-xs">
          <thead>
            <tr class="border-b border-slate-800 bg-[#0b0f17] text-[10px] font-mono uppercase tracking-wider text-slate-400">
              <th class="px-4 py-2.5 w-9">
                <input
                  type="checkbox"
                  class="obs-checkbox"
                  :checked="allPageSelected"
                  @change="toggleAllOnPage"
                />
              </th>
              <th class="px-3 py-2.5 min-w-[300px]">种子名称 / InfoHash</th>
              <th class="px-3 py-2.5 text-right w-20">文件数</th>
              <th class="px-3 py-2.5 text-right w-24">总体积</th>
              <th class="px-3 py-2.5 text-right w-24">热度</th>
              <th class="px-3 py-2.5 w-36">更新时间</th>
              <th class="px-4 py-2.5 text-right w-40">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="row in items"
              :key="row.id"
              class="group border-b border-slate-800/50 hover:bg-slate-800/35 transition-colors cursor-pointer"
              @click="openDetail(row, 'files')"
            >
              <td class="px-4 py-3 align-top" @click.stop>
                <input
                  type="checkbox"
                  class="obs-checkbox"
                  :checked="selectedIds.has(row.id)"
                  @change="toggleRowSelect(row.id)"
                />
              </td>
              <td class="px-3 py-3 max-w-xl align-top">
                <div class="font-medium text-slate-100 group-hover:text-emerald-300 transition-colors break-words line-clamp-2">
                  {{ row.refine_name || row.raw_name }}
                </div>
                <div
                  v-if="row.refine_name && row.refine_name !== row.raw_name"
                  class="mt-0.5 text-[11px] text-slate-500 truncate"
                  :title="row.raw_name"
                >
                  原标题: {{ row.raw_name }}
                </div>
                <div class="mt-1 flex flex-wrap items-center gap-2 text-[10px] font-mono text-slate-500">
                  <span>#{{ row.id }}</span>
                  <button
                    type="button"
                    class="text-cyan-400/90 hover:text-cyan-300 transition-colors cursor-pointer"
                    title="点击复制完整 InfoHash"
                    @click.stop="handleCopy(row.info_hash, 'InfoHash')"
                  >
                    {{ row.info_hash.slice(0, 14) }}…
                  </button>
                  <button
                    type="button"
                    class="text-sky-400/90 hover:text-sky-300 transition-colors cursor-pointer"
                    title="复制 Magnet 磁力链"
                    @click.stop="handleCopy(row.magnet_link, 'Magnet')"
                  >
                    🧲 磁力
                  </button>
                </div>
              </td>
              <td class="px-3 py-3 text-right font-mono-num text-slate-300 align-top whitespace-nowrap">
                {{ fmtCount(row.file_count) }}
              </td>
              <td class="px-3 py-3 text-right font-mono-num text-sky-300 align-top whitespace-nowrap">
                {{ fmtBytes(row.total_size) }}
              </td>
              <td class="px-3 py-3 text-right font-mono-num align-top whitespace-nowrap">
                <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-amber-500/10 border border-amber-500/25 text-amber-300 font-semibold text-[11px]">
                  🔥 {{ fmtCount(row.hot) }}
                </span>
              </td>
              <td class="px-3 py-3 font-mono-num text-[11px] text-slate-400 align-top whitespace-nowrap">
                {{ formatDateTime(row.updated_at) }}
              </td>
              <td class="px-4 py-3 text-right align-top whitespace-nowrap" @click.stop>
                <div class="inline-flex items-center gap-1.5">
                  <button
                    type="button"
                    class="obs-btn obs-btn-ghost px-2! py-1! text-[11px]!"
                    @click="openDetail(row, 'files')"
                  >
                    详情
                  </button>
                  <button
                    type="button"
                    class="obs-btn obs-btn-ghost px-2! py-1! text-[11px]!"
                    @click="openDetail(row, 'edit')"
                  >
                    编辑
                  </button>
                  <button
                    type="button"
                    class="obs-btn obs-btn-danger px-2! py-1! text-[11px]!"
                    @click="openSingleDelete(row)"
                  >
                    删
                  </button>
                </div>
              </td>
            </tr>

            <tr v-if="!loading && items.length === 0">
              <td colspan="7" class="py-16 text-center text-slate-500">
                <Database class="w-8 h-8 text-slate-600 mx-auto mb-2" />
                <div>未找到匹配的种子索引记录</div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <!-- Pagination Footer -->
      <div class="px-4 py-3 border-t border-slate-800 bg-[#0b0f17] flex flex-wrap items-center justify-between gap-3 text-xs">
        <div class="flex items-center gap-2">
          <ObsSelect
            v-model="pageSize"
            :options="PAGE_SIZE_OPTIONS"
            mono
            class="w-28"
            trigger-class="px-2.5! py-1! text-xs!"
          />
          <span class="font-mono-num text-[11px] text-slate-400">
            第 {{ page }} / {{ totalPages }} 页
          </span>
        </div>

        <div class="inline-flex items-center gap-1.5">
          <button
            type="button"
            class="obs-btn obs-btn-ghost px-2.5! py-1! text-xs! inline-flex items-center gap-1"
            :disabled="page <= 1"
            @click="page -= 1"
          >
            <ChevronLeft class="w-3.5 h-3.5" />
            上一页
          </button>
          <button
            type="button"
            class="obs-btn obs-btn-ghost px-2.5! py-1! text-xs! inline-flex items-center gap-1"
            :disabled="page >= totalPages"
            @click="page += 1"
          >
            下一页
            <ChevronRight class="w-3.5 h-3.5" />
          </button>
        </div>
      </div>
    </section>

    <!-- Reusable Torrent Detail & Edit Drawer -->
    <TorrentDetailDrawer
      v-model="detailVisible"
      :torrent-id="detailId"
      :initial-tab="detailTab"
      allow-delete
      @updated="fetchList(true)"
      @request-delete="openSingleDelete"
    />

    <!-- Create Torrent Modal -->
    <ObsModal v-model="createVisible" title="手动录入种子记录" subtitle="直接写入 Manticore RT 索引并可选标记 Redis 已入库集合" max-width="640px">
      <div class="space-y-3 text-xs">
        <div>
          <label class="block text-[11px] text-slate-400 mb-1">40 位十六进制 InfoHash *</label>
          <input
            v-model="createForm.info_hash"
            type="text"
            maxlength="40"
            placeholder="例如 0123456789abcdef0123456789abcdef01234567"
            class="obs-input px-3 py-1.5 text-xs font-mono"
          />
        </div>
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">原始标题 raw_name *</label>
            <input v-model="createForm.raw_name" type="text" class="obs-input px-3 py-1.5 text-xs" />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">清洗标题 refine_name（留空同 raw_name）</label>
            <input v-model="createForm.refine_name" type="text" class="obs-input px-3 py-1.5 text-xs" />
          </div>
        </div>
        <div class="grid grid-cols-2 gap-3 items-end">
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">初始热度 Hot</label>
            <ObsNumberInput v-model="createForm.hot" :min="0" />
          </div>
          <label class="inline-flex items-center gap-2 text-xs text-slate-300 cursor-pointer pb-1.5">
            <input v-model="createForm.sync_redis_indexed" type="checkbox" class="obs-checkbox" />
            同步标记至 Redis 去重集合
          </label>
        </div>

        <div class="space-y-2 pt-1">
          <div class="flex items-center justify-between">
            <span class="text-[11px] text-slate-400">文件列表</span>
            <button
              type="button"
              class="obs-btn obs-btn-ghost px-2 py-1 text-[11px] inline-flex items-center gap-1"
              @click="createForm.files.push({ path: '', size: 0 })"
            >
              <Plus class="w-3 h-3 text-emerald-400" />
              新增文件
            </button>
          </div>
          <div class="max-h-48 overflow-auto obs-scroll space-y-2 pr-1">
            <div v-for="(f, idx) in createForm.files" :key="idx" class="flex items-center gap-2">
              <input v-model="f.path" type="text" placeholder="dir/movie.mkv" class="obs-input px-2.5 py-1.5 text-xs font-mono flex-1" />
              <div class="w-36 shrink-0">
                <ObsNumberInput v-model="f.size" :min="0" :step="1024" />
              </div>
              <button
                type="button"
                class="obs-btn obs-btn-danger px-2 py-1.5 text-xs"
                @click="createForm.files.splice(idx, 1)"
              >
                删
              </button>
            </div>
          </div>
        </div>
      </div>

      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 text-xs" @click="createVisible = false">取消</button>
        <button type="button" class="obs-btn obs-btn-primary px-4 py-1.5 text-xs" :disabled="createSubmitting" @click="submitCreate">
          {{ createSubmitting ? "写入中…" : "确认写入 Manticore" }}
        </button>
      </template>
    </ObsModal>

    <!-- Delete Confirmation Modal -->
    <ObsModal
      v-model="deleteVisible"
      :title="deleteMode === 'single' ? '删除种子索引' : `批量删除 ${selectedCount} 条记录`"
      max-width="460px"
    >
      <div class="space-y-3 text-xs text-slate-300">
        <div v-if="deleteMode === 'single' && deleteTarget" class="obs-inset p-3 rounded-lg space-y-1">
          <div class="font-medium text-slate-100 break-words">
            {{ deleteTarget.refine_name || deleteTarget.raw_name }}
          </div>
          <div class="font-mono text-[11px] text-slate-500">{{ deleteTarget.info_hash }}</div>
        </div>
        <p v-else>
          确认从 Manticore 永久删除选中的 <span class="font-mono font-bold text-rose-300">{{ selectedCount }}</span> 条种子索引记录？
        </p>

        <div class="p-3 rounded-lg bg-amber-500/10 border border-amber-500/35">
          <label class="inline-flex items-center gap-2 text-xs text-amber-200 cursor-pointer">
            <input v-model="keepRedisIndexed" type="checkbox" class="obs-checkbox" />
            <span>保留 Redis 去重标记（防止爬虫重复抓取）</span>
          </label>
        </div>
      </div>

      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 text-xs" @click="deleteVisible = false">取消</button>
        <button type="button" class="obs-btn obs-btn-danger px-4 py-1.5 text-xs" :disabled="deleteSubmitting" @click="confirmDelete">
          {{ deleteSubmitting ? "删除中…" : "确认删除" }}
        </button>
      </template>
    </ObsModal>

    <!-- Async Maintenance Tasks Drawer -->
    <TorrentTasksDrawer v-model="tasksDrawerVisible" @changed="onTasksChanged" />
  </div>
</template>
