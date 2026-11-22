<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from "vue";
import { useRouter } from "vue-router";
import {
  Activity,
  AlertTriangle,
  Download,
  FileCode2,
  History,
  Play,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  Settings2,
  Sliders,
  Square,
  Terminal,
  Trash2,
  Upload,
} from "lucide-vue-next";
import { ElMessage, ElMessageBox } from "element-plus";
import type { WorkerRole, WorkerRuntimeState } from "@/api/overview";
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
  type CrawlProfileInfo,
  type CrawlerWorkerConfig,
  type IndexerWorkerConfig,
  type WorkerConfigVersionItem,
  type WorkerSummaryItem,
} from "@/api/workers";

const router = useRouter();
const loading = ref(true);
const workers = ref<WorkerSummaryItem[]>([]);
const profiles = ref<CrawlProfileInfo[]>([]);
const systemSoftNofile = ref<number | null>(null);
const actionLoadingMap = reactive<Record<string, boolean>>({});
let pollTimer: number | null = null;

// 新建 Worker 对话框
const createDialogVisible = ref(false);
const createSubmitting = ref(false);
const createForm = reactive({
  id: "",
  role: "crawler" as WorkerRole,
  display_name: "",
  port: 12314,
  crawl_profile: "P1",
  torrent_workers: 64,
  insert_workers: 64,
  batch_size: 1000,
  auto_restart: true,
  max_restarts: 5,
  restart_backoff_secs: 3,
});

// 重启策略对话框
const policyDialogVisible = ref(false);
const policySaving = ref(false);
const policyTargetId = ref("");
const policyForm = reactive({
  display_name: "",
  auto_restart: true,
  max_restarts: 5,
  restart_backoff_secs: 3,
});

// Stderr 日志弹窗
const stderrDialogVisible = ref(false);
const stderrTarget = ref<WorkerSummaryItem | null>(null);

// 参数调优抽屉
const tuningVisible = ref(false);
const tuningLoading = ref(false);
const tuningSaving = ref(false);
const tuningWorker = ref<WorkerSummaryItem | null>(null);
const tuningVersion = ref(1);
const tuningTab = ref("l1");
const tuningNote = ref("");
const crawlerForm = ref<CrawlerWorkerConfig>(defaultCrawlerConfig());
const indexerForm = ref<IndexerWorkerConfig>(defaultIndexerConfig());
const bootstrapNodesText = ref("");
const jsonEditorText = ref("");
const jsonParseError = ref("");
const showProfilesTable = ref(true);
const versions = ref<WorkerConfigVersionItem[]>([]);
const versionsLoading = ref(false);
const fileInputRef = ref<HTMLInputElement | null>(null);

const selectedProfileInfo = computed<CrawlProfileInfo | null>(() => {
  const name = crawlerForm.value.crawl_profile;
  return profiles.value.find((p) => p.name === name) ?? null;
});

const nofileTooLow = computed(() => {
  if (systemSoftNofile.value === null || !selectedProfileInfo.value) return false;
  return systemSoftNofile.value < selectedProfileInfo.value.recommended_nofile;
});

const peerLookupOverrideCount = computed(() => {
  return countSetValues(crawlerForm.value.peer_lookup as Record<string, unknown>);
});

const l3OverrideCount = computed(() => {
  const c = crawlerForm.value.crawl;
  return (
    countSetValues(c.rate_limit as Record<string, unknown>) +
    countSetValues(c.pool as Record<string, unknown>) +
    countSetValues(c.bootstrap as Record<string, unknown>) +
    countSetValues(c.target as Record<string, unknown>) +
    countSetValues(c.scheduler as Record<string, unknown>)
  );
});

function countSetValues(obj: Record<string, unknown> | undefined): number {
  if (!obj) return 0;
  return Object.values(obj).filter(
    (v) => v !== undefined && v !== null && v !== "" && !(Array.isArray(v) && v.length === 0),
  ).length;
}

function isOverridden(v: unknown): boolean {
  return v !== undefined && v !== null && v !== "";
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

function canDeleteWorker(w: WorkerSummaryItem): boolean {
  return w.runtime_state === "stopped" || w.runtime_state === "crashed";
}

function formatTs(ts: number | null | undefined): string {
  if (!ts) return "-";
  return new Date(ts * 1000).toLocaleString("zh-CN", { hour12: false });
}

function formatSummary(w: WorkerSummaryItem): string {
  const s = w.summary;
  if (!s) return "-";
  if (w.role === "crawler") {
    return `UDP :${s.port ?? "-"} · ${s.crawl_profile ?? "-"} · ${s.netmode ?? "-"} · Worker=${s.max_worker_count ?? "-"}`;
  }
  return `Stream并发=${s.torrent_workers ?? "-"} · Insert并发=${s.insert_workers ?? "-"} · 批=${s.batch_size ?? "-"}`;
}

async function fetchAll(silent = false) {
  if (!silent) loading.value = true;
  try {
    const [list, profileResp] = await Promise.all([listWorkers(), getDhtProfiles()]);
    workers.value = list;
    profiles.value = profileResp.profiles;
    systemSoftNofile.value = profileResp.system_soft_nofile;
  } catch (err) {
    if (!silent) {
      ElMessage.error(err instanceof Error ? err.message : "加载 Worker 列表失败");
    }
  } finally {
    loading.value = false;
  }
}

function openCreateDialog() {
  const usedPorts = new Set(
    workers.value
      .filter((w) => w.role === "crawler" && w.summary?.port)
      .map((w) => Number(w.summary?.port)),
  );
  let nextPort = 12313;
  while (usedPorts.has(nextPort) && nextPort < 65535) nextPort += 1;

  const nextIdx = workers.value.filter((w) => w.role === "crawler").length + 1;
  createForm.role = "crawler";
  createForm.id = `crawler-${nextIdx}`;
  createForm.display_name = `DHT 采集节点 #${nextIdx}`;
  createForm.port = nextPort;
  createForm.crawl_profile = "P1";
  createForm.torrent_workers = 64;
  createForm.insert_workers = 64;
  createForm.batch_size = 1000;
  createForm.auto_restart = true;
  createForm.max_restarts = 5;
  createForm.restart_backoff_secs = 3;
  createDialogVisible.value = true;
}

watch(
  () => createForm.role,
  (role) => {
    const cnt = workers.value.filter((w) => w.role === role).length + 1;
    if (role === "crawler") {
      if (!createForm.id || createForm.id.startsWith("indexer-")) {
        createForm.id = `crawler-${cnt}`;
        createForm.display_name = `DHT 采集节点 #${cnt}`;
      }
    } else if (!createForm.id || createForm.id.startsWith("crawler-")) {
      createForm.id = `indexer-${cnt}`;
      createForm.display_name = `索引入库节点 #${cnt}`;
    }
  },
);

async function submitCreateWorker() {
  createSubmitting.value = true;
  try {
    const config =
      createForm.role === "crawler"
        ? defaultCrawlerConfig(createForm.port, createForm.crawl_profile)
        : {
            ...defaultIndexerConfig(),
            torrent_workers: createForm.torrent_workers,
            insert_workers: createForm.insert_workers,
            batch_size: createForm.batch_size,
          };
    await createWorker({
      id: createForm.id.trim(),
      role: createForm.role,
      display_name: createForm.display_name.trim(),
      config,
      auto_restart: createForm.auto_restart,
      max_restarts: createForm.max_restarts,
      restart_backoff_secs: createForm.restart_backoff_secs,
    });
    ElMessage.success(`已创建节点 ${createForm.id.trim()}`);
    createDialogVisible.value = false;
    await fetchAll();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "创建节点失败");
  } finally {
    createSubmitting.value = false;
  }
}

async function handleStart(w: WorkerSummaryItem) {
  actionLoadingMap[w.id] = true;
  try {
    await startWorker(w.id);
    ElMessage.success(`已下发启动指令: ${w.id}`);
    await fetchAll(true);
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "启动失败");
  } finally {
    actionLoadingMap[w.id] = false;
  }
}

async function handleStop(w: WorkerSummaryItem) {
  try {
    await ElMessageBox.confirm(
      `确认向节点 ${w.id} 下发平滑排空停机指令吗？进程会等待手头任务刷新后退出。`,
      "确认停止节点",
      { type: "warning", confirmButtonText: "确认停止", cancelButtonText: "取消" },
    );
  } catch {
    return;
  }
  actionLoadingMap[w.id] = true;
  try {
    await stopWorker(w.id);
    ElMessage.success(`已停止节点: ${w.id}`);
    await fetchAll(true);
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "停止失败");
  } finally {
    actionLoadingMap[w.id] = false;
  }
}

async function handleRestart(w: WorkerSummaryItem) {
  try {
    await ElMessageBox.confirm(
      `确认重启节点 ${w.id} 吗？重启期间将平滑停机并重新读取最新激活配置。`,
      "确认重启节点",
      { type: "info", confirmButtonText: "确认重启", cancelButtonText: "取消" },
    );
  } catch {
    return;
  }
  actionLoadingMap[w.id] = true;
  try {
    await restartWorker(w.id);
    ElMessage.success(`节点 ${w.id} 已完成重启`);
    await fetchAll(true);
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "重启失败");
  } finally {
    actionLoadingMap[w.id] = false;
  }
}

async function handleDelete(w: WorkerSummaryItem) {
  if (!canDeleteWorker(w)) {
    ElMessage.warning("请先停止运行中的节点，再执行删除");
    return;
  }
  try {
    await ElMessageBox.confirm(
      `确定永久删除节点 "${w.display_name}" (${w.id}) 及其所有历史配置版本吗？此操作不可撤销。`,
      "删除确认",
      { type: "warning", confirmButtonText: "确认删除", cancelButtonText: "取消" },
    );
  } catch {
    return;
  }
  actionLoadingMap[w.id] = true;
  try {
    await deleteWorker(w.id);
    ElMessage.success(`已删除节点 ${w.id}`);
    await fetchAll(true);
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "删除失败");
  } finally {
    actionLoadingMap[w.id] = false;
  }
}

function openPolicyDialog(w: WorkerSummaryItem) {
  policyTargetId.value = w.id;
  policyForm.display_name = w.display_name;
  policyForm.auto_restart = w.auto_restart;
  policyForm.max_restarts = w.max_restarts;
  policyForm.restart_backoff_secs = w.restart_backoff_secs;
  policyDialogVisible.value = true;
}

async function submitPolicy() {
  policySaving.value = true;
  try {
    await updateWorkerPolicy(policyTargetId.value, {
      display_name: policyForm.display_name.trim(),
      auto_restart: policyForm.auto_restart,
      max_restarts: policyForm.max_restarts,
      restart_backoff_secs: policyForm.restart_backoff_secs,
    });
    ElMessage.success("重启策略已保存");
    policyDialogVisible.value = false;
    await fetchAll(true);
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "保存策略失败");
  } finally {
    policySaving.value = false;
  }
}

function openStderrModal(w: WorkerSummaryItem) {
  stderrTarget.value = w;
  stderrDialogVisible.value = true;
}

function syncBootstrapTextFromModel() {
  const nodes = crawlerForm.value.crawl.bootstrap.nodes;
  bootstrapNodesText.value = nodes && nodes.length > 0 ? nodes.join("\n") : "";
}

function syncBootstrapModelFromText() {
  const lines = bootstrapNodesText.value
    .split(/\r?\n|,/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  if (lines.length === 0) {
    delete crawlerForm.value.crawl.bootstrap.nodes;
  } else {
    crawlerForm.value.crawl.bootstrap.nodes = lines;
  }
}

function syncJsonFromForm() {
  if (!tuningWorker.value) return;
  jsonParseError.value = "";
  if (tuningWorker.value.role === "crawler") {
    syncBootstrapModelFromText();
    const cleaned = normalizeCrawlerConfigPayload(crawlerForm.value);
    jsonEditorText.value = JSON.stringify(cleaned, null, 2);
  } else {
    jsonEditorText.value = JSON.stringify(indexerForm.value, null, 2);
  }
}

function applyJsonToForm(): boolean {
  if (!tuningWorker.value) return false;
  try {
    const parsed = JSON.parse(jsonEditorText.value) as Record<string, unknown>;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      jsonParseError.value = "配置 JSON 必须是对象";
      return false;
    }
    if (tuningWorker.value.role === "crawler") {
      const merged: CrawlerWorkerConfig = {
        ...defaultCrawlerConfig(),
        ...(parsed as Partial<CrawlerWorkerConfig>),
        metadata: {
          ...defaultCrawlerConfig().metadata,
          ...((parsed.metadata as Partial<CrawlerWorkerConfig["metadata"]>) ?? {}),
        },
        discovery: {
          ...defaultCrawlerConfig().discovery,
          ...((parsed.discovery as Partial<CrawlerWorkerConfig["discovery"]>) ?? {}),
        },
        peer_lookup: {
          ...((parsed.peer_lookup as CrawlerWorkerConfig["peer_lookup"]) ?? {}),
        },
        crawl: {
          rate_limit: { ...((parsed.crawl as CrawlerWorkerConfig["crawl"])?.rate_limit ?? {}) },
          pool: { ...((parsed.crawl as CrawlerWorkerConfig["crawl"])?.pool ?? {}) },
          bootstrap: { ...((parsed.crawl as CrawlerWorkerConfig["crawl"])?.bootstrap ?? {}) },
          target: { ...((parsed.crawl as CrawlerWorkerConfig["crawl"])?.target ?? {}) },
          scheduler: { ...((parsed.crawl as CrawlerWorkerConfig["crawl"])?.scheduler ?? {}) },
        },
      };
      // 保留任意未知键以便服务端严格 deny_unknown_fields 校验或及时报错提示
      crawlerForm.value = merged;
      syncBootstrapTextFromModel();
    } else {
      indexerForm.value = {
        ...defaultIndexerConfig(),
        ...(parsed as Partial<IndexerWorkerConfig>),
      };
    }
    jsonParseError.value = "";
    return true;
  } catch (err) {
    jsonParseError.value = err instanceof Error ? err.message : "JSON 语法错误";
    return false;
  }
}

watch(tuningTab, (nextTab, prevTab) => {
  if (nextTab === "json") {
    syncJsonFromForm();
  } else if (prevTab === "json" && jsonEditorText.value.trim()) {
    applyJsonToForm();
  }
});

async function loadVersions(id: string) {
  versionsLoading.value = true;
  try {
    versions.value = await listWorkerConfigVersions(id, 30);
  } catch {
    versions.value = [];
  } finally {
    versionsLoading.value = false;
  }
}

async function openTuningDrawer(w: WorkerSummaryItem) {
  tuningWorker.value = w;
  tuningTab.value = w.role === "crawler" ? "l1" : "indexer_core";
  tuningNote.value = "";
  jsonParseError.value = "";
  tuningVisible.value = true;
  tuningLoading.value = true;

  try {
    const [detail] = await Promise.all([getWorkerConfig(w.id), loadVersions(w.id)]);
    tuningVersion.value = detail.version;
    if (w.role === "crawler") {
      const base = defaultCrawlerConfig();
      const raw = detail.config as CrawlerWorkerConfig;
      crawlerForm.value = {
        ...base,
        ...raw,
        metadata: { ...base.metadata, ...(raw.metadata ?? {}) },
        discovery: { ...base.discovery, ...(raw.discovery ?? {}) },
        peer_lookup: { ...(raw.peer_lookup ?? {}) },
        crawl: {
          rate_limit: { ...(raw.crawl?.rate_limit ?? {}) },
          pool: { ...(raw.crawl?.pool ?? {}) },
          bootstrap: { ...(raw.crawl?.bootstrap ?? {}) },
          target: { ...(raw.crawl?.target ?? {}) },
          scheduler: { ...(raw.crawl?.scheduler ?? {}) },
        },
      };
      syncBootstrapTextFromModel();
    } else {
      indexerForm.value = {
        ...defaultIndexerConfig(),
        ...(detail.config as IndexerWorkerConfig),
      };
    }
    syncJsonFromForm();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "读取节点配置失败");
    tuningVisible.value = false;
  } finally {
    tuningLoading.value = false;
  }
}

function clearL3Section(section: keyof CrawlerWorkerConfig["crawl"]) {
  crawlerForm.value.crawl[section] = {};
  if (section === "bootstrap") {
    bootstrapNodesText.value = "";
  }
}

function profileRowClassName({ row }: { row: CrawlProfileInfo }): string {
  return row.name === crawlerForm.value.crawl_profile ? "bg-indigo-50 font-medium" : "";
}

function clearOptionalKey<T extends Record<string, unknown>>(obj: T, key: keyof T) {
  delete obj[key];
}

function exportConfigJson() {
  syncJsonFromForm();
  const id = tuningWorker.value?.id ?? "worker";
  const blob = new Blob([jsonEditorText.value], { type: "application/json;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `${id}-v${tuningVersion.value}-config.json`;
  a.click();
  URL.revokeObjectURL(url);
}

function triggerImportJson() {
  fileInputRef.value?.click();
}

function handleImportJsonFile(ev: Event) {
  const input = ev.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;
  const reader = new FileReader();
  reader.onload = () => {
    jsonEditorText.value = String(reader.result ?? "");
    if (applyJsonToForm()) {
      ElMessage.success("已导入 JSON 配置到表单，请检查后点击保存");
    }
  };
  reader.readAsText(file);
  input.value = "";
}

async function saveTuningConfig(applyRestart: boolean) {
  if (!tuningWorker.value) return;
  if (tuningTab.value === "json") {
    if (!applyJsonToForm()) {
      ElMessage.error(`JSON 格式有误: ${jsonParseError.value}`);
      return;
    }
  } else if (tuningWorker.value.role === "crawler") {
    syncBootstrapModelFromText();
  }

  let payloadConfig: CrawlerWorkerConfig | IndexerWorkerConfig;
  if (tuningWorker.value.role === "crawler") {
    if (tuningTab.value === "json") {
      // 如果用户在 JSON 页手工编辑了字段，直接传给服务端，让 deny_unknown_fields 发挥最终裁决
      try {
        payloadConfig = JSON.parse(jsonEditorText.value) as CrawlerWorkerConfig;
      } catch {
        ElMessage.error("JSON 解析失败");
        return;
      }
    } else {
      payloadConfig = normalizeCrawlerConfigPayload(crawlerForm.value);
    }
  } else {
    payloadConfig = { ...indexerForm.value };
  }

  tuningSaving.value = true;
  try {
    const res = await updateWorkerConfig(tuningWorker.value.id, {
      config: payloadConfig,
      note: tuningNote.value.trim() || "面板调优更新",
      apply_restart: applyRestart,
    });
    tuningVersion.value = res.version;
    tuningNote.value = "";
    if (res.restart_error) {
      ElMessage.warning(`配置已保存为 v${res.version}，但平滑重启失败: ${res.restart_error}`);
    } else if (res.restarted) {
      ElMessage.success(`配置已保存为 v${res.version}，节点已平滑重启生效`);
    } else {
      ElMessage.success(`配置已保存为 v${res.version}`);
    }
    await Promise.all([loadVersions(tuningWorker.value.id), fetchAll(true)]);
    syncJsonFromForm();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "保存配置失败");
  } finally {
    tuningSaving.value = false;
  }
}

async function handleRollbackVersion(targetVersion: number) {
  if (!tuningWorker.value) return;
  let applyRestart = false;
  try {
    await ElMessageBox.confirm(
      `确认将节点 ${tuningWorker.value.id} 回滚到 v${targetVersion} 吗？`,
      "版本回滚确认",
      { type: "warning", confirmButtonText: "回滚配置", cancelButtonText: "取消" },
    );
    const isRunning =
      tuningWorker.value.runtime_state === "running" ||
      tuningWorker.value.runtime_state === "starting";
    if (isRunning) {
      try {
        await ElMessageBox.confirm(
          "该节点当前正在运行中，回滚后是否立即平滑重启使旧版本生效？",
          "是否重启节点",
          {
            confirmButtonText: "回滚并重启",
            cancelButtonText: "仅回滚配置",
            distinguishCancelAndClose: true,
          },
        );
        applyRestart = true;
      } catch (action) {
        if (action === "close") return;
      }
    }
  } catch {
    return;
  }

  tuningSaving.value = true;
  try {
    const res = await rollbackWorkerConfig(tuningWorker.value.id, {
      target_version: targetVersion,
      apply_restart: applyRestart,
    });
    ElMessage.success(
      res.restarted
        ? `已回滚并生成 v${res.version}，节点已平滑重启`
        : `已回滚并生成 v${res.version}`,
    );
    await openTuningDrawer(tuningWorker.value);
    await fetchAll(true);
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "回滚失败");
  } finally {
    tuningSaving.value = false;
  }
}

onMounted(async () => {
  await fetchAll();
  pollTimer = window.setInterval(() => {
    void fetchAll(true);
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
  <div class="max-w-7xl mx-auto space-y-5">
    <!-- 顶部操作栏 -->
    <div class="rounded-xl border border-slate-200 bg-white px-5 py-4 shadow-sm flex flex-wrap items-center justify-between gap-4">
      <div>
        <h2 class="text-base font-semibold text-slate-900">Worker 进程守护与采集/入库参数调优</h2>
        <p class="text-xs text-slate-500 mt-0.5">
          统一管理 Crawler (P1~P17 + L1/L2/L3 调优) 与 Indexer 进程，支持版本快照回滚与平滑启停
        </p>
      </div>
      <div class="flex items-center gap-2.5">
        <span
          v-if="systemSoftNofile !== null"
          class="text-xs text-slate-500 font-mono bg-slate-100 px-2.5 py-1 rounded"
        >
          soft nofile: {{ systemSoftNofile }}
        </span>
        <el-button size="small" :loading="loading" @click="fetchAll()">
          <RefreshCw class="h-3.5 w-3.5 mr-1" />
          刷新状态
        </el-button>
        <el-button size="small" type="primary" @click="openCreateDialog">
          <Plus class="h-3.5 w-3.5 mr-1" />
          新建 Worker 实例
        </el-button>
      </div>
    </div>

    <!-- 实例列表 -->
    <div class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm" v-loading="loading">
      <el-table :data="workers" border stripe size="small">
        <el-table-column label="实例信息" min-width="180">
          <template #default="{ row }">
            <div class="space-y-0.5">
              <div class="font-medium text-slate-900">{{ row.display_name }}</div>
              <div class="text-[11px] font-mono text-slate-500">
                {{ row.id }} · cfg v{{ row.active_config_version }}
                <span v-if="row.active_rule_version">· rule v{{ row.active_rule_version }}</span>
              </div>
            </div>
          </template>
        </el-table-column>

        <el-table-column label="角色" width="95">
          <template #default="{ row }">
            <el-tag size="small" :type="row.role === 'crawler' ? 'primary' : 'success'" effect="plain">
              {{ row.role === "crawler" ? "Crawler" : "Indexer" }}
            </el-tag>
          </template>
        </el-table-column>

        <el-table-column label="运行状态" width="150">
          <template #default="{ row }">
            <div class="space-y-1">
              <el-tag size="small" :type="stateTagType(row.runtime_state)">
                {{ stateLabel(row.runtime_state) }}
              </el-tag>
              <div
                v-if="row.runtime_state === 'draining' && row.drain_status"
                class="text-[11px] text-amber-700 font-mono"
              >
                队列={{ row.drain_status.queued_items }} 飞行={{ row.drain_status.in_flight_items }}
              </div>
            </div>
          </template>
        </el-table-column>

        <el-table-column label="PID / IPC 端口" width="125">
          <template #default="{ row }">
            <div class="font-mono text-xs text-slate-700">
              <div>PID: {{ row.pid ?? "-" }}</div>
              <div class="text-[11px] text-slate-400">IPC: {{ row.worker_port ?? "-" }}</div>
            </div>
          </template>
        </el-table-column>

        <el-table-column label="核心参数摘要" min-width="230">
          <template #default="{ row }">
            <span class="font-mono text-xs text-slate-700">{{ formatSummary(row) }}</span>
          </template>
        </el-table-column>

        <el-table-column label="重启 / 错误" width="150">
          <template #default="{ row }">
            <div class="text-xs">
              <div>
                自动重启:
                <span :class="row.auto_restart ? 'text-emerald-600' : 'text-slate-400'">
                  {{ row.auto_restart ? "开" : "关" }}
                </span>
                ({{ row.restart_count }}/{{ row.max_restarts }})
              </div>
              <div v-if="row.last_error" class="text-[11px] text-rose-600 truncate" :title="row.last_error">
                {{ row.last_error }}
              </div>
            </div>
          </template>
        </el-table-column>

        <el-table-column label="操作" width="405" fixed="right">
          <template #default="{ row }">
            <div class="flex flex-wrap items-center gap-1.5">
              <el-button
                v-if="row.runtime_state === 'stopped' || row.runtime_state === 'crashed'"
                size="small"
                type="success"
                plain
                :loading="actionLoadingMap[row.id]"
                @click="handleStart(row)"
              >
                <Play class="h-3 w-3 mr-0.5" />
                启动
              </el-button>
              <el-button
                v-else
                size="small"
                type="warning"
                plain
                :loading="actionLoadingMap[row.id]"
                @click="handleStop(row)"
              >
                <Square class="h-3 w-3 mr-0.5" />
                停止
              </el-button>

              <el-button
                size="small"
                plain
                :disabled="row.runtime_state !== 'running'"
                :loading="actionLoadingMap[row.id]"
                @click="handleRestart(row)"
              >
                <RotateCcw class="h-3 w-3 mr-0.5" />
                重启
              </el-button>

              <el-button size="small" type="primary" plain @click="openTuningDrawer(row)">
                <Sliders class="h-3 w-3 mr-0.5" />
                调优
              </el-button>

              <el-button
                size="small"
                type="info"
                plain
                @click="router.push(`/workers/${encodeURIComponent(row.id)}/diagnostics`)"
              >
                <Activity class="h-3 w-3 mr-0.5" />
                诊断
              </el-button>

              <el-button size="small" plain @click="openPolicyDialog(row)">
                <Settings2 class="h-3 w-3" />
              </el-button>

              <el-button
                size="small"
                :type="row.last_error ? 'danger' : 'default'"
                plain
                @click="openStderrModal(row)"
                title="查看进程 stderr 日志"
              >
                <Terminal class="h-3 w-3" />
              </el-button>

              <el-button
                size="small"
                type="danger"
                plain
                :disabled="!canDeleteWorker(row)"
                @click="handleDelete(row)"
                :title="canDeleteWorker(row) ? '删除节点' : '运行中的节点禁止删除，请先停止'"
              >
                <Trash2 class="h-3 w-3" />
              </el-button>
            </div>
          </template>
        </el-table-column>
      </el-table>
    </div>

    <!-- 新建 Worker 对话框 -->
    <el-dialog v-model="createDialogVisible" title="新建 Worker 实例" width="520px">
      <el-form label-width="125px" size="small">
        <el-form-item label="实例角色">
          <el-radio-group v-model="createForm.role">
            <el-radio value="crawler">Crawler (DHT 采集节点)</el-radio>
            <el-radio value="indexer">Indexer (流消费入库节点)</el-radio>
          </el-radio-group>
        </el-form-item>
        <el-form-item label="实例 ID">
          <el-input v-model="createForm.id" placeholder="2~48位字母、数字、下划线或短横线" />
        </el-form-item>
        <el-form-item label="显示名称">
          <el-input v-model="createForm.display_name" placeholder="例如：IPv6 高并发采集节点" />
        </el-form-item>

        <template v-if="createForm.role === 'crawler'">
          <el-form-item label="UDP 监听端口">
            <el-input-number v-model="createForm.port" :min="1" :max="65535" />
          </el-form-item>
          <el-form-item label="预设采集档位">
            <el-select v-model="createForm.crawl_profile" class="w-full">
              <el-option
                v-for="p in profiles"
                :key="p.name"
                :label="`${p.name} · ${p.level} (QPS=${p.defaults.find_node_rate_per_sec}, FD建议=${p.recommended_nofile})`"
                :value="p.name"
              />
            </el-select>
          </el-form-item>
        </template>

        <template v-else>
          <el-form-item label="新种消费并发">
            <el-input-number v-model="createForm.torrent_workers" :min="1" :max="1024" />
          </el-form-item>
          <el-form-item label="Manticore 写入并发">
            <el-input-number v-model="createForm.insert_workers" :min="1" :max="1024" />
          </el-form-item>
          <el-form-item label="批量写入大小">
            <el-input-number v-model="createForm.batch_size" :min="10" :max="10000" />
          </el-form-item>
        </template>

        <el-form-item label="异常自动重启">
          <el-switch v-model="createForm.auto_restart" />
        </el-form-item>
        <el-form-item label="最大重启次数">
          <el-input-number v-model="createForm.max_restarts" :min="0" :max="100" />
        </el-form-item>
        <el-form-item label="重启退避(秒)">
          <el-input-number v-model="createForm.restart_backoff_secs" :min="1" :max="300" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button size="small" @click="createDialogVisible = false">取消</el-button>
        <el-button size="small" type="primary" :loading="createSubmitting" @click="submitCreateWorker">
          确认创建
        </el-button>
      </template>
    </el-dialog>

    <!-- 守护策略对话框 -->
    <el-dialog v-model="policyDialogVisible" :title="`编辑重启策略 (${policyTargetId})`" width="460px">
      <el-form label-width="115px" size="small">
        <el-form-item label="显示名称">
          <el-input v-model="policyForm.display_name" />
        </el-form-item>
        <el-form-item label="异常自动重启">
          <el-switch v-model="policyForm.auto_restart" />
        </el-form-item>
        <el-form-item label="熔断重启阈值">
          <el-input-number v-model="policyForm.max_restarts" :min="0" :max="100" />
        </el-form-item>
        <el-form-item label="退避基础时长(秒)">
          <el-input-number v-model="policyForm.restart_backoff_secs" :min="1" :max="300" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button size="small" @click="policyDialogVisible = false">取消</el-button>
        <el-button size="small" type="primary" :loading="policySaving" @click="submitPolicy">
          保存策略
        </el-button>
      </template>
    </el-dialog>

    <!-- Stderr 日志对话框 -->
    <el-dialog
      v-model="stderrDialogVisible"
      :title="`节点标准错误日志 (${stderrTarget?.id ?? ''})`"
      width="720px"
    >
      <div class="space-y-3">
        <div v-if="stderrTarget?.last_error" class="rounded bg-rose-50 border border-rose-200 px-3 py-2 text-xs text-rose-700">
          最近状态错误: {{ stderrTarget.last_error }}
        </div>
        <pre class="rounded-lg bg-slate-950 text-slate-100 p-3.5 text-xs font-mono max-h-96 overflow-auto whitespace-pre-wrap break-all">{{ stderrTarget?.stderr_tail || "(暂无 stderr 输出)" }}</pre>
      </div>
      <template #footer>
        <el-button size="small" @click="stderrDialogVisible = false">关闭</el-button>
      </template>
    </el-dialog>

    <!-- L1/L2/L3 参数调优抽屉 -->
    <el-drawer
      v-model="tuningVisible"
      :title="tuningWorker ? `${tuningWorker.display_name} (${tuningWorker.id}) · 参数调优 (v${tuningVersion})` : '参数调优'"
      size="820px"
      destroy-on-close
    >
      <div class="space-y-4 pb-16" v-loading="tuningLoading">
        <input
          ref="fileInputRef"
          type="file"
          accept=".json,application/json"
          class="hidden"
          @change="handleImportJsonFile"
        />

        <!-- Crawler 调优面板 -->
        <template v-if="tuningWorker?.role === 'crawler'">
          <el-alert
            v-if="nofileTooLow && selectedProfileInfo"
            type="warning"
            show-icon
            :closable="false"
            class="mb-2"
          >
            <template #title>
              <span class="text-xs">
                当前系统 `soft nofile={{ systemSoftNofile }}`，低于 {{ selectedProfileInfo.name }} 档位建议的
                `{{ selectedProfileInfo.recommended_nofile }}`。高并发元数据抓手可能触发文件描述符耗尽或被进程自动降额，建议调整系统 `ulimit -n` 或降低 `max_worker_count`。
              </span>
            </template>
          </el-alert>

          <el-tabs v-model="tuningTab" type="border-card">
            <!-- L1: 基础与档位 -->
            <el-tab-pane label="L1 基础与档位" name="l1">
              <div class="space-y-5">
                <el-form label-width="175px" size="small">
                  <div class="grid grid-cols-1 md:grid-cols-2 gap-x-4">
                    <el-form-item label="UDP 监听端口 (port)">
                      <el-input-number v-model="crawlerForm.port" :min="1" :max="65535" class="w-full" />
                    </el-form-item>
                    <el-form-item label="IP 协议模式 (netmode)">
                      <el-select v-model="crawlerForm.netmode" class="w-full">
                        <el-option label="ipv4_only (仅 IPv4)" value="ipv4_only" />
                        <el-option label="ipv6_only (仅 IPv6)" value="ipv6_only" />
                        <el-option label="dual (双栈)" value="dual" />
                      </el-select>
                    </el-form-item>
                    <el-form-item label="预设档位 (crawl_profile)">
                      <el-select v-model="crawlerForm.crawl_profile" class="w-full">
                        <el-option
                          v-for="p in profiles"
                          :key="p.name"
                          :label="`${p.name} · ${p.level}`"
                          :value="p.name"
                        />
                      </el-select>
                    </el-form-item>
                    <el-form-item label="日志级别 (log_level)">
                      <el-select v-model="crawlerForm.log_level" class="w-full">
                        <el-option label="error" value="error" />
                        <el-option label="warn" value="warn" />
                        <el-option label="info" value="info" />
                        <el-option label="debug" value="debug" />
                        <el-option label="trace" value="trace" />
                      </el-select>
                    </el-form-item>
                    <el-form-item label="本地缓存容量">
                      <el-input-number v-model="crawlerForm.metadata_cache_capacity" :min="1000" :step="10000" class="w-full" />
                    </el-form-item>
                    <el-form-item label="Redis 推送并发数">
                      <el-input-number v-model="crawlerForm.push_concurrency" :min="1" :max="512" class="w-full" />
                    </el-form-item>
                    <el-form-item label="Hash 队列容量">
                      <el-input-number v-model="crawlerForm.hash_queue_capacity" :min="64" :step="512" class="w-full" />
                    </el-form-item>
                    <el-form-item label="Pending 锁 TTL (秒)">
                      <el-input-number v-model="crawlerForm.metadata_pending_ttl_secs" :min="10" class="w-full" />
                    </el-form-item>
                    <el-form-item label="失败退避 TTL (秒)">
                      <el-input-number v-model="crawlerForm.metadata_failure_backoff_secs" :min="1" class="w-full" />
                    </el-form-item>
                  </div>
                </el-form>

                <!-- P1-P17 档位对照表 -->
                <div class="rounded-lg border border-slate-200 p-3 space-y-2 bg-slate-50/50">
                  <div class="flex items-center justify-between">
                    <div class="text-xs font-semibold text-slate-800">
                      P1 ~ P17 采集档位参数对照表（当前选中：{{ crawlerForm.crawl_profile }}）
                    </div>
                    <el-button size="small" link @click="showProfilesTable = !showProfilesTable">
                      {{ showProfilesTable ? "收起表格" : "展开表格" }}
                    </el-button>
                  </div>
                  <el-table
                    v-if="showProfilesTable"
                    :data="profiles"
                    size="small"
                    border
                    max-height="280"
                    :row-class-name="profileRowClassName"
                  >
                    <el-table-column prop="name" label="档位" width="65" />
                    <el-table-column prop="level" label="定位" width="120" show-overflow-tooltip />
                    <el-table-column label="QPS/s" width="75">
                      <template #default="{ row }">{{ row.defaults.find_node_rate_per_sec }}</template>
                    </el-table-column>
                    <el-table-column label="InFlight" width="80">
                      <template #default="{ row }">{{ row.defaults.find_node_max_in_flight }}</template>
                    </el-table-column>
                    <el-table-column label="PeerLookup" width="90">
                      <template #default="{ row }">{{ row.defaults.peer_lookup_rate_per_sec }}/s</template>
                    </el-table-column>
                    <el-table-column label="路由池上限" width="95">
                      <template #default="{ row }">{{ row.defaults.pool_capacity }}</template>
                    </el-table-column>
                    <el-table-column label="建议 FD" width="85">
                      <template #default="{ row }">{{ row.recommended_nofile }}</template>
                    </el-table-column>
                    <el-table-column label="选择" width="75" fixed="right">
                      <template #default="{ row }">
                        <el-button
                          size="small"
                          link
                          type="primary"
                          @click="crawlerForm.crawl_profile = row.name"
                        >
                          {{ row.name === crawlerForm.crawl_profile ? "已选" : "切换" }}
                        </el-button>
                      </template>
                    </el-table-column>
                  </el-table>
                </div>
              </div>
            </el-tab-pane>

            <!-- L2: 抓取、主动发现与 PeerLookup -->
            <el-tab-pane label="L2 抓取与发现" name="l2">
              <div class="space-y-5">
                <div class="rounded-lg border border-slate-200 p-3.5 space-y-3">
                  <div class="text-xs font-semibold text-slate-800">Metadata BEP-9 元数据抓取</div>
                  <el-form label-width="180px" size="small">
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-x-4">
                      <el-form-item label="抓取超时 (timeout_secs)">
                        <el-input-number v-model="crawlerForm.metadata.timeout_secs" :min="1" class="w-full" />
                      </el-form-item>
                      <el-form-item label="TCP 连接超时 (ms)">
                        <el-input-number v-model="crawlerForm.metadata.connect_timeout_ms" :min="100" :step="100" class="w-full" />
                      </el-form-item>
                      <el-form-item label="最大并发 Worker 数">
                        <el-input-number v-model="crawlerForm.metadata.max_worker_count" :min="1" :max="16384" class="w-full" />
                      </el-form-item>
                      <el-form-item label="任务队列容量">
                        <el-input-number v-model="crawlerForm.metadata.max_queue_size" :min="100" :step="500" class="w-full" />
                      </el-form-item>
                      <el-form-item label="Peer 失败缓存容量">
                        <el-input-number v-model="crawlerForm.metadata.peer_failure_cache_capacity" :min="100" :step="1000" class="w-full" />
                      </el-form-item>
                      <el-form-item label="Peer 失败缓存 TTL(s)">
                        <el-input-number v-model="crawlerForm.metadata.peer_failure_ttl_secs" :min="1" class="w-full" />
                      </el-form-item>
                      <el-form-item label="首次重试延迟(s)">
                        <el-input-number v-model="crawlerForm.metadata.retry_initial_delay_secs" :min="1" class="w-full" />
                      </el-form-item>
                      <el-form-item label="最大重试延迟(s)">
                        <el-input-number v-model="crawlerForm.metadata.retry_max_delay_secs" :min="1" class="w-full" />
                      </el-form-item>
                      <el-form-item label="最大重试次数">
                        <el-input-number v-model="crawlerForm.metadata.retry_max_attempts" :min="0" :max="50" class="w-full" />
                      </el-form-item>
                      <el-form-item label="重试历史 TTL(s)">
                        <el-input-number v-model="crawlerForm.metadata.retry_history_ttl_secs" :min="10" class="w-full" />
                      </el-form-item>
                      <el-form-item label="重试历史容量">
                        <el-input-number v-model="crawlerForm.metadata.retry_history_capacity" :min="100" :step="1000" class="w-full" />
                      </el-form-item>
                    </div>
                  </el-form>
                </div>

                <div class="rounded-lg border border-slate-200 p-3.5 space-y-3">
                  <div class="flex items-center justify-between">
                    <span class="text-xs font-semibold text-slate-800">Discovery 主动采样发现 (BEP-51)</span>
                    <el-switch v-model="crawlerForm.discovery.enabled" size="small" active-text="启用" inactive-text="停用" />
                  </div>
                  <el-form label-width="185px" size="small">
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-x-4">
                      <el-form-item label="sample 查询速率 (/s)">
                        <el-input-number v-model="crawlerForm.discovery.sample_queries_per_second" :min="0" class="w-full" />
                      </el-form-item>
                      <el-form-item label="sample 突发桶">
                        <el-input-number v-model="crawlerForm.discovery.sample_burst" :min="0" class="w-full" />
                      </el-form-item>
                      <el-form-item label="sample 最大并发">
                        <el-input-number v-model="crawlerForm.discovery.sample_max_in_flight" :min="0" class="w-full" />
                      </el-form-item>
                      <el-form-item label="sample 超时(s)">
                        <el-input-number v-model="crawlerForm.discovery.sample_query_timeout_secs" :min="1" class="w-full" />
                      </el-form-item>
                      <el-form-item label="最小采样间隔(s)">
                        <el-input-number v-model="crawlerForm.discovery.sample_min_interval_secs" :min="1" class="w-full" />
                      </el-form-item>
                      <el-form-item label="失败冷却退避(s)">
                        <el-input-number v-model="crawlerForm.discovery.sample_failure_backoff_secs" :min="1" class="w-full" />
                      </el-form-item>
                      <el-form-item label="无新Hash退避(s)">
                        <el-input-number v-model="crawlerForm.discovery.sample_no_new_backoff_secs" :min="1" class="w-full" />
                      </el-form-item>
                      <el-form-item label="最大跟踪节点数">
                        <el-input-number v-model="crawlerForm.discovery.sample_max_nodes" :min="1" class="w-full" />
                      </el-form-item>
                      <el-form-item label="冷却表容量">
                        <el-input-number v-model="crawlerForm.discovery.sample_cooldown_capacity" :min="100" class="w-full" />
                      </el-form-item>
                      <el-form-item label="Hash 去重表容量">
                        <el-input-number v-model="crawlerForm.discovery.hash_dedup_capacity" :min="100" class="w-full" />
                      </el-form-item>
                      <el-form-item label="Hash 去重 TTL(s)">
                        <el-input-number v-model="crawlerForm.discovery.hash_dedup_ttl_secs" :min="1" class="w-full" />
                      </el-form-item>
                      <el-form-item label="get_peers Hash 限速">
                        <el-input-number v-model="crawlerForm.discovery.get_peers_hashes_per_second" :min="0" class="w-full" />
                      </el-form-item>
                      <el-form-item label="get_peers 突发">
                        <el-input-number v-model="crawlerForm.discovery.get_peers_burst" :min="0" class="w-full" />
                      </el-form-item>
                      <el-form-item label="sample Hash 限速">
                        <el-input-number v-model="crawlerForm.discovery.sample_hashes_per_second" :min="0" class="w-full" />
                      </el-form-item>
                      <el-form-item label="sample Hash 突发">
                        <el-input-number v-model="crawlerForm.discovery.sample_hash_burst" :min="0" class="w-full" />
                      </el-form-item>
                    </div>
                  </el-form>
                </div>

                <div class="rounded-lg border border-slate-200 p-3.5 space-y-3">
                  <div class="flex items-center justify-between">
                    <div class="flex items-center gap-2">
                      <span class="text-xs font-semibold text-slate-800">PeerLookup 覆写 (可选)</span>
                      <el-tag v-if="peerLookupOverrideCount > 0" size="small" type="warning">
                        已覆写 {{ peerLookupOverrideCount }} 项
                      </el-tag>
                    </div>
                    <el-button size="small" link @click="crawlerForm.peer_lookup = {}">
                      全部恢复档位默认
                    </el-button>
                  </div>
                  <el-form label-width="195px" size="small">
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-x-4">
                      <el-form-item label="lookup 速率 (/s)">
                        <div class="flex items-center gap-1 w-full">
                          <el-input-number
                            v-model="crawlerForm.peer_lookup.max_lookups_per_second"
                            :min="1"
                            :value-on-clear="null"
                            :placeholder="`默认 ${selectedProfileInfo?.defaults.peer_lookup_rate_per_sec ?? '-'}`"
                            class="flex-1"
                          />
                          <el-button
                            v-if="isOverridden(crawlerForm.peer_lookup.max_lookups_per_second)"
                            size="small"
                            link
                            @click="clearOptionalKey(crawlerForm.peer_lookup as Record<string, unknown>, 'max_lookups_per_second')"
                          >
                            清除
                          </el-button>
                        </div>
                      </el-form-item>
                      <el-form-item label="lookup 突发桶">
                        <div class="flex items-center gap-1 w-full">
                          <el-input-number
                            v-model="crawlerForm.peer_lookup.burst"
                            :min="1"
                            :value-on-clear="null"
                            :placeholder="`默认 ${selectedProfileInfo?.defaults.peer_lookup_burst ?? '-'}`"
                            class="flex-1"
                          />
                          <el-button
                            v-if="isOverridden(crawlerForm.peer_lookup.burst)"
                            size="small"
                            link
                            @click="clearOptionalKey(crawlerForm.peer_lookup as Record<string, unknown>, 'burst')"
                          >
                            清除
                          </el-button>
                        </div>
                      </el-form-item>
                      <el-form-item label="最大活跃 lookup 数">
                        <div class="flex items-center gap-1 w-full">
                          <el-input-number
                            v-model="crawlerForm.peer_lookup.max_active_lookups"
                            :min="1"
                            :value-on-clear="null"
                            :placeholder="`默认 ${selectedProfileInfo?.defaults.peer_lookup_max_active ?? '-'}`"
                            class="flex-1"
                          />
                          <el-button
                            v-if="isOverridden(crawlerForm.peer_lookup.max_active_lookups)"
                            size="small"
                            link
                            @click="clearOptionalKey(crawlerForm.peer_lookup as Record<string, unknown>, 'max_active_lookups')"
                          >
                            清除
                          </el-button>
                        </div>
                      </el-form-item>
                      <el-form-item label="单次最大查询次数">
                        <div class="flex items-center gap-1 w-full">
                          <el-input-number
                            v-model="crawlerForm.peer_lookup.max_queries"
                            :min="1"
                            :value-on-clear="null"
                            placeholder="引擎默认"
                            class="flex-1"
                          />
                          <el-button
                            v-if="isOverridden(crawlerForm.peer_lookup.max_queries)"
                            size="small"
                            link
                            @click="clearOptionalKey(crawlerForm.peer_lookup as Record<string, unknown>, 'max_queries')"
                          >
                            清除
                          </el-button>
                        </div>
                      </el-form-item>
                      <el-form-item label="单lookup并发查询数">
                        <div class="flex items-center gap-1 w-full">
                          <el-input-number
                            v-model="crawlerForm.peer_lookup.max_concurrent_queries"
                            :min="1"
                            :value-on-clear="null"
                            placeholder="引擎默认"
                            class="flex-1"
                          />
                          <el-button
                            v-if="isOverridden(crawlerForm.peer_lookup.max_concurrent_queries)"
                            size="small"
                            link
                            @click="clearOptionalKey(crawlerForm.peer_lookup as Record<string, unknown>, 'max_concurrent_queries')"
                          >
                            清除
                          </el-button>
                        </div>
                      </el-form-item>
                      <el-form-item label="lookup 总超时 (ms)">
                        <div class="flex items-center gap-1 w-full">
                          <el-input-number
                            v-model="crawlerForm.peer_lookup.lookup_timeout_ms"
                            :min="100"
                            :step="100"
                            :value-on-clear="null"
                            placeholder="引擎默认"
                            class="flex-1"
                          />
                          <el-button
                            v-if="isOverridden(crawlerForm.peer_lookup.lookup_timeout_ms)"
                            size="small"
                            link
                            @click="clearOptionalKey(crawlerForm.peer_lookup as Record<string, unknown>, 'lookup_timeout_ms')"
                          >
                            清除
                          </el-button>
                        </div>
                      </el-form-item>
                      <el-form-item label="单query超时 (ms)">
                        <div class="flex items-center gap-1 w-full">
                          <el-input-number
                            v-model="crawlerForm.peer_lookup.query_timeout_ms"
                            :min="50"
                            :step="50"
                            :value-on-clear="null"
                            placeholder="引擎默认"
                            class="flex-1"
                          />
                          <el-button
                            v-if="isOverridden(crawlerForm.peer_lookup.query_timeout_ms)"
                            size="small"
                            link
                            @click="clearOptionalKey(crawlerForm.peer_lookup as Record<string, unknown>, 'query_timeout_ms')"
                          >
                            清除
                          </el-button>
                        </div>
                      </el-form-item>
                    </div>
                  </el-form>
                </div>
              </div>
            </el-tab-pane>

            <!-- L3: 高级引擎细节覆写 -->
            <el-tab-pane name="l3">
              <template #label>
                <span>L3 引擎细节</span>
                <el-tag v-if="l3OverrideCount > 0" size="small" type="warning" class="ml-1.5">
                  {{ l3OverrideCount }}
                </el-tag>
              </template>
              <div class="space-y-4 text-xs">
                <p class="text-slate-500">
                  留空字段将自动继承当前档位 ({{ crawlerForm.crawl_profile }}) 默认值；点击“清除”即可取消单字段覆写。
                </p>

                <!-- rate_limit -->
                <div class="rounded-lg border border-slate-200 p-3.5 space-y-2.5">
                  <div class="flex items-center justify-between">
                    <span class="font-semibold text-slate-800">crawl.rate_limit (UDP 发包与回包限流)</span>
                    <el-button size="small" link @click="clearL3Section('rate_limit')">清空本组覆写</el-button>
                  </div>
                  <el-form label-width="195px" size="small">
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-x-4">
                      <el-form-item label="find_node QPS">
                        <el-input-number
                          v-model="crawlerForm.crawl.rate_limit.max_find_node_rate_per_sec"
                          :min="1"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.find_node_rate_per_sec ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="find_node Burst">
                        <el-input-number
                          v-model="crawlerForm.crawl.rate_limit.burst"
                          :min="1"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.find_node_burst ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="max_in_flight">
                        <el-input-number
                          v-model="crawlerForm.crawl.rate_limit.max_in_flight"
                          :min="1"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.find_node_max_in_flight ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="请求超时 (secs)">
                        <el-input-number
                          v-model="crawlerForm.crawl.rate_limit.request_timeout_secs"
                          :min="1"
                          :value-on-clear="null"
                          placeholder="默认 2"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="每分钟新目标上限">
                        <el-input-number
                          v-model="crawlerForm.crawl.rate_limit.max_new_destinations_per_minute"
                          :min="1"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.max_new_destinations_per_minute ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="最大回包速率 (/s)">
                        <el-input-number
                          v-model="crawlerForm.crawl.rate_limit.max_response_rate_per_sec"
                          :min="1"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.max_response_rate_per_sec ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="最大回包带宽 (B/s)">
                        <el-input-number
                          v-model="crawlerForm.crawl.rate_limit.max_response_bytes_per_sec"
                          :min="1024"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.max_response_bytes_per_sec ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="单源回包限速 (/s)">
                        <el-input-number
                          v-model="crawlerForm.crawl.rate_limit.max_response_rate_per_source"
                          :min="1"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.max_response_rate_per_source ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="背压保底速率 (%)">
                        <el-input-number
                          v-model="crawlerForm.crawl.rate_limit.metadata_pressure_floor_percent"
                          :min="1"
                          :max="100"
                          :value-on-clear="null"
                          placeholder="默认 25"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="每分钟路由替换数">
                        <el-input-number
                          v-model="crawlerForm.crawl.rate_limit.max_replacements_per_minute"
                          :min="1"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.max_replacements_per_minute ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="单子网在途上限">
                        <el-input-number
                          v-model="crawlerForm.crawl.rate_limit.max_in_flight_per_subnet"
                          :min="1"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.max_in_flight_per_subnet ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                    </div>
                  </el-form>
                </div>

                <!-- pool -->
                <div class="rounded-lg border border-slate-200 p-3.5 space-y-2.5">
                  <div class="flex items-center justify-between">
                    <span class="font-semibold text-slate-800">crawl.pool (路由与响应节点池)</span>
                    <el-button size="small" link @click="clearL3Section('pool')">清空本组覆写</el-button>
                  </div>
                  <el-form label-width="195px" size="small">
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-x-4">
                      <el-form-item label="节点池总容量">
                        <el-input-number
                          v-model="crawlerForm.crawl.pool.capacity"
                          :min="16"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.pool_capacity ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="活跃响应池容量">
                        <el-input-number
                          v-model="crawlerForm.crawl.pool.responsive_capacity"
                          :min="16"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.responsive_capacity ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="近期探测 TTL(s)">
                        <el-input-number
                          v-model="crawlerForm.crawl.pool.recent_probe_ttl_secs"
                          :min="1"
                          :value-on-clear="null"
                          placeholder="引擎默认"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="活跃节点 TTL(s)">
                        <el-input-number
                          v-model="crawlerForm.crawl.pool.responsive_ttl_secs"
                          :min="1"
                          :value-on-clear="null"
                          placeholder="引擎默认"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="节点回访间隔(s)">
                        <el-input-number
                          v-model="crawlerForm.crawl.pool.revisit_interval_secs"
                          :min="1"
                          :value-on-clear="null"
                          placeholder="引擎默认"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="低水位触发阈值">
                        <el-input-number
                          v-model="crawlerForm.crawl.pool.low_watermark"
                          :min="0"
                          :value-on-clear="null"
                          placeholder="引擎默认"
                          class="w-full"
                        />
                      </el-form-item>
                    </div>
                  </el-form>
                </div>

                <!-- target & bootstrap & scheduler -->
                <div class="rounded-lg border border-slate-200 p-3.5 space-y-2.5">
                  <div class="flex items-center justify-between">
                    <span class="font-semibold text-slate-800">crawl.target (目标生成策略)</span>
                    <el-button size="small" link @click="clearL3Section('target')">清空本组覆写</el-button>
                  </div>
                  <el-form label-width="195px" size="small">
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-x-4">
                      <el-form-item label="随机游走比例 (%)">
                        <el-input-number
                          v-model="crawlerForm.crawl.target.random_walk_percent"
                          :min="0"
                          :max="100"
                          :value-on-clear="null"
                          placeholder="引擎默认"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="稀疏桶探测比例 (%)">
                        <el-input-number
                          v-model="crawlerForm.crawl.target.sparse_bucket_percent"
                          :min="0"
                          :max="100"
                          :value-on-clear="null"
                          placeholder="引擎默认"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="邻居仿冒 Sender ID">
                        <el-select
                          v-model="crawlerForm.crawl.target.neighbor_sender_id"
                          clearable
                          placeholder="引擎默认"
                          class="w-full"
                        >
                          <el-option label="开启 (true)" :value="true" />
                          <el-option label="关闭 (false)" :value="false" />
                        </el-select>
                      </el-form-item>
                    </div>
                  </el-form>
                </div>

                <div class="rounded-lg border border-slate-200 p-3.5 space-y-2.5">
                  <div class="flex items-center justify-between">
                    <span class="font-semibold text-slate-800">crawl.bootstrap & scheduler</span>
                    <div class="flex gap-2">
                      <el-button size="small" link @click="clearL3Section('bootstrap')">清空 Bootstrap</el-button>
                      <el-button size="small" link @click="clearL3Section('scheduler')">清空 Scheduler</el-button>
                    </div>
                  </div>
                  <el-form label-width="195px" size="small">
                    <el-form-item label="Bootstrap 节点 (每行一个)">
                      <el-input
                        v-model="bootstrapNodesText"
                        type="textarea"
                        :rows="2"
                        placeholder="留空使用内置公网节点；例如 router.bittorrent.com:6881"
                      />
                    </el-form-item>
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-x-4">
                      <el-form-item label="Bootstrap 轮询间隔(s)">
                        <el-input-number
                          v-model="crawlerForm.crawl.bootstrap.interval_secs"
                          :min="1"
                          :value-on-clear="null"
                          placeholder="引擎默认"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="每轮最大节点数">
                        <el-input-number
                          v-model="crawlerForm.crawl.bootstrap.max_nodes_per_round"
                          :min="1"
                          :value-on-clear="null"
                          placeholder="引擎默认"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="路由快照大小">
                        <el-input-number
                          v-model="crawlerForm.crawl.scheduler.routing_snapshot_size"
                          :min="8"
                          :value-on-clear="null"
                          :placeholder="`默认 ${selectedProfileInfo?.defaults.routing_snapshot_size ?? '-'}`"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="快照刷新周期 (ms)">
                        <el-input-number
                          v-model="crawlerForm.crawl.scheduler.snapshot_refresh_millis"
                          :min="50"
                          :value-on-clear="null"
                          placeholder="引擎默认"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="优先事件通道容量">
                        <el-input-number
                          v-model="crawlerForm.crawl.scheduler.priority_event_channel_capacity"
                          :min="16"
                          :value-on-clear="null"
                          placeholder="引擎默认"
                          class="w-full"
                        />
                      </el-form-item>
                      <el-form-item label="发现事件通道容量">
                        <el-input-number
                          v-model="crawlerForm.crawl.scheduler.discovery_event_channel_capacity"
                          :min="16"
                          :value-on-clear="null"
                          placeholder="引擎默认"
                          class="w-full"
                        />
                      </el-form-item>
                    </div>
                  </el-form>
                </div>
              </div>
            </el-tab-pane>

            <!-- JSON 导入/导出 -->
            <el-tab-pane name="json">
              <template #label>
                <span class="inline-flex items-center gap-1">
                  <FileCode2 class="h-3.5 w-3.5" />
                  JSON 配置 / 导入导出
                </span>
              </template>
              <div class="space-y-3">
                <div class="flex items-center justify-between gap-2">
                  <span class="text-xs text-slate-500">
                    与表单双向同步；服务端开启严格 `deny_unknown_fields` 校验，非法字段会直接拒绝。
                  </span>
                  <div class="flex items-center gap-2">
                    <el-button size="small" @click="triggerImportJson">
                      <Upload class="h-3.5 w-3.5 mr-1" />
                      导入 JSON
                    </el-button>
                    <el-button size="small" @click="exportConfigJson">
                      <Download class="h-3.5 w-3.5 mr-1" />
                      导出 JSON
                    </el-button>
                    <el-button size="small" type="primary" plain @click="applyJsonToForm">
                      同步到表单
                    </el-button>
                  </div>
                </div>
                <el-alert v-if="jsonParseError" type="error" :closable="false" :title="jsonParseError" />
                <el-input
                  v-model="jsonEditorText"
                  type="textarea"
                  :rows="18"
                  class="font-mono text-xs"
                />
              </div>
            </el-tab-pane>

            <!-- 历史版本与回滚 -->
            <el-tab-pane name="versions">
              <template #label>
                <span class="inline-flex items-center gap-1">
                  <History class="h-3.5 w-3.5" />
                  历史版本 ({{ versions.length }})
                </span>
              </template>
              <el-table :data="versions" size="small" border v-loading="versionsLoading">
                <el-table-column prop="version" label="版本" width="75">
                  <template #default="{ row }">
                    <span class="font-mono font-semibold">v{{ row.version }}</span>
                  </template>
                </el-table-column>
                <el-table-column prop="created_by" label="操作人" width="100" />
                <el-table-column prop="note" label="变更备注" min-width="160" />
                <el-table-column label="时间" width="165">
                  <template #default="{ row }">{{ formatTs(row.created_at) }}</template>
                </el-table-column>
                <el-table-column label="操作" width="110">
                  <template #default="{ row }">
                    <el-button
                      size="small"
                      type="warning"
                      link
                      :disabled="row.version === tuningVersion"
                      @click="handleRollbackVersion(row.version)"
                    >
                      {{ row.version === tuningVersion ? "当前激活" : "回滚此版本" }}
                    </el-button>
                  </template>
                </el-table-column>
              </el-table>
            </el-tab-pane>
          </el-tabs>
        </template>

        <!-- Indexer 调优面板 -->
        <template v-else-if="tuningWorker?.role === 'indexer'">
          <el-tabs v-model="tuningTab" type="border-card">
            <el-tab-pane label="Indexer 并发与批处理参数" name="indexer_core">
              <el-form label-width="195px" size="small">
                <div class="grid grid-cols-1 md:grid-cols-2 gap-x-4">
                  <el-form-item label="新种 Stream 消费者数">
                    <el-input-number v-model="indexerForm.torrent_workers" :min="1" :max="1024" class="w-full" />
                  </el-form-item>
                  <el-form-item label="Hot Stream 消费者数">
                    <el-input-number v-model="indexerForm.hot_workers" :min="1" :max="256" class="w-full" />
                  </el-form-item>
                  <el-form-item label="XREADGROUP 阻塞 (ms)">
                    <el-input-number v-model="indexerForm.read_block_ms" :min="100" :step="500" class="w-full" />
                  </el-form-item>
                  <el-form-item label="单次读取条数">
                    <el-input-number v-model="indexerForm.read_batch_size" :min="1" :max="4096" class="w-full" />
                  </el-form-item>
                  <el-form-item label="Manticore 写入并发数">
                    <el-input-number v-model="indexerForm.insert_workers" :min="1" :max="1024" class="w-full" />
                  </el-form-item>
                  <el-form-item label="批量写库条数 (batch)">
                    <el-input-number v-model="indexerForm.batch_size" :min="1" :max="10000" class="w-full" />
                  </el-form-item>
                  <el-form-item label="定时刷盘间隔 (ms)">
                    <el-input-number v-model="indexerForm.flush_interval_ms" :min="100" :step="500" class="w-full" />
                  </el-form-item>
                  <el-form-item label="新种通道缓冲">
                    <el-input-number v-model="indexerForm.channel_buffer_size" :min="10" :step="200" class="w-full" />
                  </el-form-item>
                  <el-form-item label="Hot 通道缓冲">
                    <el-input-number v-model="indexerForm.hot_channel_buffer_size" :min="10" :step="500" class="w-full" />
                  </el-form-item>
                  <el-form-item label="XAUTOCLAIM 周期(s)">
                    <el-input-number v-model="indexerForm.xautoclaim_interval_secs" :min="1" class="w-full" />
                  </el-form-item>
                  <el-form-item label="Pending 接管超时 (ms)">
                    <el-input-number v-model="indexerForm.xautoclaim_min_idle_ms" :min="1000" :step="1000" class="w-full" />
                  </el-form-item>
                  <el-form-item label="死信投递次数上限">
                    <el-input-number v-model="indexerForm.max_delivery_attempts" :min="1" :max="100" class="w-full" />
                  </el-form-item>
                  <el-form-item label="日志级别 (log_level)">
                    <el-select v-model="indexerForm.log_level" class="w-full">
                      <el-option label="error" value="error" />
                      <el-option label="warn" value="warn" />
                      <el-option label="info" value="info" />
                      <el-option label="debug" value="debug" />
                      <el-option label="trace" value="trace" />
                    </el-select>
                  </el-form-item>
                </div>
              </el-form>
            </el-tab-pane>

            <el-tab-pane label="JSON 配置 / 导入导出" name="json">
              <div class="space-y-3">
                <div class="flex items-center justify-end gap-2">
                  <el-button size="small" @click="triggerImportJson">
                    <Upload class="h-3.5 w-3.5 mr-1" />
                    导入 JSON
                  </el-button>
                  <el-button size="small" @click="exportConfigJson">
                    <Download class="h-3.5 w-3.5 mr-1" />
                    导出 JSON
                  </el-button>
                  <el-button size="small" type="primary" plain @click="applyJsonToForm">
                    同步到表单
                  </el-button>
                </div>
                <el-alert v-if="jsonParseError" type="error" :closable="false" :title="jsonParseError" />
                <el-input
                  v-model="jsonEditorText"
                  type="textarea"
                  :rows="16"
                  class="font-mono text-xs"
                />
              </div>
            </el-tab-pane>

            <el-tab-pane :label="`历史版本 (${versions.length})`" name="versions">
              <el-table :data="versions" size="small" border v-loading="versionsLoading">
                <el-table-column prop="version" label="版本" width="75">
                  <template #default="{ row }">
                    <span class="font-mono font-semibold">v{{ row.version }}</span>
                  </template>
                </el-table-column>
                <el-table-column prop="created_by" label="操作人" width="100" />
                <el-table-column prop="note" label="变更备注" min-width="160" />
                <el-table-column label="时间" width="165">
                  <template #default="{ row }">{{ formatTs(row.created_at) }}</template>
                </el-table-column>
                <el-table-column label="操作" width="110">
                  <template #default="{ row }">
                    <el-button
                      size="small"
                      type="warning"
                      link
                      :disabled="row.version === tuningVersion"
                      @click="handleRollbackVersion(row.version)"
                    >
                      {{ row.version === tuningVersion ? "当前激活" : "回滚此版本" }}
                    </el-button>
                  </template>
                </el-table-column>
              </el-table>
            </el-tab-pane>
          </el-tabs>
        </template>
      </div>

      <!-- 抽屉底部保存栏 -->
      <template #footer>
        <div class="flex flex-wrap items-center justify-between gap-3 w-full">
          <el-input
            v-model="tuningNote"
            size="small"
            placeholder="变更备注（可选，例如：提升 P6 档位并发）"
            class="flex-1 min-w-[200px]"
          />
          <div class="flex items-center gap-2">
            <el-button size="small" @click="tuningVisible = false">关闭</el-button>
            <el-button
              size="small"
              :loading="tuningSaving"
              @click="saveTuningConfig(false)"
            >
              <Save class="h-3.5 w-3.5 mr-1" />
              仅保存配置
            </el-button>
            <el-button
              size="small"
              type="primary"
              :loading="tuningSaving"
              :disabled="!tuningWorker || (tuningWorker.runtime_state !== 'running' && tuningWorker.runtime_state !== 'starting')"
              @click="saveTuningConfig(true)"
            >
              <RotateCcw class="h-3.5 w-3.5 mr-1" />
              保存并平滑重启
            </el-button>
          </div>
        </div>
      </template>
    </el-drawer>
  </div>
</template>
