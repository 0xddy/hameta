<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { RouterLink, useRoute, useRouter } from "vue-router";
import { ElMessage } from "element-plus";
import {
  AlertTriangle,
  Cpu,
  History,
  Plus,
  RefreshCw,
} from "lucide-vue-next";
import {
  createWorker,
  defaultCrawlerConfig,
  defaultIndexerConfig,
  deleteWorker,
  getDhtProfiles,
  getWorkerConfig,
  listWorkerConfigVersions,
  listWorkers,
  normalizeCrawlerConfigPayload,
  restartWorker,
  rollbackWorkerConfig,
  startWorker,
  stopWorker,
  updateWorkerConfig,
  updateWorkerPolicy,
  type CrawlProfileDefaults,
  type CrawlProfileInfo,
  type CrawlerWorkerConfig,
  type IndexerWorkerConfig,
  type NetModeConfig,
  type WorkerConfigVersionItem,
  type WorkerSummaryItem,
} from "@/api/workers";
import type { WorkerRole, WorkerRuntimeState } from "@/api/overview";
import ObsDrawer from "@/components/ui/ObsDrawer.vue";
import ObsModal from "@/components/ui/ObsModal.vue";
import ObsNumberInput from "@/components/ui/ObsNumberInput.vue";
import ObsSelect from "@/components/ui/ObsSelect.vue";
import { DHT_PROFILES, DHT_PROFILE_SELECT_OPTIONS, getDhtProfileLevelLabel } from "@/data/dhtProfiles";
import { fmtBytes, fmtCount, fmtExact, formatDateTime, fmtRate } from "@/utils/format";

const route = useRoute();
const router = useRouter();

const loading = ref(false);
const workers = ref<WorkerSummaryItem[]>([]);
const roleFilter = ref<"all" | WorkerRole>("all");
const actionBusyId = ref<string | null>(null);
const profiles = ref<CrawlProfileInfo[]>([]);
const systemSoftNofile = ref<number | null>(null);
let pollTimer: number | null = null;

// Create Worker Modal
const createVisible = ref(false);
const createSubmitting = ref(false);
const createForm = ref<{
  id: string;
  role: WorkerRole;
  display_name: string;
  port: number;
  netmode: NetModeConfig;
  crawl_profile: string;
  torrent_workers: number;
  insert_workers: number;
  batch_size: number;
  auto_restart: boolean;
  max_restarts: number;
  restart_backoff_secs: number;
}>({
  id: "crawler-1",
  role: "crawler",
  display_name: "DHT 爬虫实例 #1",
  port: 12313,
  netmode: "ipv4_only",
  crawl_profile: "P5",
  torrent_workers: 64,
  insert_workers: 64,
  batch_size: 1000,
  auto_restart: true,
  max_restarts: 5,
  restart_backoff_secs: 3,
});

// Policy Modal
const policyVisible = ref(false);
const policySubmitting = ref(false);
const policyTargetId = ref("");
const policyForm = ref({
  display_name: "",
  auto_restart: true,
  max_restarts: 5,
  restart_backoff_secs: 3,
});

// Stderr Modal
const stderrVisible = ref(false);
const stderrTarget = ref<WorkerSummaryItem | null>(null);

// Delete Confirm Modal
const deleteVisible = ref(false);
const deleteTarget = ref<WorkerSummaryItem | null>(null);
const deleteBusy = ref(false);

// Tuning Drawer
const tuneVisible = ref(false);
const tuneLoading = ref(false);
const tuneSaving = ref(false);
const tuneWorker = ref<WorkerSummaryItem | null>(null);
const tuneTab = ref<"l1" | "l2" | "l3" | "json" | "versions">("l1");
const tuneVersion = ref(0);
const tuneNote = ref("");
const tuneApplyRestart = ref(true);
const crawlerCfg = ref<CrawlerWorkerConfig>(defaultCrawlerConfig());
const indexerCfg = ref<IndexerWorkerConfig>(defaultIndexerConfig());
const bootstrapNodesText = ref("");
const rawJsonText = ref("");
const rawJsonError = ref("");
const versions = ref<WorkerConfigVersionItem[]>([]);
const versionsLoading = ref(false);
const rollbackBusyVer = ref<number | null>(null);
const showProfilesTable = ref(false);

const NETMODE_OPTIONS = [
  { label: "仅 IPv4 (ipv4_only)", value: "ipv4_only" },
  { label: "仅 IPv6 (ipv6_only)", value: "ipv6_only" },
  { label: "双栈 Dual-Stack", value: "dual" },
];

const LOG_LEVEL_OPTIONS = [
  { label: "error", value: "error" },
  { label: "warn", value: "warn" },
  { label: "info", value: "info" },
  { label: "debug", value: "debug" },
];

const profileSelectOptions = computed(() => {
  if (profiles.value.length > 0) {
    return profiles.value.map((p) => {
      const tier = getDhtProfileLevelLabel(p.name);
      return {
        value: p.name,
        label: tier ? `${p.name} · ${tier}` : p.name,
      };
    });
  }
  return DHT_PROFILE_SELECT_OPTIONS;
});

const selectedCreateProfileMeta = computed(() =>
  DHT_PROFILES.find((p) => p.name === createForm.value.crawl_profile) ?? DHT_PROFILES[4],
);

const selectedTuneProfileMeta = computed(() =>
  DHT_PROFILES.find((p) => p.name === crawlerCfg.value.crawl_profile) ?? null,
);

const currentTuneDefaults = computed<Partial<CrawlProfileDefaults>>(() => {
  const apiMatch = profiles.value.find((p) => p.name === crawlerCfg.value.crawl_profile);
  if (apiMatch) return apiMatch.defaults;
  const st = selectedTuneProfileMeta.value;
  if (!st) return {};
  return {
    find_node_rate_per_sec: st.qps,
    find_node_burst: st.burst,
    find_node_max_in_flight: st.inFlight,
    max_new_destinations_per_minute: st.maxNewDest,
    max_response_rate_per_sec: st.respRate,
    max_response_bytes_per_sec: st.respBytes,
    max_response_rate_per_source: st.respSrc,
    max_replacements_per_minute: st.replPerMin,
    max_in_flight_per_subnet: st.subInFlight,
    peer_lookup_rate_per_sec: st.peerLookup,
    peer_lookup_burst: st.peerBurst,
    peer_lookup_max_active: st.peerActive,
    pool_capacity: st.poolCap,
    responsive_capacity: st.respCap,
    routing_snapshot_size: st.snapSize,
  };
});

function recNofileOf(profileName: string): number {
  const api = profiles.value.find((p) => p.name === profileName);
  if (api) return api.recommended_nofile;
  return DHT_PROFILES.find((p) => p.name === profileName)?.recFd ?? 16384;
}

const profileTableRows = computed(() =>
  DHT_PROFILES.map((s) => {
    const api = profiles.value.find((p) => p.name === s.name);
    return {
      name: s.name,
      level: s.level,
      qps: api?.defaults.find_node_rate_per_sec ?? s.qps,
      inFlight: api?.defaults.find_node_max_in_flight ?? s.inFlight,
      peerLookup: api?.defaults.peer_lookup_rate_per_sec ?? s.peerLookup,
      poolCap: api?.defaults.pool_capacity ?? s.poolCap,
      recFd: api?.recommended_nofile ?? s.recFd,
    };
  }),
);

function isSet(v: unknown): boolean {
  return v !== undefined && v !== null && v !== "";
}

function countSetObj(obj: unknown): number {
  if (!obj || typeof obj !== "object") return 0;
  let n = 0;
  for (const val of Object.values(obj as Record<string, unknown>)) {
    if (Array.isArray(val)) {
      if (val.length > 0) n += 1;
    } else if (val !== undefined && val !== null && val !== "") {
      n += 1;
    }
  }
  return n;
}

const peerOvCount = computed(() => countSetObj(crawlerCfg.value.peer_lookup));
const l3OvCount = computed(
  () =>
    countSetObj(crawlerCfg.value.crawl.rate_limit) +
    countSetObj(crawlerCfg.value.crawl.pool) +
    countSetObj(crawlerCfg.value.crawl.target) +
    countSetObj(crawlerCfg.value.crawl.scheduler) +
    countSetObj(crawlerCfg.value.crawl.bootstrap),
);

function clearTuningGroup(
  group: "peer_lookup" | "rate_limit" | "pool" | "target" | "scheduler" | "bootstrap",
) {
  if (group === "peer_lookup") {
    crawlerCfg.value.peer_lookup = {};
  } else if (group === "rate_limit") {
    crawlerCfg.value.crawl.rate_limit = {};
  } else if (group === "pool") {
    crawlerCfg.value.crawl.pool = {};
  } else if (group === "target") {
    crawlerCfg.value.crawl.target = {};
  } else if (group === "scheduler") {
    crawlerCfg.value.crawl.scheduler = {};
  } else if (group === "bootstrap") {
    crawlerCfg.value.crawl.bootstrap = {};
    bootstrapNodesText.value = "";
  }
}

const NETMODE_LABELS: Record<string, string> = {
  ipv4_only: "IPv4",
  ipv6_only: "IPv6",
  dual: "双栈",
};

function workerRss(w: WorkerSummaryItem): number {
  const payload = w.snapshot?.payload as Record<string, unknown> | undefined;
  const r = payload?.rss_bytes;
  return typeof r === "number" && Number.isFinite(r) ? r : 0;
}

function workerSummaryText(w: WorkerSummaryItem): string {
  const cfg = w.summary;
  const pay = w.snapshot?.payload as Record<string, unknown> | undefined;
  if (w.role === "crawler") {
    const prof = String(cfg?.crawl_profile ?? pay?.crawl_profile ?? "-");
    const net = NETMODE_LABELS[String(cfg?.netmode ?? "")] ?? "IPv4";
    const port = w.worker_port ?? (typeof cfg?.port === "number" ? cfg.port : undefined);
    const metaW =
      typeof pay?.effective_metadata_workers === "number"
        ? pay.effective_metadata_workers
        : typeof cfg?.max_worker_count === "number"
          ? cfg.max_worker_count
          : 0;
    const dhtObj = pay?.dht as Record<string, unknown> | undefined;
    const rate =
      typeof dhtObj?.find_node_effective_rate_per_sec === "number"
        ? dhtObj.find_node_effective_rate_per_sec
        : 0;
    return `${prof} · ${net} · UDP:${port ?? "-"} · BEP-9 并发 ${metaW || "-"}${
      rate > 0 ? ` · 探测 ${fmtRate(rate)}/s` : ""
    }`;
  }
  const tw = typeof cfg?.torrent_workers === "number" ? cfg.torrent_workers : "-";
  const iw = typeof cfg?.insert_workers === "number" ? cfg.insert_workers : "-";
  const bs = typeof cfg?.batch_size === "number" ? fmtCount(cfg.batch_size) : "-";
  const appObj = pay?.app as Record<string, unknown> | undefined;
  const written = typeof appObj?.records_written === "number" ? appObj.records_written : 0;
  return `Worker ${tw} · Insert ${iw} · Batch ${bs}${
    w.active_rule_version ? ` · Rule v${w.active_rule_version}` : ""
  }${written > 0 ? ` · 累计 ${fmtCount(written)}` : ""}`;
}

const tuneNofileWarn = computed(() => {
  if (tuneWorker.value?.role !== "crawler" || systemSoftNofile.value === null) return null;
  const rec = recNofileOf(crawlerCfg.value.crawl_profile);
  if (systemSoftNofile.value < rec) {
    return `当前系统 ulimit -n (${systemSoftNofile.value}) 低于 ${crawlerCfg.value.crawl_profile} 建议阈值 (${rec})，高并发连接可能触发 EMFILE`;
  }
  return null;
});

const filteredWorkers = computed(() => {
  if (roleFilter.value === "all") return workers.value;
  return workers.value.filter((w) => w.role === roleFilter.value);
});

function runtimeBadge(state: WorkerRuntimeState): { text: string; cls: string; dot: string } {
  switch (state) {
    case "running":
      return { text: "RUNNING", cls: "bg-emerald-500/15 text-emerald-300", dot: "bg-emerald-400" };
    case "starting":
      return { text: "STARTING", cls: "bg-sky-500/15 text-sky-300", dot: "bg-sky-400 animate-pulse" };
    case "draining":
      return { text: "DRAINING", cls: "bg-amber-500/15 text-amber-300", dot: "bg-amber-400 animate-pulse" };
    case "crashed":
      return { text: "CRASHED", cls: "bg-rose-500/15 text-rose-300", dot: "bg-rose-400" };
    default:
      return { text: "STOPPED", cls: "bg-slate-800 text-slate-400", dot: "bg-slate-500" };
  }
}

async function fetchWorkers(silent = false) {
  if (!silent) loading.value = true;
  try {
    workers.value = await listWorkers();
  } catch (e) {
    if (!silent) ElMessage.error(e instanceof Error ? e.message : "读取 Worker 列表失败");
  } finally {
    loading.value = false;
  }
}

async function fetchProfiles() {
  try {
    const res = await getDhtProfiles();
    profiles.value = res.profiles;
    systemSoftNofile.value = res.system_soft_nofile;
  } catch {
    // fallback to static DHT_PROFILES
  }
}

function openCreateModal(presetRole: WorkerRole = "crawler") {
  const sameRoleCount = workers.value.filter((w) => w.role === presetRole).length;
  const idx = sameRoleCount + 1;
  createForm.value = {
    id: presetRole === "crawler" ? `crawler-${idx}` : `indexer-${idx}`,
    role: presetRole,
    display_name: presetRole === "crawler" ? `DHT 爬虫实例 #${idx}` : `Manticore 索引器 #${idx}`,
    port: 12313 + workers.value.filter((w) => w.role === "crawler").length,
    netmode: "ipv4_only",
    crawl_profile: "P5",
    torrent_workers: 64,
    insert_workers: 64,
    batch_size: 1000,
    auto_restart: true,
    max_restarts: 5,
    restart_backoff_secs: 3,
  };
  createVisible.value = true;
}

async function submitCreate() {
  const id = createForm.value.id.trim();
  if (!id) {
    ElMessage.warning("请填写唯一的实例 ID");
    return;
  }
  const cfg =
    createForm.value.role === "crawler"
      ? (() => {
          const base = defaultCrawlerConfig(createForm.value.port, createForm.value.crawl_profile);
          base.netmode = createForm.value.netmode;
          return base;
        })()
      : (() => {
          const base = defaultIndexerConfig();
          base.torrent_workers = createForm.value.torrent_workers;
          base.insert_workers = createForm.value.insert_workers;
          base.batch_size = createForm.value.batch_size;
          return base;
        })();

  createSubmitting.value = true;
  try {
    await createWorker({
      id,
      role: createForm.value.role,
      display_name: createForm.value.display_name.trim() || id,
      config: cfg,
      auto_restart: createForm.value.auto_restart,
      max_restarts: createForm.value.max_restarts,
      restart_backoff_secs: createForm.value.restart_backoff_secs,
    });
    ElMessage.success(`已创建 Worker 实例 ${id}`);
    createVisible.value = false;
    await fetchWorkers();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "创建失败");
  } finally {
    createSubmitting.value = false;
  }
}

async function runLifecycle(w: WorkerSummaryItem, action: "start" | "stop" | "restart") {
  actionBusyId.value = w.id;
  try {
    if (action === "start") {
      await startWorker(w.id);
      ElMessage.success(`已向 ${w.id} 发送启动指令`);
    } else if (action === "stop") {
      await stopWorker(w.id);
      ElMessage.success(`已触发 ${w.id} 优雅停止`);
    } else {
      await restartWorker(w.id);
      ElMessage.success(`已重启 ${w.id}`);
    }
    await fetchWorkers(true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "执行操作失败");
  } finally {
    actionBusyId.value = null;
  }
}

function openPolicy(w: WorkerSummaryItem) {
  policyTargetId.value = w.id;
  policyForm.value = {
    display_name: w.display_name,
    auto_restart: w.auto_restart,
    max_restarts: w.max_restarts,
    restart_backoff_secs: w.restart_backoff_secs,
  };
  policyVisible.value = true;
}

async function submitPolicy() {
  policySubmitting.value = true;
  try {
    await updateWorkerPolicy(policyTargetId.value, {
      display_name: policyForm.value.display_name.trim() || policyTargetId.value,
      auto_restart: policyForm.value.auto_restart,
      max_restarts: policyForm.value.max_restarts,
      restart_backoff_secs: policyForm.value.restart_backoff_secs,
    });
    ElMessage.success("守护策略已更新");
    policyVisible.value = false;
    await fetchWorkers(true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "保存失败");
  } finally {
    policySubmitting.value = false;
  }
}

function openStderr(w: WorkerSummaryItem) {
  stderrTarget.value = w;
  stderrVisible.value = true;
}

function askDelete(w: WorkerSummaryItem) {
  deleteTarget.value = w;
  deleteVisible.value = true;
}

async function confirmDelete() {
  if (!deleteTarget.value) return;
  deleteBusy.value = true;
  try {
    await deleteWorker(deleteTarget.value.id);
    ElMessage.success(`已删除 ${deleteTarget.value.id}`);
    deleteVisible.value = false;
    await fetchWorkers();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "删除失败");
  } finally {
    deleteBusy.value = false;
  }
}

function hydrateCrawlerForm(raw: CrawlerWorkerConfig) {
  const def = defaultCrawlerConfig(raw.port ?? 12313, raw.crawl_profile ?? "P5");
  crawlerCfg.value = {
    ...def,
    ...raw,
    metadata: { ...def.metadata, ...(raw.metadata ?? {}) },
    discovery: { ...def.discovery, ...(raw.discovery ?? {}) },
    peer_lookup: { ...(raw.peer_lookup ?? {}) },
    crawl: {
      rate_limit: { ...(raw.crawl?.rate_limit ?? {}) },
      pool: { ...(raw.crawl?.pool ?? {}) },
      bootstrap: { ...(raw.crawl?.bootstrap ?? {}) },
      target: { ...(raw.crawl?.target ?? {}) },
      scheduler: { ...(raw.crawl?.scheduler ?? {}) },
    },
  };
  bootstrapNodesText.value = (raw.crawl?.bootstrap?.nodes ?? []).join("\n");
}

function syncJsonPreview() {
  if (!tuneWorker.value) return;
  if (tuneWorker.value.role === "crawler") {
    const cloned = JSON.parse(JSON.stringify(crawlerCfg.value));
    const lines = bootstrapNodesText.value
      .split(/\r?\n/)
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    if (lines.length > 0) cloned.crawl.bootstrap.nodes = lines;
    else delete cloned.crawl.bootstrap.nodes;
    rawJsonText.value = JSON.stringify(normalizeCrawlerConfigPayload(cloned), null, 2);
  } else {
    rawJsonText.value = JSON.stringify(indexerCfg.value, null, 2);
  }
  rawJsonError.value = "";
}

function applyJsonToForm(): boolean {
  if (!tuneWorker.value) return false;
  try {
    const parsed = JSON.parse(rawJsonText.value) as Record<string, unknown>;
    if (tuneWorker.value.role === "crawler") {
      hydrateCrawlerForm(parsed as unknown as CrawlerWorkerConfig);
    } else {
      indexerCfg.value = { ...defaultIndexerConfig(), ...(parsed as unknown as IndexerWorkerConfig) };
    }
    rawJsonError.value = "";
    return true;
  } catch (e) {
    rawJsonError.value = e instanceof Error ? e.message : "JSON 解析错误";
    return false;
  }
}

async function openTuningDrawer(w: WorkerSummaryItem) {
  tuneWorker.value = w;
  tuneTab.value = "l1";
  tuneNote.value = "";
  tuneApplyRestart.value = w.runtime_state === "running" || w.runtime_state === "draining";
  tuneVisible.value = true;
  tuneLoading.value = true;
  versions.value = [];

  try {
    const detail = await getWorkerConfig(w.id);
    tuneVersion.value = detail.version;
    if (w.role === "crawler") {
      hydrateCrawlerForm(detail.config as CrawlerWorkerConfig);
    } else {
      indexerCfg.value = { ...defaultIndexerConfig(), ...(detail.config as IndexerWorkerConfig) };
    }
    syncJsonPreview();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "读取 Worker 配置失败");
  } finally {
    tuneLoading.value = false;
  }
}

async function loadVersions() {
  if (!tuneWorker.value) return;
  versionsLoading.value = true;
  try {
    versions.value = await listWorkerConfigVersions(tuneWorker.value.id, 25);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "加载配置历史版本失败");
  } finally {
    versionsLoading.value = false;
  }
}

function selectTuneTab(next: "l1" | "l2" | "l3" | "json" | "versions") {
  if (tuneTab.value === "json" && next !== "json") {
    if (!applyJsonToForm()) {
      ElMessage.warning("请先修正 JSON 语法错误");
      return;
    }
  }
  tuneTab.value = next;
  if (next === "json") syncJsonPreview();
  if (next === "versions") void loadVersions();
}

async function submitTuningConfig() {
  if (!tuneWorker.value) return;
  if (tuneTab.value === "json" && !applyJsonToForm()) {
    ElMessage.error("JSON 语法无效，无法提交");
    return;
  }

  let finalCfg: CrawlerWorkerConfig | IndexerWorkerConfig;
  if (tuneWorker.value.role === "crawler") {
    const cloned = JSON.parse(JSON.stringify(crawlerCfg.value));
    const lines = bootstrapNodesText.value
      .split(/\r?\n/)
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    if (lines.length > 0) cloned.crawl.bootstrap.nodes = lines;
    else delete cloned.crawl.bootstrap.nodes;
    finalCfg = normalizeCrawlerConfigPayload(cloned);
  } else {
    finalCfg = { ...indexerCfg.value };
  }

  tuneSaving.value = true;
  try {
    const res = await updateWorkerConfig(tuneWorker.value.id, {
      config: finalCfg,
      note: tuneNote.value.trim() || "通过面板参数调优保存",
      apply_restart: tuneApplyRestart.value,
    });
    tuneVersion.value = res.version;
    if (res.restart_error) {
      ElMessage.warning(`配置已保存为 v${res.version}，但热重启失败: ${res.restart_error}`);
    } else if (res.restarted) {
      ElMessage.success(`配置已保存为 v${res.version} 并自动重启生效`);
    } else {
      ElMessage.success(`配置已保存为 v${res.version}`);
    }
    syncJsonPreview();
    await fetchWorkers(true);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "保存配置失败");
  } finally {
    tuneSaving.value = false;
  }
}

async function handleRollback(ver: number) {
  if (!tuneWorker.value) return;
  rollbackBusyVer.value = ver;
  try {
    const res = await rollbackWorkerConfig(tuneWorker.value.id, {
      target_version: ver,
      apply_restart: tuneApplyRestart.value,
    });
    tuneVersion.value = res.version;
    if (tuneWorker.value.role === "crawler") {
      hydrateCrawlerForm(res.config as CrawlerWorkerConfig);
    } else {
      indexerCfg.value = { ...defaultIndexerConfig(), ...(res.config as IndexerWorkerConfig) };
    }
    syncJsonPreview();
    await loadVersions();
    await fetchWorkers(true);
    ElMessage.success(`已回滚并生成新版本 v${res.version}`);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "回滚失败");
  } finally {
    rollbackBusyVer.value = null;
  }
}

function handleTuneQuery() {
  const q = route.query.tune;
  if (typeof q === "string" && q.trim()) {
    const found = workers.value.find((w) => w.id === q.trim());
    if (found) {
      void openTuningDrawer(found);
      void router.replace({ path: "/workers", query: {} });
    }
  }
}

watch(
  () => [route.query.tune, workers.value.length] as const,
  () => {
    handleTuneQuery();
  },
);

onMounted(async () => {
  await Promise.all([fetchWorkers(), fetchProfiles()]);
  handleTuneQuery();
  pollTimer = window.setInterval(() => {
    void fetchWorkers(true);
  }, 4000);
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
    <!-- Frameless Top Status & Action Bar -->
    <div class="space-y-3">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div class="flex flex-wrap items-center gap-2 text-xs font-mono-num">
          <span class="px-2.5 py-1 rounded-lg bg-slate-900 border border-slate-800 text-slate-300 inline-flex items-center gap-2">
            <span class="h-1.5 w-1.5 rounded-full bg-emerald-400"></span>
            本机进程守护 · 异常自动拉起 · 本地安全通道
          </span>
          <span
            v-if="systemSoftNofile !== null"
            class="px-2.5 py-1 rounded-lg bg-emerald-500/10 border border-emerald-500/25 text-emerald-300"
            :title="fmtExact(systemSoftNofile)"
          >
            soft nofile {{ fmtCount(systemSoftNofile) }}
          </span>
        </div>

        <div class="flex items-center gap-2">
          <button
            type="button"
            class="obs-btn-ghost px-3 py-2 rounded-lg text-xs inline-flex items-center gap-1.5 cursor-pointer"
            @click="fetchWorkers()"
          >
            <RefreshCw class="w-3.5 h-3.5" :class="loading ? 'animate-spin text-emerald-400' : ''" />
            刷新
          </button>
          <button
            type="button"
            class="obs-btn-primary px-3.5 py-2 rounded-lg text-xs cursor-pointer"
            @click="openCreateModal('crawler')"
          >
            + 新建 Worker 进程
          </button>
        </div>
      </div>

      <!-- Dedicated Role Filter Tabs Row -->
      <div class="flex items-center">
        <div class="inline-flex items-center gap-1 p-1 rounded-xl bg-[#0b0f17] border border-slate-800 text-xs">
          <button
            type="button"
            class="px-3 py-1.5 rounded-lg cursor-pointer transition"
            :class="roleFilter === 'all' ? 'bg-emerald-500/18 text-emerald-300 font-medium border border-emerald-500/30' : 'text-slate-400 hover:text-slate-200 border border-transparent'"
            @click="roleFilter = 'all'"
          >
            全部 ({{ workers.length }})
          </button>
          <button
            type="button"
            class="px-3 py-1.5 rounded-lg cursor-pointer transition"
            :class="roleFilter === 'crawler' ? 'bg-cyan-500/18 text-cyan-300 font-medium border border-cyan-500/30' : 'text-slate-400 hover:text-slate-200 border border-transparent'"
            @click="roleFilter = 'crawler'"
          >
            Crawler ({{ workers.filter((w) => w.role === "crawler").length }})
          </button>
          <button
            type="button"
            class="px-3 py-1.5 rounded-lg cursor-pointer transition"
            :class="roleFilter === 'indexer' ? 'bg-emerald-500/18 text-emerald-300 font-medium border border-emerald-500/30' : 'text-slate-400 hover:text-slate-200 border border-transparent'"
            @click="roleFilter = 'indexer'"
          >
            Indexer ({{ workers.filter((w) => w.role === "indexer").length }})
          </button>
        </div>
      </div>
    </div>

    <!-- Workers Card Grid -->
    <div v-if="filteredWorkers.length > 0" class="grid grid-cols-1 lg:grid-cols-2 gap-4">
      <article
        v-for="w in filteredWorkers"
        :key="w.id"
        class="obs-card obs-card-hover rounded-xl p-5 space-y-4"
      >
        <div class="flex flex-wrap items-start justify-between gap-3">
          <div class="flex items-center gap-3 min-w-0">
            <div
              class="h-10 w-10 rounded-xl border flex items-center justify-center font-mono-num font-bold text-xs shrink-0"
              :class="
                w.role === 'crawler'
                  ? 'bg-cyan-500/12 border-cyan-500/25 text-cyan-400'
                  : 'bg-emerald-500/12 border-emerald-500/25 text-emerald-400'
              "
            >
              {{ w.id.slice(0, 1).toUpperCase() }}{{ w.id.slice(-1) }}
            </div>
            <div class="min-w-0">
              <div class="flex flex-wrap items-center gap-2">
                <span class="font-mono-num font-semibold text-slate-100">{{ w.id }}</span>
                <span
                  class="px-1.5 py-0.5 rounded text-[10px] font-mono-num"
                  :class="runtimeBadge(w.runtime_state).cls"
                >
                  {{ runtimeBadge(w.runtime_state).text }}
                </span>
                <span class="px-1.5 py-0.5 rounded bg-slate-800 text-slate-300 text-[10px] font-mono-num">
                  cfg v{{ w.active_config_version }}
                </span>
                <span
                  v-if="w.active_rule_version"
                  class="px-1.5 py-0.5 rounded bg-emerald-500/10 text-emerald-300 text-[10px] font-mono-num"
                >
                  ArcSwap v{{ w.active_rule_version }}
                </span>
              </div>
              <div class="text-xs text-slate-400 mt-0.5 truncate">
                {{ w.display_name }} · PID {{ w.pid ?? "-" }}
              </div>
            </div>
          </div>

          <div class="flex flex-wrap items-center gap-1.5">
            <button
              v-if="w.runtime_state === 'stopped' || w.runtime_state === 'crashed'"
              type="button"
              class="obs-btn-primary px-3 py-1.5 rounded-lg text-xs cursor-pointer"
              :disabled="actionBusyId === w.id"
              @click="runLifecycle(w, 'start')"
            >
              启动
            </button>
            <template v-else>
              <button
                type="button"
                class="obs-btn-ghost px-2.5 py-1.5 rounded-lg text-xs text-amber-300 cursor-pointer"
                :disabled="actionBusyId === w.id"
                @click="runLifecycle(w, 'stop')"
              >
                停止
              </button>
              <button
                type="button"
                class="obs-btn-ghost px-2.5 py-1.5 rounded-lg text-xs cursor-pointer"
                :disabled="actionBusyId === w.id"
                @click="runLifecycle(w, 'restart')"
              >
                重启
              </button>
            </template>
            <RouterLink
              :to="`/workers/${encodeURIComponent(w.id)}/diagnostics`"
              class="obs-btn-ghost px-2.5 py-1.5 rounded-lg text-xs text-cyan-300 cursor-pointer inline-block"
            >
              诊断
            </RouterLink>
            <button
              type="button"
              class="obs-btn-primary px-3 py-1.5 rounded-lg text-xs cursor-pointer"
              @click="openTuningDrawer(w)"
            >
              调优
            </button>
          </div>
        </div>

        <div class="obs-inset rounded-lg px-3 py-2 text-xs font-mono-num text-slate-300 truncate">
          {{ workerSummaryText(w) }}
        </div>

        <div
          v-if="w.drain_status && w.drain_status.draining"
          class="obs-inset px-3 py-2 rounded-lg border-amber-500/30 text-[11px] font-mono-num text-amber-300 flex items-center justify-between"
        >
          <span>正在优雅排空内存队列…</span>
          <span>排队 {{ w.drain_status.queued_items }} · 处理中 {{ w.drain_status.in_flight_items }}</span>
        </div>

        <div
          v-if="w.last_error"
          class="px-3 py-2 rounded-lg bg-rose-950/35 border border-rose-500/30 text-[11px] font-mono-num text-rose-300 break-all"
        >
          {{ w.last_error }}
        </div>

        <div class="flex flex-wrap items-center justify-between gap-2 text-[11px] text-slate-500 font-mono-num">
          <span>
            自动拉起: {{ w.auto_restart ? "ON" : "OFF" }} ({{ w.restart_count }}/{{ w.max_restarts }}) · RSS {{ workerRss(w) > 0 ? fmtBytes(workerRss(w)) : "-" }}
          </span>
          <div class="flex items-center gap-3">
            <button
              type="button"
              class="text-slate-400 hover:text-slate-200 cursor-pointer"
              @click="openPolicy(w)"
            >
              守护策略
            </button>
            <button
              type="button"
              class="text-slate-400 hover:text-slate-200 cursor-pointer"
              @click="openStderr(w)"
            >
              Stderr 日志
            </button>
            <button
              v-if="w.runtime_state === 'stopped' || w.runtime_state === 'crashed'"
              type="button"
              class="text-rose-400 hover:underline cursor-pointer"
              @click="askDelete(w)"
            >
              删除
            </button>
          </div>
        </div>
      </article>
    </div>

    <!-- Empty State -->
    <div v-else class="obs-card rounded-xl p-12 text-center">
      <Cpu class="w-9 h-9 text-slate-600 mx-auto mb-2.5" />
      <div class="text-sm font-medium text-slate-200">当前筛选下没有受管 Worker 实例</div>
      <div class="text-xs text-slate-500 mt-1 mb-4">
        可新建 DHT 爬虫抓取进程或 Manticore RT 批量写入索引进程
      </div>
      <button type="button" class="obs-btn obs-btn-primary px-4 py-2 text-xs inline-flex items-center gap-1.5" @click="openCreateModal('crawler')">
        <Plus class="w-3.5 h-3.5" />
        新建 Worker 实例
      </button>
    </div>

    <!-- Create Worker Modal -->
    <ObsModal v-model="createVisible" title="新建受管 Worker 子进程" subtitle="选择角色、网络端口或初始采集档位" max-width="560px">
      <div class="space-y-4 text-xs">
        <div class="grid grid-cols-2 gap-2">
          <button
            type="button"
            class="p-3 rounded-lg border text-left transition cursor-pointer"
            :class="createForm.role === 'crawler' ? 'bg-cyan-500/12 border-cyan-500/40 text-cyan-200' : 'obs-inset text-slate-400 hover:text-slate-200'"
            @click="createForm.role = 'crawler'; createForm.id = `crawler-${workers.filter((w) => w.role === 'crawler').length + 1}`; createForm.display_name = `DHT 爬虫实例 #${workers.filter((w) => w.role === 'crawler').length + 1}`"
          >
            <div class="font-semibold">DHT 爬虫 (Crawler)</div>
            <div class="text-[11px] text-slate-400 mt-0.5">UDP 路由嗅探 + BEP-9 元数据抓取</div>
          </button>
          <button
            type="button"
            class="p-3 rounded-lg border text-left transition cursor-pointer"
            :class="createForm.role === 'indexer' ? 'bg-indigo-500/12 border-indigo-500/40 text-indigo-200' : 'obs-inset text-slate-400 hover:text-slate-200'"
            @click="createForm.role = 'indexer'; createForm.id = `indexer-${workers.filter((w) => w.role === 'indexer').length + 1}`; createForm.display_name = `Manticore 索引器 #${workers.filter((w) => w.role === 'indexer').length + 1}`"
          >
            <div class="font-semibold">Manticore 索引器 (Indexer)</div>
            <div class="text-[11px] text-slate-400 mt-0.5">Redis 消费 + 过滤规则 + 批量入库</div>
          </button>
        </div>

        <div class="grid grid-cols-2 gap-3">
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">实例唯一 ID</label>
            <input v-model="createForm.id" type="text" class="obs-input px-3 py-1.5 text-xs font-mono-num" />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">显示名称</label>
            <input v-model="createForm.display_name" type="text" class="obs-input px-3 py-1.5 text-xs" />
          </div>
        </div>

        <template v-if="createForm.role === 'crawler'">
          <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">UDP 监听端口</label>
              <ObsNumberInput v-model="createForm.port" :min="1024" :max="65535" />
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">协议栈模式</label>
              <ObsSelect v-model="createForm.netmode" :options="NETMODE_OPTIONS" class="w-full" />
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">采集档位 (P1–P17)</label>
              <ObsSelect v-model="createForm.crawl_profile" :options="profileSelectOptions" mono class="w-full" />
            </div>
          </div>

          <div class="obs-inset p-3 rounded-lg grid grid-cols-4 gap-2 text-[11px] font-mono-num">
            <div>
              <span class="text-slate-500 block">目标 QPS</span>
              <span class="text-cyan-300">{{ selectedCreateProfileMeta.qps }}</span>
            </div>
            <div>
              <span class="text-slate-500 block">Peer Lookup</span>
              <span class="text-emerald-300">{{ selectedCreateProfileMeta.peerLookup }}/s</span>
            </div>
            <div>
              <span class="text-slate-500 block">Peer 池上限</span>
              <span class="text-slate-200">{{ selectedCreateProfileMeta.poolCap }}</span>
            </div>
            <div>
              <span class="text-slate-500 block">建议 ulimit -n</span>
              <span class="text-slate-200">{{ selectedCreateProfileMeta.recFd }}</span>
            </div>
          </div>

          <div
            v-if="systemSoftNofile !== null && systemSoftNofile < selectedCreateProfileMeta.recFd"
            class="px-3 py-2 rounded-lg bg-amber-500/10 border border-amber-500/30 text-[11px] text-amber-300 flex items-center gap-2"
          >
            <AlertTriangle class="w-4 h-4 shrink-0" />
            系统当前软限制 ({{ systemSoftNofile }}) 低于档位建议阈值 ({{ selectedCreateProfileMeta.recFd }})
          </div>
        </template>

        <template v-else>
          <div class="grid grid-cols-3 gap-3">
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">消费并发协程</label>
              <ObsNumberInput v-model="createForm.torrent_workers" :min="1" :max="512" />
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">写入并发协程</label>
              <ObsNumberInput v-model="createForm.insert_workers" :min="1" :max="512" />
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">单批条数 (Batch)</label>
              <ObsNumberInput v-model="createForm.batch_size" :min="10" :max="10000" />
            </div>
          </div>
        </template>

        <div class="obs-inset rounded-xl p-3.5 space-y-3">
          <div class="flex flex-wrap items-center justify-between gap-2 pb-2.5 border-b border-slate-800/75">
            <label class="inline-flex items-center gap-2 text-xs font-medium text-slate-200 cursor-pointer select-none">
              <input v-model="createForm.auto_restart" type="checkbox" class="obs-checkbox" />
              <span>崩溃自动拉起</span>
            </label>
            <span class="text-[11px] text-slate-500">异常退出后按退避间隔自动拉起</span>
          </div>
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">最大重启次数</label>
              <ObsNumberInput v-model="createForm.max_restarts" :min="0" :max="100" :disabled="!createForm.auto_restart" />
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">退避延迟 (秒)</label>
              <ObsNumberInput v-model="createForm.restart_backoff_secs" :min="1" :max="300" :disabled="!createForm.auto_restart" />
            </div>
          </div>
        </div>
      </div>

      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 text-xs" @click="createVisible = false">取消</button>
        <button type="button" class="obs-btn obs-btn-primary px-4 py-1.5 text-xs" :disabled="createSubmitting" @click="submitCreate">
          {{ createSubmitting ? "创建中…" : "确认创建" }}
        </button>
      </template>
    </ObsModal>

    <!-- Policy Modal -->
    <ObsModal v-model="policyVisible" :title="`守护策略 · ${policyTargetId}`" max-width="440px">
      <div class="space-y-3 text-xs">
        <div>
          <label class="block text-[11px] text-slate-400 mb-1">显示名称</label>
          <input v-model="policyForm.display_name" type="text" class="obs-input px-3 py-1.5 text-xs" />
        </div>
        <div class="obs-inset rounded-xl p-3.5 space-y-3">
          <div class="pb-2.5 border-b border-slate-800/75">
            <label class="inline-flex items-center gap-2 text-xs font-medium text-slate-200 cursor-pointer select-none">
              <input v-model="policyForm.auto_restart" type="checkbox" class="obs-checkbox" />
              <span>启用异常退出自动拉起</span>
            </label>
          </div>
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">最大重启次数</label>
              <ObsNumberInput v-model="policyForm.max_restarts" :min="0" :max="100" :disabled="!policyForm.auto_restart" />
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">退避间隔 (秒)</label>
              <ObsNumberInput v-model="policyForm.restart_backoff_secs" :min="1" :max="300" :disabled="!policyForm.auto_restart" />
            </div>
          </div>
        </div>
      </div>

      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 text-xs" @click="policyVisible = false">取消</button>
        <button type="button" class="obs-btn obs-btn-primary px-4 py-1.5 text-xs" :disabled="policySubmitting" @click="submitPolicy">
          {{ policySubmitting ? "保存中…" : "保存策略" }}
        </button>
      </template>
    </ObsModal>

    <!-- Stderr Modal -->
    <ObsModal v-model="stderrVisible" :title="`进程标准错误日志 (Stderr) · ${stderrTarget?.id ?? ''}`" max-width="760px">
      <pre class="obs-inset p-3 rounded-lg font-mono-num text-[11px] text-slate-300 max-h-96 overflow-auto obs-scroll whitespace-pre-wrap">{{ stderrTarget?.stderr_tail || "（暂无 Stderr 输出）" }}</pre>
      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 text-xs" @click="stderrVisible = false">关闭</button>
      </template>
    </ObsModal>

    <!-- Delete Confirm Modal -->
    <ObsModal v-model="deleteVisible" title="确认删除受管 Worker" max-width="420px">
      <div class="text-xs text-slate-300 space-y-2">
        <p>确认永久移除实例 <span class="font-mono-num font-semibold text-slate-100">{{ deleteTarget?.id }}</span> 及其配置历史记录？</p>
        <p class="text-[11px] text-amber-300">如进程正在运行，请先优雅停止后再删除。</p>
      </div>
      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 text-xs" @click="deleteVisible = false">取消</button>
        <button type="button" class="obs-btn obs-btn-danger px-4 py-1.5 text-xs" :disabled="deleteBusy" @click="confirmDelete">
          {{ deleteBusy ? "删除中…" : "确认删除" }}
        </button>
      </template>
    </ObsModal>

    <!-- L1/L2/L3 Configuration & Tuning Drawer -->
    <ObsDrawer v-model="tuneVisible" max-width="880px">
      <template #header>
        <div>
          <div class="flex items-center gap-2">
            <span class="px-2 py-0.5 rounded bg-emerald-500/15 border border-emerald-500/30 text-[11px] font-mono-num text-emerald-300">
              v{{ tuneVersion }}
            </span>
            <span class="text-xs font-mono-num text-slate-400">{{ tuneWorker?.id }}</span>
          </div>
          <h3 class="mt-1 text-sm font-semibold text-slate-100">
            {{ tuneWorker?.display_name }} · 参数调优与版本管理
          </h3>
        </div>
      </template>

      <div v-if="tuneLoading" class="py-16 text-center text-xs font-mono-num text-slate-400">
        正在读取当前激活配置…
      </div>

      <div v-else-if="tuneWorker" class="space-y-4">
        <!-- Drawer Sub-Tabs -->
        <div class="flex flex-wrap items-center gap-1 border-b border-slate-800/80">
          <template v-if="tuneWorker.role === 'crawler'">
            <button
              type="button"
              class="px-3 py-2 text-xs font-medium border-b-2 transition cursor-pointer"
              :class="tuneTab === 'l1' ? 'border-emerald-400 text-emerald-300' : 'border-transparent text-slate-400 hover:text-slate-200'"
              @click="selectTuneTab('l1')"
            >
              L1 基础与档位
            </button>
            <button
              type="button"
              class="px-3 py-2 text-xs font-medium border-b-2 transition cursor-pointer"
              :class="tuneTab === 'l2' ? 'border-emerald-400 text-emerald-300' : 'border-transparent text-slate-400 hover:text-slate-200'"
              @click="selectTuneTab('l2')"
            >
              L2 抓取与发现<span v-if="peerOvCount > 0" class="ml-1 text-amber-300">({{ peerOvCount }})</span>
            </button>
            <button
              type="button"
              class="px-3 py-2 text-xs font-medium border-b-2 transition cursor-pointer inline-flex items-center gap-1"
              :class="tuneTab === 'l3' ? 'border-emerald-400 text-emerald-300' : 'border-transparent text-slate-400 hover:text-slate-200'"
              @click="selectTuneTab('l3')"
            >
              L3 引擎细节
              <span v-if="l3OvCount > 0" class="px-1.5 py-0.2 rounded bg-amber-500/20 text-amber-300 font-mono-num text-[10px]">
                {{ l3OvCount }}
              </span>
            </button>
          </template>
          <template v-else>
            <button
              type="button"
              class="px-3 py-2 text-xs font-medium border-b-2 transition cursor-pointer"
              :class="tuneTab === 'l1' ? 'border-emerald-400 text-emerald-300' : 'border-transparent text-slate-400 hover:text-slate-200'"
              @click="selectTuneTab('l1')"
            >
              Indexer 并发与批写参数
            </button>
          </template>

          <button
            type="button"
            class="px-3 py-2 text-xs font-medium border-b-2 transition cursor-pointer"
            :class="tuneTab === 'json' ? 'border-emerald-400 text-emerald-300' : 'border-transparent text-slate-400 hover:text-slate-200'"
            @click="selectTuneTab('json')"
          >
            JSON 配置 / 导入导出
          </button>
          <button
            type="button"
            class="px-3 py-2 text-xs font-medium border-b-2 transition inline-flex items-center gap-1 cursor-pointer"
            :class="tuneTab === 'versions' ? 'border-emerald-400 text-emerald-300' : 'border-transparent text-slate-400 hover:text-slate-200'"
            @click="selectTuneTab('versions')"
          >
            <History class="w-3 h-3" />
            历史版本<span v-if="versions.length > 0"> ({{ versions.length }})</span>
          </button>
        </div>

        <!-- CRAWLER L1 -->
        <div v-if="tuneWorker.role === 'crawler' && tuneTab === 'l1'" class="space-y-4 text-xs">
          <div class="obs-inset rounded-xl p-4 space-y-3">
            <div class="flex items-center justify-between">
              <span class="font-semibold text-slate-200">L1 基础运行与并发参数</span>
              <span class="font-mono-num text-[11px] text-slate-500">修改核心端口/档位将平滑重建子进程</span>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
              <div>
                <label class="text-slate-400 block mb-1">UDP 监听端口 (port)</label>
                <ObsNumberInput v-model="crawlerCfg.port" :min="1" :max="65535" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">IP 协议模式 (netmode)</label>
                <ObsSelect v-model="crawlerCfg.netmode" :options="NETMODE_OPTIONS" mono class="w-full" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">预设档位 (crawl_profile)</label>
                <ObsSelect v-model="crawlerCfg.crawl_profile" :options="profileSelectOptions" mono class="w-full" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">日志级别 (log_level)</label>
                <ObsSelect v-model="crawlerCfg.log_level" :options="LOG_LEVEL_OPTIONS" mono class="w-full" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">本地去重缓存容量</label>
                <ObsNumberInput v-model="crawlerCfg.metadata_cache_capacity" :min="1000" :step="10000" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">Redis 推送并发数</label>
                <ObsNumberInput v-model="crawlerCfg.push_concurrency" :min="1" :max="256" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">Hash 入口队列容量</label>
                <ObsNumberInput v-model="crawlerCfg.hash_queue_capacity" :min="64" :step="512" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">Pending 锁 TTL (秒)</label>
                <ObsNumberInput v-model="crawlerCfg.metadata_pending_ttl_secs" :min="10" :step="60" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">失败退避 TTL (秒)</label>
                <ObsNumberInput v-model="crawlerCfg.metadata_failure_backoff_secs" :min="1" :step="10" />
              </div>
            </div>
          </div>

          <div
            v-if="tuneNofileWarn"
            class="px-3 py-2 rounded-lg bg-amber-500/10 border border-amber-500/30 text-[11px] text-amber-300 flex items-center gap-2"
          >
            <AlertTriangle class="w-4 h-4 shrink-0" />
            {{ tuneNofileWarn }}
          </div>

          <!-- P1 ~ P17 完整档位参数对照表 -->
          <div class="obs-inset rounded-xl p-3.5 space-y-2.5">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="font-semibold text-slate-200">
                P1 ~ P17 采集档位参数对照表
                <span class="font-mono-num text-emerald-400 ml-1">(当前: {{ crawlerCfg.crawl_profile }})</span>
              </div>
              <button
                type="button"
                class="text-cyan-400 hover:underline cursor-pointer"
                @click="showProfilesTable = !showProfilesTable"
              >
                {{ showProfilesTable ? "收起对照表" : "展开对照表" }}
              </button>
            </div>

            <div class="grid grid-cols-3 sm:grid-cols-6 gap-2 font-mono-num text-[11px]">
              <div><span class="text-slate-500 block">find_node/s</span><span class="text-cyan-300">{{ currentTuneDefaults.find_node_rate_per_sec ?? "—" }}</span></div>
              <div><span class="text-slate-500 block">Burst</span><span class="text-slate-200">{{ currentTuneDefaults.find_node_burst ?? "—" }}</span></div>
              <div><span class="text-slate-500 block">InFlight</span><span class="text-slate-200">{{ currentTuneDefaults.find_node_max_in_flight ?? "—" }}</span></div>
              <div><span class="text-slate-500 block">PeerLookup</span><span class="text-emerald-300">{{ currentTuneDefaults.peer_lookup_rate_per_sec ?? "—" }}/s</span></div>
              <div><span class="text-slate-500 block">Pool Cap</span><span class="text-slate-200">{{ currentTuneDefaults.pool_capacity ?? "—" }}</span></div>
              <div><span class="text-slate-500 block">Resp Cap</span><span class="text-slate-200">{{ currentTuneDefaults.responsive_capacity ?? "—" }}</span></div>
            </div>

            <div v-if="showProfilesTable" class="overflow-x-auto max-h-64 obs-scroll rounded-lg border border-slate-800/80">
              <table class="w-full text-left font-mono-num text-[11px]">
                <thead class="sticky top-0 bg-[#080a0f] border-b border-slate-800 text-slate-500">
                  <tr>
                    <th class="py-2 px-2.5">档位</th>
                    <th class="py-2 px-2 font-sans">定位</th>
                    <th class="py-2 px-2">QPS/s</th>
                    <th class="py-2 px-2">InFlight</th>
                    <th class="py-2 px-2">PeerLookup</th>
                    <th class="py-2 px-2">路由 Peer 池</th>
                    <th class="py-2 px-2">建议 FD</th>
                    <th class="py-2 px-2.5 text-right">操作</th>
                  </tr>
                </thead>
                <tbody class="divide-y divide-slate-800/60">
                  <tr
                    v-for="p in profileTableRows"
                    :key="p.name"
                    :class="p.name === crawlerCfg.crawl_profile ? 'bg-emerald-500/12 text-emerald-200' : 'hover:bg-slate-800/35 text-slate-300'"
                  >
                    <td class="py-2 px-2.5 font-semibold">{{ p.name }}</td>
                    <td class="py-2 px-2 font-sans text-slate-400">{{ p.level }}</td>
                    <td class="py-2 px-2" :title="fmtExact(p.qps)">{{ fmtCount(p.qps) }}</td>
                    <td class="py-2 px-2" :title="fmtExact(p.inFlight)">{{ fmtCount(p.inFlight) }}</td>
                    <td class="py-2 px-2">{{ fmtCount(p.peerLookup) }}/s</td>
                    <td class="py-2 px-2" :title="fmtExact(p.poolCap)">{{ fmtCount(p.poolCap) }}</td>
                    <td
                      class="py-2 px-2"
                      :class="systemSoftNofile !== null && systemSoftNofile < p.recFd ? 'text-amber-300' : 'text-slate-400'"
                      :title="fmtExact(p.recFd)"
                    >
                      {{ fmtCount(p.recFd) }}<template v-if="systemSoftNofile !== null && systemSoftNofile < p.recFd"> ⚠</template>
                    </td>
                    <td class="py-2 px-2.5 text-right">
                      <span v-if="p.name === crawlerCfg.crawl_profile" class="text-emerald-400 font-semibold">已选</span>
                      <button
                        v-else
                        type="button"
                        class="text-cyan-400 hover:underline cursor-pointer"
                        @click="crawlerCfg.crawl_profile = p.name"
                      >
                        切换
                      </button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- CRAWLER L2 -->
        <div v-else-if="tuneWorker.role === 'crawler' && tuneTab === 'l2'" class="space-y-4 text-xs">
          <!-- Metadata BEP-9 (11项) -->
          <div class="obs-inset rounded-xl p-4 space-y-3">
            <div class="flex items-center justify-between">
              <span class="font-semibold text-slate-200">Metadata BEP-9 元数据抓取 (11 项精细参数)</span>
              <span class="font-mono-num text-[11px] text-cyan-400">TCP / ut_metadata</span>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
              <div>
                <label class="text-slate-400 block mb-1">抓取超时 (timeout_secs)</label>
                <ObsNumberInput v-model="crawlerCfg.metadata.timeout_secs" :min="1" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">TCP 连接超时 (ms)</label>
                <ObsNumberInput v-model="crawlerCfg.metadata.connect_timeout_ms" :min="100" :step="100" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">最大并发 Worker 数</label>
                <ObsNumberInput v-model="crawlerCfg.metadata.max_worker_count" :min="1" :max="4096" :step="32" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">任务队列容量</label>
                <ObsNumberInput v-model="crawlerCfg.metadata.max_queue_size" :min="64" :step="500" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">Peer 失败缓存容量</label>
                <ObsNumberInput v-model="crawlerCfg.metadata.peer_failure_cache_capacity" :min="100" :step="1000" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">Peer 失败缓存 TTL(s)</label>
                <ObsNumberInput v-model="crawlerCfg.metadata.peer_failure_ttl_secs" :min="1" :step="5" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">首次重试延迟(s)</label>
                <ObsNumberInput v-model="crawlerCfg.metadata.retry_initial_delay_secs" :min="1" :step="5" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">最大重试延迟(s)</label>
                <ObsNumberInput v-model="crawlerCfg.metadata.retry_max_delay_secs" :min="1" :step="60" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">最大重试次数</label>
                <ObsNumberInput v-model="crawlerCfg.metadata.retry_max_attempts" :min="0" :max="20" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">重试历史 TTL(s)</label>
                <ObsNumberInput v-model="crawlerCfg.metadata.retry_history_ttl_secs" :min="10" :step="300" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1">重试历史容量</label>
                <ObsNumberInput v-model="crawlerCfg.metadata.retry_history_capacity" :min="100" :step="1000" />
              </div>
            </div>
          </div>

          <!-- Discovery BEP-51 (15项) -->
          <div class="obs-inset rounded-xl p-4 space-y-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <span class="font-semibold text-slate-200">Discovery 主动采样发现 (BEP-51 · 15 项)</span>
              <label class="inline-flex items-center gap-2 cursor-pointer">
                <input v-model="crawlerCfg.discovery.enabled" type="checkbox" class="obs-checkbox" />
                <span class="font-mono-num" :class="crawlerCfg.discovery.enabled ? 'text-emerald-300' : 'text-slate-500'">
                  {{ crawlerCfg.discovery.enabled ? "已启用" : "已停用" }}
                </span>
              </label>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3" :class="crawlerCfg.discovery.enabled ? '' : 'opacity-45 pointer-events-none'">
              <div><label class="text-slate-400 block mb-1">sample 查询速率 (/s)</label><ObsNumberInput v-model="crawlerCfg.discovery.sample_queries_per_second" :min="1" /></div>
              <div><label class="text-slate-400 block mb-1">sample 突发桶</label><ObsNumberInput v-model="crawlerCfg.discovery.sample_burst" :min="1" /></div>
              <div><label class="text-slate-400 block mb-1">sample 最大并发</label><ObsNumberInput v-model="crawlerCfg.discovery.sample_max_in_flight" :min="1" /></div>
              <div><label class="text-slate-400 block mb-1">sample 超时(s)</label><ObsNumberInput v-model="crawlerCfg.discovery.sample_query_timeout_secs" :min="1" /></div>
              <div><label class="text-slate-400 block mb-1">最小采样间隔(s)</label><ObsNumberInput v-model="crawlerCfg.discovery.sample_min_interval_secs" :min="1" :step="10" /></div>
              <div><label class="text-slate-400 block mb-1">失败冷却退避(s)</label><ObsNumberInput v-model="crawlerCfg.discovery.sample_failure_backoff_secs" :min="1" :step="30" /></div>
              <div><label class="text-slate-400 block mb-1">无新Hash退避(s)</label><ObsNumberInput v-model="crawlerCfg.discovery.sample_no_new_backoff_secs" :min="1" :step="60" /></div>
              <div><label class="text-slate-400 block mb-1">最大跟踪 Peer 数</label><ObsNumberInput v-model="crawlerCfg.discovery.sample_max_nodes" :min="1" :step="256" /></div>
              <div><label class="text-slate-400 block mb-1">冷却表容量</label><ObsNumberInput v-model="crawlerCfg.discovery.sample_cooldown_capacity" :min="100" :step="1000" /></div>
              <div><label class="text-slate-400 block mb-1">Hash 去重表容量</label><ObsNumberInput v-model="crawlerCfg.discovery.hash_dedup_capacity" :min="100" :step="1000" /></div>
              <div><label class="text-slate-400 block mb-1">Hash 去重 TTL(s)</label><ObsNumberInput v-model="crawlerCfg.discovery.hash_dedup_ttl_secs" :min="1" :step="30" /></div>
              <div><label class="text-slate-400 block mb-1">get_peers Hash 限速</label><ObsNumberInput v-model="crawlerCfg.discovery.get_peers_hashes_per_second" :min="1" /></div>
              <div><label class="text-slate-400 block mb-1">get_peers 突发</label><ObsNumberInput v-model="crawlerCfg.discovery.get_peers_burst" :min="1" /></div>
              <div><label class="text-slate-400 block mb-1">sample Hash 限速</label><ObsNumberInput v-model="crawlerCfg.discovery.sample_hashes_per_second" :min="1" /></div>
              <div><label class="text-slate-400 block mb-1">sample Hash 突发</label><ObsNumberInput v-model="crawlerCfg.discovery.sample_hash_burst" :min="1" /></div>
            </div>
          </div>

          <!-- PeerLookup 覆写 (7项) -->
          <div class="obs-inset rounded-xl p-4 space-y-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="flex items-center gap-2">
                <span class="font-semibold text-slate-200">PeerLookup 覆写 (可选 · 7 项)</span>
                <span v-if="peerOvCount > 0" class="px-1.5 py-0.5 rounded bg-amber-500/20 text-amber-300 font-mono-num text-[10px]">
                  已覆写 {{ peerOvCount }} 项
                </span>
              </div>
              <button type="button" class="text-cyan-400 hover:underline cursor-pointer" @click="clearTuningGroup('peer_lookup')">
                全部恢复档位默认
              </button>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.peer_lookup.max_lookups_per_second) ? 'text-amber-300 font-medium' : 'text-slate-400'">lookup 速率 (/s)</label>
                  <button v-if="isSet(crawlerCfg.peer_lookup.max_lookups_per_second)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.peer_lookup.max_lookups_per_second = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.peer_lookup.max_lookups_per_second" allow-empty :min="1" :placeholder="`默认 ${currentTuneDefaults.peer_lookup_rate_per_sec ?? ''}`" :input-class="isSet(crawlerCfg.peer_lookup.max_lookups_per_second) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.peer_lookup.burst) ? 'text-amber-300 font-medium' : 'text-slate-400'">lookup 突发桶</label>
                  <button v-if="isSet(crawlerCfg.peer_lookup.burst)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.peer_lookup.burst = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.peer_lookup.burst" allow-empty :min="1" :placeholder="`默认 ${currentTuneDefaults.peer_lookup_burst ?? ''}`" :input-class="isSet(crawlerCfg.peer_lookup.burst) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.peer_lookup.max_active_lookups) ? 'text-amber-300 font-medium' : 'text-slate-400'">最大活跃 lookup 数</label>
                  <button v-if="isSet(crawlerCfg.peer_lookup.max_active_lookups)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.peer_lookup.max_active_lookups = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.peer_lookup.max_active_lookups" allow-empty :min="1" :placeholder="`默认 ${currentTuneDefaults.peer_lookup_max_active ?? ''}`" :input-class="isSet(crawlerCfg.peer_lookup.max_active_lookups) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.peer_lookup.max_queries) ? 'text-amber-300 font-medium' : 'text-slate-400'">单次最大查询次数</label>
                  <button v-if="isSet(crawlerCfg.peer_lookup.max_queries)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.peer_lookup.max_queries = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.peer_lookup.max_queries" allow-empty :min="1" placeholder="引擎默认" :input-class="isSet(crawlerCfg.peer_lookup.max_queries) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.peer_lookup.max_concurrent_queries) ? 'text-amber-300 font-medium' : 'text-slate-400'">单lookup并发查询数</label>
                  <button v-if="isSet(crawlerCfg.peer_lookup.max_concurrent_queries)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.peer_lookup.max_concurrent_queries = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.peer_lookup.max_concurrent_queries" allow-empty :min="1" placeholder="引擎默认" :input-class="isSet(crawlerCfg.peer_lookup.max_concurrent_queries) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.peer_lookup.lookup_timeout_ms) ? 'text-amber-300 font-medium' : 'text-slate-400'">lookup 总超时 (ms)</label>
                  <button v-if="isSet(crawlerCfg.peer_lookup.lookup_timeout_ms)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.peer_lookup.lookup_timeout_ms = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.peer_lookup.lookup_timeout_ms" allow-empty :min="100" :step="100" placeholder="引擎默认" :input-class="isSet(crawlerCfg.peer_lookup.lookup_timeout_ms) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.peer_lookup.query_timeout_ms) ? 'text-amber-300 font-medium' : 'text-slate-400'">单query超时 (ms)</label>
                  <button v-if="isSet(crawlerCfg.peer_lookup.query_timeout_ms)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.peer_lookup.query_timeout_ms = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.peer_lookup.query_timeout_ms" allow-empty :min="50" :step="50" placeholder="引擎默认" :input-class="isSet(crawlerCfg.peer_lookup.query_timeout_ms) ? 'border-amber-500/45!' : ''" />
              </div>
            </div>
          </div>
        </div>

        <!-- CRAWLER L3 -->
        <div v-else-if="tuneWorker.role === 'crawler' && tuneTab === 'l3'" class="space-y-4 text-xs">
          <div class="rounded-lg border border-slate-800 bg-[#080a0f] px-3.5 py-2 text-slate-400">
            留空字段将自动继承当前档位 (<strong class="text-emerald-300 font-mono-num">{{ crawlerCfg.crawl_profile }}</strong>) 默认值；填入数值即转为显式覆写（高亮橙色），点击「清除」可随时恢复继承。
          </div>

          <!-- crawl.rate_limit -->
          <div class="obs-inset rounded-xl p-4 space-y-3">
            <div class="flex items-center justify-between">
              <span class="font-semibold text-slate-200 font-mono-num">
                crawl.rate_limit <span class="font-sans text-slate-400 font-normal">(UDP 发包与回包限流)</span>
              </span>
              <button type="button" class="text-cyan-400 hover:underline cursor-pointer" @click="clearTuningGroup('rate_limit')">清空本组覆写</button>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.rate_limit.max_find_node_rate_per_sec) ? 'text-amber-300 font-medium' : 'text-slate-400'">find_node QPS</label>
                  <button v-if="isSet(crawlerCfg.crawl.rate_limit.max_find_node_rate_per_sec)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.rate_limit.max_find_node_rate_per_sec = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.rate_limit.max_find_node_rate_per_sec" allow-empty :min="1" :placeholder="`默认 ${currentTuneDefaults.find_node_rate_per_sec ?? ''}`" :input-class="isSet(crawlerCfg.crawl.rate_limit.max_find_node_rate_per_sec) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.rate_limit.burst) ? 'text-amber-300 font-medium' : 'text-slate-400'">find_node Burst</label>
                  <button v-if="isSet(crawlerCfg.crawl.rate_limit.burst)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.rate_limit.burst = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.rate_limit.burst" allow-empty :min="1" :placeholder="`默认 ${currentTuneDefaults.find_node_burst ?? ''}`" :input-class="isSet(crawlerCfg.crawl.rate_limit.burst) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.rate_limit.max_in_flight) ? 'text-amber-300 font-medium' : 'text-slate-400'">max_in_flight</label>
                  <button v-if="isSet(crawlerCfg.crawl.rate_limit.max_in_flight)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.rate_limit.max_in_flight = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.rate_limit.max_in_flight" allow-empty :min="1" :placeholder="`默认 ${currentTuneDefaults.find_node_max_in_flight ?? ''}`" :input-class="isSet(crawlerCfg.crawl.rate_limit.max_in_flight) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.rate_limit.request_timeout_secs) ? 'text-amber-300 font-medium' : 'text-slate-400'">请求超时 (secs)</label>
                  <button v-if="isSet(crawlerCfg.crawl.rate_limit.request_timeout_secs)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.rate_limit.request_timeout_secs = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.rate_limit.request_timeout_secs" allow-empty :min="1" placeholder="默认 2" :input-class="isSet(crawlerCfg.crawl.rate_limit.request_timeout_secs) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.rate_limit.max_new_destinations_per_minute) ? 'text-amber-300 font-medium' : 'text-slate-400'">每分钟新目标上限</label>
                  <button v-if="isSet(crawlerCfg.crawl.rate_limit.max_new_destinations_per_minute)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.rate_limit.max_new_destinations_per_minute = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.rate_limit.max_new_destinations_per_minute" allow-empty :min="1" :placeholder="`默认 ${currentTuneDefaults.max_new_destinations_per_minute ?? ''}`" :input-class="isSet(crawlerCfg.crawl.rate_limit.max_new_destinations_per_minute) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.rate_limit.max_response_rate_per_sec) ? 'text-amber-300 font-medium' : 'text-slate-400'">最大回包速率 (/s)</label>
                  <button v-if="isSet(crawlerCfg.crawl.rate_limit.max_response_rate_per_sec)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.rate_limit.max_response_rate_per_sec = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.rate_limit.max_response_rate_per_sec" allow-empty :min="1" :placeholder="`默认 ${currentTuneDefaults.max_response_rate_per_sec ?? ''}`" :input-class="isSet(crawlerCfg.crawl.rate_limit.max_response_rate_per_sec) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.rate_limit.max_response_bytes_per_sec) ? 'text-amber-300 font-medium' : 'text-slate-400'">最大回包带宽 (B/s)</label>
                  <button v-if="isSet(crawlerCfg.crawl.rate_limit.max_response_bytes_per_sec)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.rate_limit.max_response_bytes_per_sec = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.rate_limit.max_response_bytes_per_sec" allow-empty :min="1024" :step="1024" :placeholder="`默认 ${fmtCount(currentTuneDefaults.max_response_bytes_per_sec)}`" :input-class="isSet(crawlerCfg.crawl.rate_limit.max_response_bytes_per_sec) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.rate_limit.max_response_rate_per_source) ? 'text-amber-300 font-medium' : 'text-slate-400'">单源回包限速 (/s)</label>
                  <button v-if="isSet(crawlerCfg.crawl.rate_limit.max_response_rate_per_source)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.rate_limit.max_response_rate_per_source = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.rate_limit.max_response_rate_per_source" allow-empty :min="1" :placeholder="`默认 ${currentTuneDefaults.max_response_rate_per_source ?? ''}`" :input-class="isSet(crawlerCfg.crawl.rate_limit.max_response_rate_per_source) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.rate_limit.metadata_pressure_floor_percent) ? 'text-amber-300 font-medium' : 'text-slate-400'">元数据降速下限 (%)</label>
                  <button v-if="isSet(crawlerCfg.crawl.rate_limit.metadata_pressure_floor_percent)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.rate_limit.metadata_pressure_floor_percent = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.rate_limit.metadata_pressure_floor_percent" allow-empty :min="1" :max="100" placeholder="25" :input-class="isSet(crawlerCfg.crawl.rate_limit.metadata_pressure_floor_percent) ? 'border-amber-500/45!' : ''" />
              </div>
            </div>
          </div>

          <!-- crawl.pool -->
          <div class="obs-inset rounded-xl p-4 space-y-3">
            <div class="flex items-center justify-between">
              <span class="font-semibold text-slate-200 font-mono-num">
                crawl.pool <span class="font-sans text-slate-400 font-normal">(DHT 路由 Peer 池)</span>
              </span>
              <button type="button" class="text-cyan-400 hover:underline cursor-pointer" @click="clearTuningGroup('pool')">清空本组覆写</button>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.pool.capacity) ? 'text-amber-300 font-medium' : 'text-slate-400'">候选 Peer 容量</label>
                  <button v-if="isSet(crawlerCfg.crawl.pool.capacity)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.pool.capacity = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.pool.capacity" allow-empty :min="128" :placeholder="`默认 ${currentTuneDefaults.pool_capacity ?? ''}`" :input-class="isSet(crawlerCfg.crawl.pool.capacity) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.pool.responsive_capacity) ? 'text-amber-300 font-medium' : 'text-slate-400'">高响应 Peer 容量</label>
                  <button v-if="isSet(crawlerCfg.crawl.pool.responsive_capacity)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.pool.responsive_capacity = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.pool.responsive_capacity" allow-empty :min="64" :placeholder="`默认 ${currentTuneDefaults.responsive_capacity ?? ''}`" :input-class="isSet(crawlerCfg.crawl.pool.responsive_capacity) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.pool.responsive_ttl_secs) ? 'text-amber-300 font-medium' : 'text-slate-400'">活跃 Peer TTL (秒)</label>
                  <button v-if="isSet(crawlerCfg.crawl.pool.responsive_ttl_secs)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.pool.responsive_ttl_secs = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.pool.responsive_ttl_secs" allow-empty :min="30" placeholder="900" :input-class="isSet(crawlerCfg.crawl.pool.responsive_ttl_secs) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.pool.revisit_interval_secs) ? 'text-amber-300 font-medium' : 'text-slate-400'">重访冷却 (秒)</label>
                  <button v-if="isSet(crawlerCfg.crawl.pool.revisit_interval_secs)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.pool.revisit_interval_secs = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.pool.revisit_interval_secs" allow-empty :min="5" placeholder="60" :input-class="isSet(crawlerCfg.crawl.pool.revisit_interval_secs) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.pool.low_watermark) ? 'text-amber-300 font-medium' : 'text-slate-400'">低水位补种阈值</label>
                  <button v-if="isSet(crawlerCfg.crawl.pool.low_watermark)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.pool.low_watermark = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.pool.low_watermark" allow-empty :min="1" placeholder="256" :input-class="isSet(crawlerCfg.crawl.pool.low_watermark) ? 'border-amber-500/45!' : ''" />
              </div>
            </div>
          </div>

          <!-- crawl.target & scheduler -->
          <div class="obs-inset rounded-xl p-4 space-y-3">
            <div class="flex items-center justify-between">
              <span class="font-semibold text-slate-200 font-mono-num">
                crawl.target / scheduler <span class="font-sans text-slate-400 font-normal">(调度器与目标策略)</span>
              </span>
              <button type="button" class="text-cyan-400 hover:underline cursor-pointer" @click="clearTuningGroup('target'); clearTuningGroup('scheduler')">清空本组覆写</button>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.target.random_walk_percent) ? 'text-amber-300 font-medium' : 'text-slate-400'">随机漫步比重 (%)</label>
                  <button v-if="isSet(crawlerCfg.crawl.target.random_walk_percent)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.target.random_walk_percent = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.target.random_walk_percent" allow-empty :min="0" :max="100" placeholder="40" :input-class="isSet(crawlerCfg.crawl.target.random_walk_percent) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.target.sparse_bucket_percent) ? 'text-amber-300 font-medium' : 'text-slate-400'">稀疏桶比重 (%)</label>
                  <button v-if="isSet(crawlerCfg.crawl.target.sparse_bucket_percent)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.target.sparse_bucket_percent = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.target.sparse_bucket_percent" allow-empty :min="0" :max="100" placeholder="40" :input-class="isSet(crawlerCfg.crawl.target.sparse_bucket_percent) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.scheduler.routing_snapshot_size) ? 'text-amber-300 font-medium' : 'text-slate-400'">路由快照尺寸</label>
                  <button v-if="isSet(crawlerCfg.crawl.scheduler.routing_snapshot_size)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.scheduler.routing_snapshot_size = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.scheduler.routing_snapshot_size" allow-empty :min="16" :placeholder="`默认 ${currentTuneDefaults.routing_snapshot_size ?? ''}`" :input-class="isSet(crawlerCfg.crawl.scheduler.routing_snapshot_size) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.scheduler.snapshot_refresh_millis) ? 'text-amber-300 font-medium' : 'text-slate-400'">快照刷新周期 (ms)</label>
                  <button v-if="isSet(crawlerCfg.crawl.scheduler.snapshot_refresh_millis)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.scheduler.snapshot_refresh_millis = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.scheduler.snapshot_refresh_millis" allow-empty :min="50" placeholder="500" :input-class="isSet(crawlerCfg.crawl.scheduler.snapshot_refresh_millis) ? 'border-amber-500/45!' : ''" />
              </div>
              <label class="inline-flex items-center gap-2 text-xs text-slate-300 cursor-pointer pt-5">
                <input v-model="crawlerCfg.crawl.target.neighbor_sender_id" type="checkbox" class="obs-checkbox" />
                启用邻近 Sender ID 伪装
              </label>
            </div>
          </div>

          <!-- crawl.bootstrap -->
          <div class="obs-inset rounded-xl p-4 space-y-3">
            <div class="flex items-center justify-between">
              <span class="font-semibold text-slate-200 font-mono-num">
                crawl.bootstrap <span class="font-sans text-slate-400 font-normal">(引导入口列表)</span>
              </span>
              <button type="button" class="text-cyan-400 hover:underline cursor-pointer" @click="clearTuningGroup('bootstrap')">清空本组覆写</button>
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">引导域名 / IP:Port 列表（每行一条，留空使用内置公网列表）</label>
              <textarea
                v-model="bootstrapNodesText"
                rows="4"
                placeholder="router.bittorrent.com:6881&#10;dht.transmissionbt.com:6881&#10;router.utorrent.com:6881"
                class="obs-input p-2.5 text-xs font-mono-num"
              ></textarea>
            </div>
            <div class="grid grid-cols-2 sm:grid-cols-4 gap-3">
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.bootstrap.interval_secs) ? 'text-amber-300 font-medium' : 'text-slate-400'">重引导周期 (秒)</label>
                  <button v-if="isSet(crawlerCfg.crawl.bootstrap.interval_secs)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.bootstrap.interval_secs = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.bootstrap.interval_secs" allow-empty :min="5" placeholder="60" :input-class="isSet(crawlerCfg.crawl.bootstrap.interval_secs) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.bootstrap.max_nodes_per_round) ? 'text-amber-300 font-medium' : 'text-slate-400'">每轮最大地址数</label>
                  <button v-if="isSet(crawlerCfg.crawl.bootstrap.max_nodes_per_round)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.bootstrap.max_nodes_per_round = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.bootstrap.max_nodes_per_round" allow-empty :min="1" placeholder="16" :input-class="isSet(crawlerCfg.crawl.bootstrap.max_nodes_per_round) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.bootstrap.source_backoff_base_secs) ? 'text-amber-300 font-medium' : 'text-slate-400'">源退避基准 (秒)</label>
                  <button v-if="isSet(crawlerCfg.crawl.bootstrap.source_backoff_base_secs)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.bootstrap.source_backoff_base_secs = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.bootstrap.source_backoff_base_secs" allow-empty :min="1" placeholder="30" :input-class="isSet(crawlerCfg.crawl.bootstrap.source_backoff_base_secs) ? 'border-amber-500/45!' : ''" />
              </div>
              <div>
                <div class="flex items-center justify-between mb-1 gap-1">
                  <label class="truncate" :class="isSet(crawlerCfg.crawl.bootstrap.source_backoff_max_secs) ? 'text-amber-300 font-medium' : 'text-slate-400'">最大退避 (秒)</label>
                  <button v-if="isSet(crawlerCfg.crawl.bootstrap.source_backoff_max_secs)" type="button" class="text-[10px] text-amber-400 hover:underline shrink-0 cursor-pointer" @click="crawlerCfg.crawl.bootstrap.source_backoff_max_secs = undefined">清除</button>
                  <span v-else class="text-[10px] text-slate-600 shrink-0">继承</span>
                </div>
                <ObsNumberInput v-model="crawlerCfg.crawl.bootstrap.source_backoff_max_secs" allow-empty :min="1" placeholder="600" :input-class="isSet(crawlerCfg.crawl.bootstrap.source_backoff_max_secs) ? 'border-amber-500/45!' : ''" />
              </div>
            </div>
          </div>
        </div>

        <!-- INDEXER CONFIG -->
        <div v-else-if="tuneWorker.role === 'indexer' && tuneTab === 'l1'" class="space-y-4 text-xs">
          <div class="obs-inset rounded-xl p-4 grid grid-cols-2 sm:grid-cols-3 gap-3">
            <div><label class="block text-[11px] text-slate-400 mb-1">Torrent 过滤协程数</label><ObsNumberInput v-model="indexerCfg.torrent_workers" :min="1" :max="512" /></div>
            <div><label class="block text-[11px] text-slate-400 mb-1">热度更新协程数</label><ObsNumberInput v-model="indexerCfg.hot_workers" :min="1" :max="128" /></div>
            <div><label class="block text-[11px] text-slate-400 mb-1">Manticore 写入并发</label><ObsNumberInput v-model="indexerCfg.insert_workers" :min="1" :max="512" /></div>
            <div><label class="block text-[11px] text-slate-400 mb-1">单次 XREADGROUP 批量</label><ObsNumberInput v-model="indexerCfg.read_batch_size" :min="1" :max="4096" /></div>
            <div><label class="block text-[11px] text-slate-400 mb-1">XREAD 阻塞超时 (ms)</label><ObsNumberInput v-model="indexerCfg.read_block_ms" :min="100" /></div>
            <div><label class="block text-[11px] text-slate-400 mb-1">Manticore 单批写入量</label><ObsNumberInput v-model="indexerCfg.batch_size" :min="1" :max="10000" /></div>
            <div><label class="block text-[11px] text-slate-400 mb-1">攒批冲洗间隔 (ms)</label><ObsNumberInput v-model="indexerCfg.flush_interval_ms" :min="100" /></div>
            <div><label class="block text-[11px] text-slate-400 mb-1">New 通道缓冲大小</label><ObsNumberInput v-model="indexerCfg.channel_buffer_size" :min="64" /></div>
            <div><label class="block text-[11px] text-slate-400 mb-1">Hot 通道缓冲大小</label><ObsNumberInput v-model="indexerCfg.hot_channel_buffer_size" :min="64" /></div>
            <div><label class="block text-[11px] text-slate-400 mb-1">XAUTOCLAIM 周期 (秒)</label><ObsNumberInput v-model="indexerCfg.xautoclaim_interval_secs" :min="5" /></div>
            <div><label class="block text-[11px] text-slate-400 mb-1">最小挂起空闲 (ms)</label><ObsNumberInput v-model="indexerCfg.xautoclaim_min_idle_ms" :min="1000" /></div>
            <div><label class="block text-[11px] text-slate-400 mb-1">毒丸最大投递次数</label><ObsNumberInput v-model="indexerCfg.max_delivery_attempts" :min="1" /></div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">日志级别</label>
              <ObsSelect v-model="indexerCfg.log_level" :options="LOG_LEVEL_OPTIONS" mono class="w-full" />
            </div>
          </div>
        </div>

        <!-- JSON TAB -->
        <div v-else-if="tuneTab === 'json'" class="space-y-2.5 text-xs">
          <div class="flex items-center justify-between text-[11px] text-slate-400">
            <span>直接编辑持久化 JSON（提交前将通过 strict schema 校验）</span>
            <button type="button" class="obs-btn obs-btn-ghost px-2 py-1 text-[11px]" @click="syncJsonPreview">
              从表单重新生成
            </button>
          </div>
          <textarea
            v-model="rawJsonText"
            rows="20"
            class="obs-input p-3 font-mono-num text-[11px] leading-relaxed obs-scroll"
          ></textarea>
          <div v-if="rawJsonError" class="text-[11px] font-mono-num text-rose-300">
            {{ rawJsonError }}
          </div>
        </div>

        <!-- VERSIONS TAB -->
        <div v-else-if="tuneTab === 'versions'" class="space-y-2.5">
          <div v-if="versionsLoading" class="py-12 text-center text-xs font-mono-num text-slate-400">
            正在查询配置版本审计记录…
          </div>
          <div v-else-if="versions.length > 0" class="space-y-2">
            <div
              v-for="v in versions"
              :key="v.id"
              class="obs-inset p-3 rounded-lg flex flex-wrap items-center justify-between gap-2 text-xs"
            >
              <div class="space-y-0.5 min-w-0">
                <div class="flex items-center gap-2">
                  <span class="font-mono-num font-semibold text-emerald-300">v{{ v.version }}</span>
                  <span class="text-slate-200 truncate">{{ v.note || "未填写变更说明" }}</span>
                </div>
                <div class="text-[11px] font-mono-num text-slate-500">
                  {{ v.created_by }} · {{ formatDateTime(v.created_at) }}
                </div>
              </div>
              <button
                type="button"
                class="obs-btn obs-btn-ghost px-2.5 py-1 text-xs"
                :disabled="rollbackBusyVer === v.version"
                @click="handleRollback(v.version)"
              >
                {{ rollbackBusyVer === v.version ? "回滚中…" : "回滚至此版本" }}
              </button>
            </div>
          </div>
          <div v-else class="py-12 text-center text-xs text-slate-500">暂无配置版本记录</div>
        </div>
      </div>

      <template #footer>
        <div v-if="tuneTab !== 'versions'" class="w-full flex flex-wrap items-center justify-between gap-3">
          <div class="flex flex-wrap items-center gap-3 flex-1 min-w-[220px]">
            <input
              v-model="tuneNote"
              type="text"
              placeholder="版本备注（可选，例如：调优 P8 与 Redis 并发）"
              class="obs-input px-3 py-1.5 text-xs max-w-xs"
            />
            <label
              v-if="tuneWorker?.runtime_state === 'running' || tuneWorker?.runtime_state === 'draining'"
              class="inline-flex items-center gap-1.5 text-xs text-slate-300 cursor-pointer"
            >
              <input v-model="tuneApplyRestart" type="checkbox" class="obs-checkbox" />
              保存后自动重启生效
            </label>
          </div>
          <div class="flex items-center gap-2">
            <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 text-xs" @click="tuneVisible = false">关闭</button>
            <button type="button" class="obs-btn obs-btn-primary px-4 py-1.5 text-xs" :disabled="tuneSaving" @click="submitTuningConfig">
              {{ tuneSaving ? "保存中…" : "保存配置版本" }}
            </button>
          </div>
        </div>
        <button v-else type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 text-xs" @click="tuneVisible = false">关闭</button>
      </template>
    </ObsDrawer>
  </div>
</template>
