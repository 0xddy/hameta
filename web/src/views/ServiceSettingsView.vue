<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import { History, RotateCcw, Save, Zap } from "lucide-vue-next";
import { ApiError } from "@/api/client";
import {
  getServicesSettings,
  initManticoreTable,
  listSettingsHistory,
  rollbackSettings,
  testManticoreSettings,
  testRedisSettings,
  updateManticoreSettings,
  updateRedisSettings,
  type AffectedWorkerItem,
  type ServiceHistoryItem,
  type WorkerRestartOutcome,
} from "@/api/settings";
import {
  defaultManticoreConfig,
  defaultRedisConfig,
  type ManticoreServiceConfig,
  type ManticoreTestReport,
  type RedisServiceConfig,
  type RedisTestReport,
} from "@/api/setup";
import ManticoreConfigFields from "@/components/ManticoreConfigFields.vue";
import RedisConfigFields from "@/components/RedisConfigFields.vue";

const activeTab = ref<"redis" | "manticore" | "history">("redis");
const loading = ref(true);

const redisMeta = reactive({
  version: 0,
  last_test_ok: null as boolean | null,
  last_test_message: null as string | null,
  updated_by: "",
  note: "",
  affected: [] as AffectedWorkerItem[],
});
const redisForm = ref<RedisServiceConfig>(defaultRedisConfig());
const redisNote = ref("");
const redisReport = ref<RedisTestReport | null>(null);
const testingRedis = ref(false);
const savingRedis = ref(false);

const manticoreMeta = reactive({
  version: 0,
  last_test_ok: null as boolean | null,
  last_test_message: null as string | null,
  updated_by: "",
  note: "",
  affected: [] as AffectedWorkerItem[],
});
const manticoreForm = ref<ManticoreServiceConfig>(defaultManticoreConfig());
const manticoreNote = ref("");
const manticoreReport = ref<ManticoreTestReport | null>(null);
const testingManticore = ref(false);
const savingManticore = ref(false);
const creatingTable = ref(false);

const restartOutcomes = ref<WorkerRestartOutcome[]>([]);

const historyKind = ref<"redis" | "manticore">("redis");
const historyRows = ref<ServiceHistoryItem[]>([]);
const loadingHistory = ref(false);
const snapshotDialogVisible = ref(false);
const selectedSnapshotJson = ref("");

const currentHistoryKind = computed(() => historyKind.value);

async function loadOverview() {
  loading.value = true;
  try {
    const res = await getServicesSettings();
    redisForm.value = { ...res.redis.config };
    redisMeta.version = res.redis.version;
    redisMeta.last_test_ok = res.redis.last_test_ok;
    redisMeta.last_test_message = res.redis.last_test_message;
    redisMeta.updated_by = res.redis.updated_by;
    redisMeta.note = res.redis.note;
    redisMeta.affected = res.redis.affected_running_workers;

    manticoreForm.value = { ...res.manticore.config };
    manticoreMeta.version = res.manticore.version;
    manticoreMeta.last_test_ok = res.manticore.last_test_ok;
    manticoreMeta.last_test_message = res.manticore.last_test_message;
    manticoreMeta.updated_by = res.manticore.updated_by;
    manticoreMeta.note = res.manticore.note;
    manticoreMeta.affected = res.manticore.affected_running_workers;
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "加载服务配置失败");
  } finally {
    loading.value = false;
  }
}

async function loadHistory() {
  loadingHistory.value = true;
  try {
    historyRows.value = await listSettingsHistory(currentHistoryKind.value, 30);
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "加载变更历史失败");
  } finally {
    loadingHistory.value = false;
  }
}

watch([activeTab, historyKind], ([tab]) => {
  if (tab === "history") {
    void loadHistory();
  }
});

onMounted(() => {
  void loadOverview();
});

async function handleTestRedis() {
  testingRedis.value = true;
  try {
    redisReport.value = await testRedisSettings(redisForm.value);
    if (redisReport.value.ok) {
      ElMessage.success(redisReport.value.message);
    } else {
      ElMessage.error(redisReport.value.message);
    }
    await loadOverview();
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "测试 Redis 连接失败");
  } finally {
    testingRedis.value = false;
  }
}

async function handleSaveRedis(withRestart: boolean) {
  if (withRestart && redisMeta.affected.length > 0) {
    const names = redisMeta.affected.map((w) => `${w.id} (${w.role})`).join("、");
    try {
      await ElMessageBox.confirm(
        `保存新版 Redis 配置后，将依次优雅排空并重启以下运行中的节点：${names}。是否继续？`,
        "确认滚动重启受影响 Worker",
        { type: "warning", confirmButtonText: "保存并滚动重启", cancelButtonText: "取消" },
      );
    } catch {
      return;
    }
  }

  savingRedis.value = true;
  restartOutcomes.value = [];
  try {
    const res = await updateRedisSettings(redisForm.value, redisNote.value, withRestart);
    restartOutcomes.value = res.restarted_workers;
    ElMessage.success(`已保存 Redis 配置 v${res.version}`);
    redisNote.value = "";
    await loadOverview();
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "保存 Redis 配置失败");
  } finally {
    savingRedis.value = false;
  }
}

async function handleTestManticore() {
  testingManticore.value = true;
  try {
    manticoreReport.value = await testManticoreSettings(manticoreForm.value);
    if (manticoreReport.value.ok) {
      ElMessage.success(manticoreReport.value.message);
    } else {
      ElMessage.warning(manticoreReport.value.message);
    }
    await loadOverview();
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "测试 Manticore 失败");
  } finally {
    testingManticore.value = false;
  }
}

async function handleInitManticoreTable() {
  try {
    await ElMessageBox.confirm(
      "确认为当前空 Manticore 实例初始化 torrents 表？若表已存在，系统将自动拒绝并保护存量数据。",
      "显式初始化建表确认",
      { type: "warning", confirmButtonText: "确认建表", cancelButtonText: "取消" },
    );
  } catch {
    return;
  }

  creatingTable.value = true;
  try {
    const res = await initManticoreTable(manticoreForm.value);
    ElMessage.success(res.message);
    await handleTestManticore();
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "初始化建表失败");
  } finally {
    creatingTable.value = false;
  }
}

async function handleSaveManticore(withRestart: boolean) {
  if (withRestart && manticoreMeta.affected.length > 0) {
    const names = manticoreMeta.affected.map((w) => w.id).join("、");
    try {
      await ElMessageBox.confirm(
        `保存新版 Manticore 配置后，将依次排空并重启以下运行中的索引节点：${names}。是否继续？`,
        "确认滚动重启 Indexer",
        { type: "warning", confirmButtonText: "保存并滚动重启", cancelButtonText: "取消" },
      );
    } catch {
      return;
    }
  }

  savingManticore.value = true;
  restartOutcomes.value = [];
  try {
    const res = await updateManticoreSettings(manticoreForm.value, manticoreNote.value, withRestart);
    restartOutcomes.value = res.restarted_workers;
    ElMessage.success(`已保存 Manticore 配置 v${res.version}`);
    manticoreNote.value = "";
    await loadOverview();
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "保存 Manticore 配置失败");
  } finally {
    savingManticore.value = false;
  }
}

function viewSnapshot(row: ServiceHistoryItem) {
  selectedSnapshotJson.value = JSON.stringify(row.config, null, 2);
  snapshotDialogVisible.value = true;
}

async function handleRollback(row: ServiceHistoryItem) {
  let restart = false;
  try {
    await ElMessageBox.confirm(
      `确认将 ${row.kind.toUpperCase()} 回滚至 v${row.version}（将生成新的激活版本号）？`,
      "确认回滚配置版本",
      {
        type: "warning",
        distinguishCancelAndClose: true,
        confirmButtonText: "回滚并重启受影响节点",
        cancelButtonText: "仅回滚配置",
      },
    );
    restart = true;
  } catch (action) {
    if (action !== "cancel") {
      return;
    }
    restart = false;
  }

  try {
    const res = await rollbackSettings(row.kind, row.version, restart);
    restartOutcomes.value = res.restarted_workers;
    ElMessage.success(`已回滚 ${row.kind} 并生成激活版本 v${res.version}`);
    await loadOverview();
    await loadHistory();
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "回滚失败");
  }
}
</script>

<template>
  <div class="max-w-5xl mx-auto space-y-6">
    <div v-if="restartOutcomes.length > 0" class="rounded-xl border border-slate-200 bg-white p-4">
      <div class="text-sm font-medium text-slate-800 mb-2">联动滚动重启结果</div>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-2">
        <el-alert
          v-for="item in restartOutcomes"
          :key="item.id"
          :type="item.ok ? 'success' : 'error'"
          :title="`${item.id} (${item.role}): ${item.message}`"
          :closable="false"
          show-icon
        />
      </div>
    </div>

    <div class="rounded-xl border border-slate-200 bg-white p-6 shadow-sm" v-loading="loading">
      <el-tabs v-model="activeTab">
        <!-- Redis Tab -->
        <el-tab-pane label="Redis 服务配置" name="redis">
          <div class="mb-4 flex flex-wrap items-center justify-between gap-2 rounded-lg bg-slate-50 px-4 py-2.5 text-xs text-slate-600">
            <div class="flex flex-wrap gap-4">
              <span>当前版本: <strong class="font-mono">v{{ redisMeta.version }}</strong></span>
              <span v-if="redisMeta.updated_by">修改人: {{ redisMeta.updated_by }}</span>
              <span v-if="redisMeta.last_test_message">上次检测: {{ redisMeta.last_test_message }}</span>
            </div>
            <div>
              运行中受影响节点:
              <strong class="text-indigo-600">{{ redisMeta.affected.length }}</strong>
              <span v-if="redisMeta.affected.length > 0" class="ml-1 text-slate-500">
                ({{ redisMeta.affected.map((w) => w.id).join(", ") }})
              </span>
            </div>
          </div>

          <el-form label-position="top">
            <RedisConfigFields v-model="redisForm" :test-report="redisReport" />

            <el-form-item label="本次变更备注" class="mt-4">
              <el-input v-model="redisNote" placeholder="例如：更换主库 Redis 密码 / 调整连接池大小" />
            </el-form-item>

            <div class="mt-4 flex flex-wrap justify-end gap-3">
              <el-button :loading="testingRedis" @click="handleTestRedis">
                <Zap class="h-4 w-4 mr-1" />
                一键测试连接
              </el-button>
              <el-button type="primary" plain :loading="savingRedis" @click="handleSaveRedis(false)">
                <Save class="h-4 w-4 mr-1" />
                仅保存配置
              </el-button>
              <el-button
                type="primary"
                :loading="savingRedis"
                @click="handleSaveRedis(true)"
              >
                <RotateCcw class="h-4 w-4 mr-1" />
                保存并滚动重启受影响节点 ({{ redisMeta.affected.length }})
              </el-button>
            </div>
          </el-form>
        </el-tab-pane>

        <!-- Manticore Tab -->
        <el-tab-pane label="Manticore Search 配置" name="manticore">
          <div class="mb-4 flex flex-wrap items-center justify-between gap-2 rounded-lg bg-slate-50 px-4 py-2.5 text-xs text-slate-600">
            <div class="flex flex-wrap gap-4">
              <span>当前版本: <strong class="font-mono">v{{ manticoreMeta.version }}</strong></span>
              <span v-if="manticoreMeta.updated_by">修改人: {{ manticoreMeta.updated_by }}</span>
              <span v-if="manticoreMeta.last_test_message">上次检测: {{ manticoreMeta.last_test_message }}</span>
            </div>
            <div>
              运行中受影响索引节点:
              <strong class="text-indigo-600">{{ manticoreMeta.affected.length }}</strong>
              <span v-if="manticoreMeta.affected.length > 0" class="ml-1 text-slate-500">
                ({{ manticoreMeta.affected.map((w) => w.id).join(", ") }})
              </span>
            </div>
          </div>

          <el-form label-position="top">
            <ManticoreConfigFields v-model="manticoreForm" :test-report="manticoreReport" />

            <el-form-item label="本次变更备注" class="mt-4">
              <el-input v-model="manticoreNote" placeholder="例如：切换新 Manticore 实例 / 调整连接上限" />
            </el-form-item>

            <div class="mt-4 flex flex-wrap justify-end gap-3">
              <el-button
                v-if="manticoreReport?.schema && !manticoreReport.schema.table_exists"
                type="warning"
                :loading="creatingTable"
                @click="handleInitManticoreTable"
              >
                初始化新建 torrents 表（仅空库）
              </el-button>
              <el-button :loading="testingManticore" @click="handleTestManticore">
                <Zap class="h-4 w-4 mr-1" />
                测试连接并核验表结构
              </el-button>
              <el-button
                type="primary"
                plain
                :loading="savingManticore"
                @click="handleSaveManticore(false)"
              >
                <Save class="h-4 w-4 mr-1" />
                仅保存配置
              </el-button>
              <el-button
                type="primary"
                :loading="savingManticore"
                @click="handleSaveManticore(true)"
              >
                <RotateCcw class="h-4 w-4 mr-1" />
                保存并滚动重启受影响 Indexer ({{ manticoreMeta.affected.length }})
              </el-button>
            </div>
          </el-form>
        </el-tab-pane>

        <!-- History Tab -->
        <el-tab-pane name="history">
          <template #label>
            <span class="inline-flex items-center gap-1">
              <History class="h-3.5 w-3.5" />
              历史版本与回滚
            </span>
          </template>

          <div class="mb-4 flex items-center justify-between">
            <el-radio-group v-model="historyKind" size="small">
              <el-radio-button value="redis">Redis 配置历史</el-radio-button>
              <el-radio-button value="manticore">Manticore 配置历史</el-radio-button>
            </el-radio-group>
            <el-button size="small" @click="loadHistory">刷新列表</el-button>
          </div>

          <el-table :data="historyRows" v-loading="loadingHistory" stripe size="small">
            <el-table-column label="版本" width="90">
              <template #default="{ row }">
                <span class="font-mono font-semibold">v{{ row.version }}</span>
                <el-tag v-if="row.is_active" size="small" type="success" class="ml-1">当前</el-tag>
              </template>
            </el-table-column>
            <el-table-column prop="created_by" label="操作人" width="110" />
            <el-table-column prop="note" label="变更备注" min-width="180" />
            <el-table-column label="检测结论" min-width="180">
              <template #default="{ row }">
                <span v-if="row.last_test_message" class="text-xs">{{ row.last_test_message }}</span>
                <span v-else class="text-xs text-slate-400">未记录</span>
              </template>
            </el-table-column>
            <el-table-column label="时间" width="165">
              <template #default="{ row }">
                {{ new Date(row.created_at * 1000).toLocaleString() }}
              </template>
            </el-table-column>
            <el-table-column label="操作" width="170" fixed="right">
              <template #default="{ row }">
                <el-button link type="primary" size="small" @click="viewSnapshot(row)">
                  查看完整参数
                </el-button>
                <el-button
                  v-if="!row.is_active"
                  link
                  type="warning"
                  size="small"
                  @click="handleRollback(row)"
                >
                  回滚
                </el-button>
              </template>
            </el-table-column>
          </el-table>
        </el-tab-pane>
      </el-tabs>
    </div>

    <el-dialog v-model="snapshotDialogVisible" title="完整配置版本快照（所见即所得）" width="600px">
      <pre class="rounded-lg bg-slate-900 text-slate-100 p-4 text-xs overflow-auto max-h-96 font-mono">{{ selectedSnapshotJson }}</pre>
    </el-dialog>
  </div>
</template>
