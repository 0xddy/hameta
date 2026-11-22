<script setup lang="ts">
import { onMounted, reactive, ref } from "vue";
import {
  AlertTriangle,
  Database,
  Eye,
  KeyRound,
  RefreshCw,
  Scissors,
  Search,
  Trash2,
} from "lucide-vue-next";
import { ElMessage, ElMessageBox } from "element-plus";
import {
  clearRedisHashLocks,
  getRedisOverview,
  inspectRedisHash,
  previewRedisStream,
  trimRedisStream,
  type PreviewStreamResponse,
  type RedisHashInspectResponse,
  type RedisOverviewResponse,
  type RedisStreamOverviewDto,
} from "@/api/redis";

const loading = ref(true);
const overview = ref<RedisOverviewResponse | null>(null);

// 预览抽屉
const previewVisible = ref(false);
const previewLoading = ref(false);
const previewStreamTarget = ref<RedisStreamOverviewDto | null>(null);
const previewCount = ref(20);
const previewData = ref<PreviewStreamResponse | null>(null);

// 受控 XTRIM 弹窗
const trimVisible = ref(false);
const trimSubmitting = ref(false);
const trimTarget = ref<RedisStreamOverviewDto | null>(null);
const trimForm = reactive({
  max_len: 100000,
  approximate: true,
  confirm_stream_name: "",
});

// 单 InfoHash 诊断工具
const hashInput = ref("");
const hashLoading = ref(false);
const hashReport = ref<RedisHashInspectResponse | null>(null);
const clearOpts = reactive({
  clear_pending: true,
  clear_rejected: true,
  clear_indexed: false,
});
const clearingLocks = ref(false);

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

function formatPttl(pttlMs: number): string {
  if (pttlMs === -2) return "键不存在";
  if (pttlMs === -1) return "永久 (-1)";
  if (pttlMs < 1000) return `${pttlMs} ms`;
  return `${Math.round(pttlMs / 1000)} s`;
}

function formatTs(ts: number | null | undefined): string {
  if (!ts) return "-";
  return new Date(ts * 1000).toLocaleString("zh-CN", { hour12: false });
}

async function fetchOverview() {
  loading.value = true;
  try {
    overview.value = await getRedisOverview();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "加载 Redis 状态总览失败");
  } finally {
    loading.value = false;
  }
}

async function openPreviewDrawer(stream: RedisStreamOverviewDto) {
  previewStreamTarget.value = stream;
  previewVisible.value = true;
  await loadPreview();
}

async function loadPreview() {
  if (!previewStreamTarget.value) return;
  previewLoading.value = true;
  try {
    previewData.value = await previewRedisStream(
      previewStreamTarget.value.role,
      previewCount.value,
    );
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "读取 Stream 预览失败");
  } finally {
    previewLoading.value = false;
  }
}

function openTrimDialog(stream: RedisStreamOverviewDto) {
  trimTarget.value = stream;
  trimForm.max_len = Math.min(stream.length, 100000);
  trimForm.approximate = true;
  trimForm.confirm_stream_name = "";
  trimVisible.value = true;
}

async function submitTrim() {
  if (!trimTarget.value) return;
  if (trimForm.confirm_stream_name.trim() !== trimTarget.value.stream_name) {
    ElMessage.warning(`请准确输入 Stream 键名 "${trimTarget.value.stream_name}" 确认修剪`);
    return;
  }
  trimSubmitting.value = true;
  try {
    const res = await trimRedisStream(trimTarget.value.role, {
      max_len: trimForm.max_len,
      approximate: trimForm.approximate,
      confirm_stream_name: trimForm.confirm_stream_name.trim(),
    });
    ElMessage.success(`XTRIM 完成：裁掉 ${res.trimmed_count} 条，剩余 ${res.remaining_len} 条`);
    trimVisible.value = false;
    await fetchOverview();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "修剪 Stream 失败");
  } finally {
    trimSubmitting.value = false;
  }
}

async function handleInspectHash() {
  const h = hashInput.value.trim();
  if (!/^[0-9a-fA-F]{40}$/.test(h)) {
    ElMessage.warning("请输入 40 位十六进制 InfoHash");
    return;
  }
  hashLoading.value = true;
  try {
    hashReport.value = await inspectRedisHash(h);
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "诊断 InfoHash 失败");
  } finally {
    hashLoading.value = false;
  }
}

async function handleClearHashLocks() {
  if (!hashReport.value) return;
  if (!clearOpts.clear_pending && !clearOpts.clear_rejected && !clearOpts.clear_indexed) {
    ElMessage.warning("请至少勾选一项要清理的 Redis 标记");
    return;
  }
  try {
    await ElMessageBox.confirm(
      `确认对 ${hashReport.value.info_hash} 执行所选 Redis 标记清理吗？${
        clearOpts.clear_indexed ? "（注意：已勾选清除 metadata:indexed 去重标记，爬虫遇到后将重新抓取）" : ""
      }`,
      "清理确认",
      { type: "warning", confirmButtonText: "确认清理", cancelButtonText: "取消" },
    );
  } catch {
    return;
  }
  clearingLocks.value = true;
  try {
    const res = await clearRedisHashLocks(hashReport.value.info_hash, clearOpts);
    ElMessage.success(`已清理 ${res.deleted_count} 个 Redis 键`);
    await handleInspectHash();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "清理失败");
  } finally {
    clearingLocks.value = false;
  }
}

onMounted(() => {
  void fetchOverview();
});
</script>

<template>
  <div class="max-w-7xl mx-auto space-y-5" v-loading="loading">
    <!-- 顶部栏 -->
    <div class="rounded-xl border border-slate-200 bg-white px-5 py-4 shadow-sm flex flex-wrap items-center justify-between gap-3">
      <div>
        <h2 class="text-base font-semibold text-slate-900">Redis Streams 与去重锁监控中心</h2>
        <p class="text-xs text-slate-500 mt-0.5">
          观测连接池与内存状态、双 Stream (`new` / `hot`) 长度/Lag/PEL 消费者详情，支持队尾样例预览、受控修剪及单 InfoHash 全链路排查
        </p>
      </div>
      <el-button size="small" :loading="loading" @click="fetchOverview">
        <RefreshCw class="h-3.5 w-3.5 mr-1" />
        刷新状态
      </el-button>
    </div>

    <el-alert
      v-if="overview && !overview.ok"
      type="error"
      show-icon
      :closable="false"
      :title="overview.message || 'Redis 连通异常'"
    />

    <!-- 中间件状态 KPI -->
    <div v-if="overview" class="grid grid-cols-2 sm:grid-cols-4 gap-4">
      <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
        <div class="text-xs text-slate-500">服务状态与 PING 延迟</div>
        <div class="text-lg font-bold font-mono text-slate-900">
          {{ overview.ok ? `${overview.latency_ms} ms` : "异常" }}
        </div>
        <div class="text-[11px] text-slate-500 font-mono">
          Redis v{{ overview.redis_version ?? "-" }} · 运行 {{ overview.uptime_secs ?? "-" }}s
        </div>
      </div>

      <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
        <div class="text-xs text-slate-500">内存使用 & 峰值</div>
        <div class="text-lg font-bold font-mono text-slate-900">
          {{ overview.used_memory_human ?? "-" }}
        </div>
        <div class="text-[11px] text-slate-500 font-mono">
          峰值: {{ overview.used_memory_peak_human ?? "-" }} · DBSIZE={{ overview.dbsize?.toLocaleString("zh-CN") ?? "-" }}
        </div>
      </div>

      <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
        <div class="text-xs text-slate-500">客户端连接 & OPS</div>
        <div class="text-lg font-bold font-mono text-slate-900">
          {{ overview.instantaneous_ops_per_sec ?? 0 }} ops/s
        </div>
        <div class="text-[11px] text-slate-500 font-mono">
          活跃客户端: {{ overview.connected_clients ?? "-" }}
        </div>
      </div>

      <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm space-y-1">
        <div class="text-xs text-slate-500">Panel Redis 连接池</div>
        <div class="text-lg font-bold font-mono text-slate-900">
          {{ overview.pool.available }} / {{ overview.pool.max_size }} 可用
        </div>
        <div class="text-[11px] text-slate-500 font-mono">
          已建={{ overview.pool.size }} · 等待={{ overview.pool.waiting }}
        </div>
      </div>
    </div>

    <!-- 双 Streams 队列观测卡片 -->
    <div v-if="overview?.ok" class="grid grid-cols-1 lg:grid-cols-2 gap-5">
      <div
        v-for="stream in overview.streams"
        :key="stream.stream_name"
        class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-4"
      >
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div class="space-y-0.5">
            <div class="flex items-center gap-2">
              <Database class="h-4 w-4 text-rose-500" />
              <span class="font-semibold text-sm text-slate-900">
                {{ stream.role === "new" ? "新种子待入库流 (new)" : "热度增量更新流 (hot)" }}
              </span>
              <el-tag size="small" :type="stream.exists ? 'success' : 'info'" effect="plain">
                {{ stream.exists ? "活跃存在" : "尚未写入" }}
              </el-tag>
            </div>
            <div class="font-mono text-xs text-slate-500">{{ stream.stream_name }}</div>
          </div>

          <div class="flex items-center gap-2">
            <el-button
              size="small"
              plain
              :disabled="!stream.exists"
              @click="openPreviewDrawer(stream)"
            >
              <Eye class="h-3.5 w-3.5 mr-1" />
              预览队尾
            </el-button>
            <el-button
              size="small"
              type="danger"
              plain
              :disabled="!stream.exists"
              @click="openTrimDialog(stream)"
            >
              <Scissors class="h-3.5 w-3.5 mr-1" />
              受控修剪
            </el-button>
          </div>
        </div>

        <div class="grid grid-cols-3 gap-3 text-xs rounded-lg bg-slate-50 border border-slate-100 p-3 font-mono">
          <div>
            <div class="text-slate-400">当前长度 (XLEN)</div>
            <div class="text-base font-bold text-slate-900">{{ stream.length.toLocaleString("zh-CN") }}</div>
          </div>
          <div>
            <div class="text-slate-400">累计入队</div>
            <div class="text-base font-bold text-slate-900">{{ stream.entries_added?.toLocaleString("zh-CN") ?? "-" }}</div>
          </div>
          <div>
            <div class="text-slate-400">末条 ID</div>
            <div class="text-xs text-slate-700 truncate" :title="stream.last_entry_id ?? ''">
              {{ stream.last_entry_id ?? "-" }}
            </div>
          </div>
        </div>

        <!-- 消费组状态 -->
        <div class="space-y-1.5">
          <div class="text-xs font-medium text-slate-700">消费组 (Consumer Groups)</div>
          <el-table :data="stream.groups" size="small" border empty-text="暂无消费组（等待 Indexer 初始化）">
            <el-table-column prop="name" label="组名" min-width="140">
              <template #default="{ row }"><span class="font-mono text-xs">{{ row.name }}</span></template>
            </el-table-column>
            <el-table-column prop="consumers" label="消费者" width="70" />
            <el-table-column prop="pending" label="PEL未确认" width="90">
              <template #default="{ row }">
                <span :class="row.pending > 0 ? 'text-amber-600 font-semibold' : ''">{{ row.pending }}</span>
              </template>
            </el-table-column>
            <el-table-column label="Lag滞后" width="85">
              <template #default="{ row }">
                <span :class="(row.lag ?? 0) > 1000 ? 'text-rose-600 font-semibold' : ''">{{ row.lag ?? "-" }}</span>
              </template>
            </el-table-column>
          </el-table>
        </div>

        <!-- 活跃消费者列表 -->
        <div v-if="stream.target_group_consumers.length > 0" class="space-y-1.5">
          <div class="text-xs font-medium text-slate-700">
            目标组消费者明细 (`{{ overview.config_summary.consumer_group }}`)
          </div>
          <el-table :data="stream.target_group_consumers" size="small" border max-height="180">
            <el-table-column prop="name" label="消费者实例" min-width="150">
              <template #default="{ row }"><span class="font-mono text-xs">{{ row.name }}</span></template>
            </el-table-column>
            <el-table-column prop="pending" label="持有Pending" width="100" />
            <el-table-column label="空闲时长" width="100">
              <template #default="{ row }">{{ Math.round(row.idle_ms / 1000) }}s</template>
            </el-table-column>
          </el-table>
        </div>
      </div>
    </div>

    <!-- 单 InfoHash 状态诊断工具 -->
    <div class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h3 class="text-sm font-semibold text-slate-900 flex items-center gap-2">
            <KeyRound class="h-4 w-4 text-indigo-600" />
            单 InfoHash 状态全链路诊断
          </h3>
          <p class="text-xs text-slate-500 mt-0.5">
            联查 Redis `metadata:indexed` / `pending` 租约锁 / `rejected` 超大拒绝缓存及 Manticore 存量状态，定位未入库或被去重原因
          </p>
        </div>
        <div class="flex items-center gap-2 flex-1 max-w-md">
          <el-input
            v-model="hashInput"
            size="small"
            clearable
            placeholder="输入 40 位 info_hash (十六进制)..."
            class="font-mono"
            @keyup.enter="handleInspectHash"
          />
          <el-button size="small" type="primary" :loading="hashLoading" @click="handleInspectHash">
            <Search class="h-3.5 w-3.5 mr-1" />
            诊断查询
          </el-button>
        </div>
      </div>

      <div v-if="hashReport" class="space-y-4">
        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-3 text-xs">
          <div class="rounded-lg border border-slate-200 p-3 space-y-1">
            <div class="flex justify-between items-center">
              <span class="font-semibold text-slate-800">已入库去重标记</span>
              <el-tag size="small" :type="hashReport.indexed.exists ? 'success' : 'info'">
                {{ hashReport.indexed.exists ? "已存在" : "未置位" }}
              </el-tag>
            </div>
            <div class="font-mono text-[11px] text-slate-500 break-all">{{ hashReport.indexed.key }}</div>
            <div class="font-mono text-slate-700">
              值={{ hashReport.indexed.value ?? "-" }} · TTL={{ formatPttl(hashReport.indexed.pttl_ms) }}
            </div>
          </div>

          <div class="rounded-lg border border-slate-200 p-3 space-y-1">
            <div class="flex justify-between items-center">
              <span class="font-semibold text-slate-800">抓取中租约锁 (Pending)</span>
              <el-tag size="small" :type="hashReport.pending.exists ? 'warning' : 'info'">
                {{ hashReport.pending.exists ? "占用中" : "空闲" }}
              </el-tag>
            </div>
            <div class="font-mono text-[11px] text-slate-500 break-all">{{ hashReport.pending.key }}</div>
            <div class="font-mono text-slate-700">
              Token={{ hashReport.pending.value ?? "-" }} · TTL={{ formatPttl(hashReport.pending.pttl_ms) }}
            </div>
          </div>

          <div class="rounded-lg border border-slate-200 p-3 space-y-1">
            <div class="flex justify-between items-center">
              <span class="font-semibold text-slate-800">超大拒绝缓存 (Rejected)</span>
              <el-tag size="small" :type="hashReport.rejected.exists ? 'danger' : 'info'">
                {{ hashReport.rejected.exists ? "已拦截" : "未命中" }}
              </el-tag>
            </div>
            <div class="font-mono text-[11px] text-slate-500 break-all">{{ hashReport.rejected.key }}</div>
            <div class="font-mono text-slate-700">
              体积={{ hashReport.rejected.value ?? "-" }} · TTL={{ formatPttl(hashReport.rejected.pttl_ms) }}
            </div>
          </div>

          <div class="rounded-lg border border-slate-200 p-3 space-y-1">
            <div class="flex justify-between items-center">
              <span class="font-semibold text-slate-800">Manticore 存在性</span>
              <el-tag
                size="small"
                :type="!hashReport.manticore_checked ? 'info' : hashReport.manticore_exists ? 'success' : 'warning'"
              >
                {{
                  !hashReport.manticore_checked
                    ? "未检查"
                    : hashReport.manticore_exists
                      ? "存在记录"
                      : "无记录"
                }}
              </el-tag>
            </div>
            <div class="font-mono text-[11px] text-slate-500">ID={{ hashReport.expected_id }}</div>
            <div v-if="hashReport.manticore_exists" class="text-[11px] text-slate-700 truncate">
              {{ hashReport.manticore_refine_name || hashReport.manticore_raw_name }} (hot={{ hashReport.manticore_hot }}, {{ formatTs(hashReport.manticore_updated_at) }})
            </div>
            <div v-else class="text-[11px] text-slate-400">未检索到 Manticore 索引文档</div>
          </div>
        </div>

        <div class="flex flex-wrap items-center justify-between gap-3 rounded-lg bg-slate-50 border border-slate-200 px-4 py-2.5 text-xs">
          <div class="flex flex-wrap items-center gap-4">
            <el-checkbox v-model="clearOpts.clear_pending" size="small">清理 pending 租约锁</el-checkbox>
            <el-checkbox v-model="clearOpts.clear_rejected" size="small">清理 rejected 超大缓存</el-checkbox>
            <el-checkbox v-model="clearOpts.clear_indexed" size="small">
              <span class="text-rose-700 font-medium">同步移除 metadata:indexed 去重标记（高危）</span>
            </el-checkbox>
          </div>
          <el-button
            size="small"
            type="danger"
            plain
            :loading="clearingLocks"
            @click="handleClearHashLocks"
          >
            <Trash2 class="h-3.5 w-3.5 mr-1" />
            清理所选 Redis 标记
          </el-button>
        </div>
      </div>
    </div>

    <!-- 队尾预览抽屉 -->
    <el-drawer
      v-model="previewVisible"
      :title="`Stream 队尾样例预览 (${previewStreamTarget?.stream_name ?? ''})`"
      size="760px"
    >
      <div class="space-y-4" v-loading="previewLoading">
        <div class="flex items-center justify-between text-xs">
          <span>当前总长度: <strong class="font-mono">{{ previewData?.total_len ?? 0 }}</strong></span>
          <div class="flex items-center gap-2">
            <span>预览条数:</span>
            <el-select v-model="previewCount" size="small" style="width: 85px" @change="loadPreview">
              <el-option :value="10" label="10" />
              <el-option :value="20" label="20" />
              <el-option :value="50" label="50" />
              <el-option :value="100" label="100" />
            </el-select>
            <el-button size="small" @click="loadPreview">刷新</el-button>
          </div>
        </div>

        <el-table :data="previewData?.items ?? []" size="small" border stripe>
          <el-table-column prop="entry_id" label="Entry ID" width="145">
            <template #default="{ row }"><span class="font-mono text-xs">{{ row.entry_id }}</span></template>
          </el-table-column>
          <el-table-column prop="info_hash" label="InfoHash" width="150" show-overflow-tooltip>
            <template #default="{ row }"><span class="font-mono text-xs">{{ row.info_hash ?? "-" }}</span></template>
          </el-table-column>
          <el-table-column label="标题 / 载荷" min-width="220" show-overflow-tooltip>
            <template #default="{ row }">
              <span v-if="row.name" class="text-xs text-slate-800">{{ row.name }}</span>
              <span v-else class="font-mono text-[11px] text-slate-500">{{ JSON.stringify(row.fields) }}</span>
            </template>
          </el-table-column>
          <el-table-column label="体积 / 文件数" width="120">
            <template #default="{ row }">
              <span class="font-mono text-xs">
                {{ row.total_size !== null ? formatBytes(row.total_size) : "-" }} / {{ row.file_count ?? "-" }}
              </span>
            </template>
          </el-table-column>
        </el-table>
      </div>
    </el-drawer>

    <!-- 高危受控 XTRIM 对话框 -->
    <el-dialog v-model="trimVisible" title="受控修剪 Redis Stream (XTRIM MAXLEN)" width="480px">
      <div class="space-y-3 text-xs">
        <div class="rounded-lg border border-rose-200 bg-rose-50 p-3 flex items-start gap-2 text-rose-800">
          <AlertTriangle class="h-4 w-4 text-rose-600 shrink-0 mt-0.5" />
          <div>
            高危操作警告：XTRIM 会从 Stream 头部丢弃超出 `max_len` 的旧消息，可能包含尚未确认消费的记录，仅建议在队列严重积压导致内存告警时执行。
          </div>
        </div>

        <el-form label-width="130px" size="small">
          <el-form-item label="目标 Stream">
            <span class="font-mono font-semibold text-slate-900">{{ trimTarget?.stream_name }}</span>
          </el-form-item>
          <el-form-item label="保留最大条数">
            <el-input-number v-model="trimForm.max_len" :min="0" :step="1000" class="w-full" />
          </el-form-item>
          <el-form-item label="近似裁剪 (~)">
            <el-switch v-model="trimForm.approximate" active-text="高效近似 (~)" inactive-text="精确裁剪" />
          </el-form-item>
          <el-form-item label="确认输入 Stream 名">
            <el-input
              v-model="trimForm.confirm_stream_name"
              :placeholder="trimTarget?.stream_name"
              class="font-mono"
            />
          </el-form-item>
        </el-form>
      </div>
      <template #footer>
        <el-button size="small" @click="trimVisible = false">取消</el-button>
        <el-button size="small" type="danger" :loading="trimSubmitting" @click="submitTrim">
          确认执行 XTRIM
        </el-button>
      </template>
    </el-dialog>
  </div>
</template>
