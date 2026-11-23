<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { ElMessage } from "element-plus";
import {
  Activity,
  Cpu,
  Database,
  Eye,
  KeyRound,
  Layers,
  RefreshCw,
  Scissors,
  Search,
  Trash2,
} from "lucide-vue-next";
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
import ObsDrawer from "@/components/ui/ObsDrawer.vue";
import ObsModal from "@/components/ui/ObsModal.vue";
import ObsNumberInput from "@/components/ui/ObsNumberInput.vue";
import {
  fmtBytes,
  fmtCount,
  fmtDurMs,
  fmtDurSecs,
  fmtExact,
  fmtRate,
  formatDateTime,
} from "@/utils/format";

const route = useRoute();
const router = useRouter();

const loading = ref(false);
const overview = ref<RedisOverviewResponse | null>(null);

// Stream Preview Drawer
const previewVisible = ref(false);
const previewLoading = ref(false);
const previewData = ref<PreviewStreamResponse | null>(null);

// Safe XTRIM Modal
const trimVisible = ref(false);
const trimStreamName = ref("");
const trimMaxLen = ref(1_000_000);
const trimApprox = ref(true);
const trimConfirmInput = ref("");
const trimSubmitting = ref(false);

// Hash Inspector
const hashInput = ref("");
const inspecting = ref(false);
const inspectResult = ref<RedisHashInspectResponse | null>(null);
const clearOpts = ref({
  clear_pending: true,
  clear_rejected: true,
  clear_indexed: false,
});
const clearingLocks = ref(false);

const trimConfirmed = computed(
  () => trimConfirmInput.value.trim() === trimStreamName.value && trimStreamName.value.length > 0,
);

async function fetchOverview() {
  loading.value = true;
  try {
    overview.value = await getRedisOverview();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "加载 Redis 状态失败");
  } finally {
    loading.value = false;
  }
}

async function openPreview(stream: RedisStreamOverviewDto) {
  previewVisible.value = true;
  previewLoading.value = true;
  previewData.value = null;
  try {
    previewData.value = await previewRedisStream(stream.stream_name, 25);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "读取 Stream 消息预览失败");
  } finally {
    previewLoading.value = false;
  }
}

function openTrimModal(stream: RedisStreamOverviewDto) {
  trimStreamName.value = stream.stream_name;
  trimMaxLen.value = Math.min(Math.max(10_000, Math.floor(stream.length * 0.5)), 2_000_000);
  trimApprox.value = true;
  trimConfirmInput.value = "";
  trimVisible.value = true;
}

async function submitTrim() {
  if (!trimConfirmed.value) return;
  trimSubmitting.value = true;
  try {
    const res = await trimRedisStream(trimStreamName.value, {
      max_len: trimMaxLen.value,
      approximate: trimApprox.value,
      confirm_stream_name: trimConfirmInput.value.trim(),
    });
    ElMessage.success(`XTRIM 完成：裁剪 ${fmtExact(res.trimmed_count)} 条，当前剩余 ${fmtExact(res.remaining_len)} 条`);
    trimVisible.value = false;
    await fetchOverview();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "裁剪失败");
  } finally {
    trimSubmitting.value = false;
  }
}

async function runInspectHash(explicitHash?: string) {
  const target = (explicitHash ?? hashInput.value).trim();
  if (!/^[0-9a-fA-F]{40}$/.test(target)) {
    ElMessage.warning("请输入合法的 40 位十六进制 InfoHash");
    return;
  }
  hashInput.value = target.toLowerCase();
  inspecting.value = true;
  try {
    inspectResult.value = await inspectRedisHash(hashInput.value);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "穿透查询失败");
  } finally {
    inspecting.value = false;
  }
}

async function handleClearLocks() {
  if (!inspectResult.value) return;
  if (!clearOpts.value.clear_pending && !clearOpts.value.clear_rejected && !clearOpts.value.clear_indexed) {
    ElMessage.warning("请至少勾选一项要清理的 Redis 锁标记");
    return;
  }
  clearingLocks.value = true;
  try {
    const res = await clearRedisHashLocks(inspectResult.value.info_hash, clearOpts.value);
    ElMessage.success(`已清理 ${res.deleted_count} 个 Redis 键`);
    await runInspectHash(inspectResult.value.info_hash);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "清理失败");
  } finally {
    clearingLocks.value = false;
  }
}

function handleHashQueryParam() {
  const q = route.query.hash;
  if (typeof q === "string" && q.trim()) {
    hashInput.value = q.trim();
    void runInspectHash(q.trim());
    void router.replace({ path: "/redis", query: {} });
  }
}

function formatPttl(pttl: number, exists: boolean): string {
  if (!exists) return "—";
  if (pttl === -1) return "永不过期 (TTL -1)";
  if (pttl < 0) return String(pttl);
  return fmtDurMs(pttl);
}

watch(
  () => route.query.hash,
  () => {
    handleHashQueryParam();
  },
);

onMounted(async () => {
  await fetchOverview();
  handleHashQueryParam();
});
</script>

<template>
  <div class="space-y-4">
    <!-- Top 4 KPI Cards -->
    <div class="grid grid-cols-2 lg:grid-cols-4 gap-3">
      <div class="obs-card obs-card-hover rounded-xl p-4">
        <div class="flex items-center justify-between text-[11px] text-slate-400">
          <span>Redis 连通性 &amp; RTT 延迟</span>
          <Activity class="w-3.5 h-3.5" :class="overview?.ok ? 'text-emerald-400' : 'text-rose-400'" />
        </div>
        <div class="mt-2 flex items-baseline gap-2">
          <span
            class="text-2xl font-bold font-mono-num"
            :class="overview?.ok ? 'text-emerald-300' : 'text-rose-300'"
          >
            {{ overview?.ok ? "ONLINE" : overview ? "ERROR" : "—" }}
          </span>
          <span v-if="overview" class="text-xs font-mono-num text-slate-400">{{ overview.latency_ms }} ms</span>
        </div>
        <div class="mt-2 pt-2 border-t border-slate-800/70 text-[11px] font-mono text-slate-400 truncate">
          v{{ overview?.redis_version ?? "—" }} · Uptime {{ fmtDurSecs(overview?.uptime_secs) }}
        </div>
      </div>

      <div class="obs-card obs-card-hover rounded-xl p-4">
        <div class="flex items-center justify-between text-[11px] text-slate-400">
          <span>内存占用 / 峰值</span>
          <Cpu class="w-3.5 h-3.5 text-sky-400" />
        </div>
        <div class="mt-2 text-2xl font-bold font-mono-num text-white">
          {{ overview?.used_memory_human || fmtBytes(overview?.used_memory_bytes) }}
        </div>
        <div class="mt-2 pt-2 border-t border-slate-800/70 text-[11px] font-mono text-slate-400 truncate">
          峰值 {{ overview?.used_memory_peak_human ?? "—" }} · 客户端 {{ overview?.connected_clients ?? "—" }}
        </div>
      </div>

      <div class="obs-card obs-card-hover rounded-xl p-4">
        <div class="flex items-center justify-between text-[11px] text-slate-400">
          <span>键空间与即时 OPS</span>
          <Database class="w-3.5 h-3.5 text-cyan-400" />
        </div>
        <div class="mt-2 flex items-baseline gap-2 font-mono-num">
          <span class="text-2xl font-bold text-white">{{ fmtCount(overview?.dbsize) }}</span>
          <span class="text-[11px] text-slate-500">keys</span>
        </div>
        <div class="mt-2 pt-2 border-t border-slate-800/70 text-[11px] font-mono-num text-cyan-300">
          {{ fmtRate(overview?.instantaneous_ops_per_sec) }} ops/s
        </div>
      </div>

      <div class="obs-card obs-card-hover rounded-xl p-4">
        <div class="flex items-center justify-between text-[11px] text-slate-400">
          <span>面板 Redis 连接池</span>
          <Layers class="w-3.5 h-3.5 text-indigo-400" />
        </div>
        <div class="mt-2 text-2xl font-bold font-mono-num text-white">
          {{ overview ? `${overview.pool.available} / ${overview.pool.size}` : "—" }}
        </div>
        <div class="mt-2 pt-2 border-t border-slate-800/70 text-[11px] font-mono text-slate-400">
          上限 {{ overview?.pool.max_size ?? "—" }} · 等待队列 {{ overview?.pool.waiting ?? 0 }}
        </div>
      </div>
    </div>

    <!-- Config Summary Strip + Refresh -->
    <section v-if="overview" class="obs-card rounded-xl px-4 py-2.5 flex flex-wrap items-center justify-between gap-2 text-[11px] font-mono">
      <div class="flex flex-wrap items-center gap-4 text-slate-400">
        <span>Prefix: <b class="text-slate-100">{{ overview.config_summary.indexed_key_prefix }}</b></span>
        <span>Group: <b class="text-emerald-300">{{ overview.config_summary.consumer_group }}</b></span>
        <span>MAXLEN: <b class="text-slate-100">{{ fmtCount(overview.config_summary.max_stream_len) }}</b></span>
        <span>MaxMsg: <b class="text-slate-100">{{ fmtBytes(overview.config_summary.max_message_size) }}</b></span>
      </div>
      <button type="button" class="obs-btn obs-btn-ghost px-3! py-1! text-xs! inline-flex items-center gap-1" @click="fetchOverview">
        <RefreshCw class="w-3.5 h-3.5" :class="loading ? 'animate-spin text-emerald-400' : ''" />
        刷新队列遥测
      </button>
    </section>

    <!-- Stream Cards (New & Hot) -->
    <div v-if="overview" class="grid grid-cols-1 xl:grid-cols-2 gap-4">
      <section v-for="st in overview.streams" :key="st.stream_name" class="obs-card rounded-xl overflow-hidden flex flex-col">
        <div class="px-4 py-3 border-b border-slate-800 bg-[#0b0f17] flex flex-wrap items-center justify-between gap-2">
          <div>
            <div class="flex items-center gap-2">
              <span
                class="px-2 py-0.5 rounded-md text-[10px] font-mono border"
                :class="
                  st.role === 'new'
                    ? 'bg-emerald-500/12 text-emerald-300 border-emerald-500/30'
                    : 'bg-amber-500/12 text-amber-300 border-amber-500/30'
                "
              >
                {{ st.role === "new" ? "NEW TORRENTS" : "HOT DELTA" }}
              </span>
              <span class="font-mono text-xs font-semibold text-white">{{ st.stream_name }}</span>
              <span class="px-1.5 py-0.5 rounded bg-emerald-500/10 text-emerald-300 border border-emerald-500/25 text-[10px] font-mono inline-flex items-center gap-1">
                <span class="w-1.5 h-1.5 rounded-full bg-emerald-400" />
                活跃
              </span>
            </div>
            <div class="mt-1 text-[10px] font-mono text-slate-500">
              {{ st.first_entry_id ?? "—" }} .. {{ st.last_entry_id ?? "—" }}
            </div>
          </div>

          <div class="flex items-center gap-1.5">
            <button type="button" class="obs-btn obs-btn-ghost px-2.5! py-1! text-xs! inline-flex items-center gap-1" @click="openPreview(st)">
              <Eye class="w-3.5 h-3.5 text-sky-400" />
              消息预览
            </button>
            <button type="button" class="obs-btn obs-btn-danger px-2.5! py-1! text-xs! inline-flex items-center gap-1" @click="openTrimModal(st)">
              <Scissors class="w-3.5 h-3.5" />
              安全裁剪
            </button>
          </div>
        </div>

        <div class="p-4 space-y-3">
          <!-- Stream stats -->
          <div class="obs-inset rounded-xl p-3 grid grid-cols-3 gap-2 text-xs font-mono-num">
            <div>
              <div class="text-[10px] text-slate-500">当前长度 (XLEN)</div>
              <div class="text-base font-bold text-white mt-0.5">{{ fmtExact(st.length) }}</div>
            </div>
            <div>
              <div class="text-[10px] text-slate-500">累计入队</div>
              <div class="text-base font-bold text-sky-300 mt-0.5">{{ st.entries_added !== null ? fmtCount(st.entries_added) : "—" }}</div>
            </div>
            <div>
              <div class="text-[10px] text-slate-500">消费积压 (Lag / PEL)</div>
              <div class="text-base font-bold text-amber-300 mt-0.5">
                <template v-if="st.groups[0]">
                  {{ fmtCount(st.groups[0].lag ?? 0) }} / {{ fmtCount(st.groups[0].pending) }}
                </template>
                <template v-else>—</template>
              </div>
            </div>
          </div>

          <!-- Consumers table -->
          <div class="space-y-1.5">
            <div class="text-[11px] font-medium text-slate-400">
              活跃消费者 ({{ st.target_group_consumers.length }})
            </div>
            <div class="obs-inset rounded-xl overflow-hidden max-h-44 overflow-auto obs-scroll">
              <table class="w-full text-left border-collapse text-[11px] font-mono">
                <thead class="bg-[#0b0f17] border-b border-slate-800 text-[10px] uppercase tracking-wider text-slate-500">
                  <tr>
                    <th class="px-3 py-2">Consumer ID</th>
                    <th class="px-3 py-2 text-right">Pending</th>
                    <th class="px-3 py-2 text-right">空闲时间 (Idle)</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="c in st.target_group_consumers" :key="c.name" class="border-b border-slate-800/50 hover:bg-slate-800/30">
                    <td class="px-3 py-2 text-cyan-300">{{ c.name }}</td>
                    <td class="px-3 py-2 text-right font-mono-num text-amber-300">{{ c.pending }}</td>
                    <td class="px-3 py-2 text-right font-mono-num text-slate-400">{{ fmtDurMs(c.idle_ms) }}</td>
                  </tr>
                  <tr v-if="st.target_group_consumers.length === 0">
                    <td colspan="3" class="py-4 text-center text-slate-500">当前无已注册消费者</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </section>
    </div>

    <!-- InfoHash Penetrating Diagnostics -->
    <section class="obs-card rounded-xl p-5 space-y-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <div class="flex items-center gap-2">
            <KeyRound class="w-4 h-4 text-emerald-400" />
            <h2 class="text-sm font-semibold text-white">InfoHash 穿透诊断</h2>
          </div>
          <p class="text-[11px] text-slate-400 mt-0.5">
            穿透查询 Redis indexed / pending / rejected 与 Manticore 63-bit ID 实库状态
          </p>
        </div>

        <div class="flex items-center gap-2 flex-1 max-w-lg">
          <div class="relative flex-1">
            <Search class="w-3.5 h-3.5 text-slate-500 absolute left-3 top-1/2 -translate-y-1/2 pointer-events-none" />
            <input
              v-model="hashInput"
              type="text"
              maxlength="40"
              placeholder="输入 40 位十六进制 InfoHash…"
              class="obs-input w-full rounded-lg pl-9 pr-3 py-1.5 text-xs font-mono text-cyan-200"
              @keyup.enter="runInspectHash()"
            />
          </div>
          <button type="button" class="obs-btn obs-btn-primary px-4! py-1.5! text-xs! shrink-0" :disabled="inspecting" @click="runInspectHash()">
            {{ inspecting ? "穿透查询中…" : "查询诊断" }}
          </button>
        </div>
      </div>

      <template v-if="inspectResult">
        <div class="flex flex-wrap items-center gap-3 text-xs font-mono text-slate-300">
          <span>Hash: <b class="text-cyan-300">{{ inspectResult.info_hash }}</b></span>
          <span>Manticore 预期 ID: <b class="text-emerald-300">{{ inspectResult.expected_id }}</b></span>
        </div>

        <!-- 4-column diagnostics cards -->
        <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-3 text-xs">
          <!-- 1. Indexed Key -->
          <div
            class="obs-inset rounded-xl p-3.5 space-y-1.5 border transition-colors"
            :class="inspectResult.indexed.exists ? 'border-emerald-500/35 bg-emerald-500/[0.04]' : 'border-slate-800'"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="font-medium text-white">去重标记 (indexed)</span>
              <span
                class="px-2 py-0.5 rounded-md text-[10px] font-mono border"
                :class="inspectResult.indexed.exists ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30' : 'bg-slate-800 text-slate-500 border-slate-700'"
              >
                {{ inspectResult.indexed.exists ? "已命中" : "未标记" }}
              </span>
            </div>
            <div class="font-mono text-[10px] text-slate-500 break-all">{{ inspectResult.indexed.key }}</div>
            <div class="font-mono text-[11px] text-slate-400">
              值: {{ inspectResult.indexed.value ?? "—" }} · TTL: {{ formatPttl(inspectResult.indexed.pttl_ms, inspectResult.indexed.exists) }}
            </div>
          </div>

          <!-- 2. Pending Lock -->
          <div
            class="obs-inset rounded-xl p-3.5 space-y-1.5 border transition-colors"
            :class="inspectResult.pending.exists ? 'border-amber-500/35 bg-amber-500/[0.04]' : 'border-slate-800'"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="font-medium text-white">抓取锁 (pending)</span>
              <span
                class="px-2 py-0.5 rounded-md text-[10px] font-mono border"
                :class="inspectResult.pending.exists ? 'bg-amber-500/15 text-amber-300 border-amber-500/30' : 'bg-slate-800 text-slate-500 border-slate-700'"
              >
                {{ inspectResult.pending.exists ? "锁定中" : "无锁" }}
              </span>
            </div>
            <div class="font-mono text-[10px] text-slate-500 break-all">{{ inspectResult.pending.key }}</div>
            <div class="font-mono text-[11px] text-slate-400">
              值: {{ inspectResult.pending.value ?? "—" }} · 剩余: {{ formatPttl(inspectResult.pending.pttl_ms, inspectResult.pending.exists) }}
            </div>
          </div>

          <!-- 3. Rejected Key -->
          <div
            class="obs-inset rounded-xl p-3.5 space-y-1.5 border transition-colors"
            :class="inspectResult.rejected.exists ? 'border-rose-500/35 bg-rose-500/[0.04]' : 'border-slate-800'"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="font-medium text-white">拒绝缓存 (rejected)</span>
              <span
                class="px-2 py-0.5 rounded-md text-[10px] font-mono border"
                :class="inspectResult.rejected.exists ? 'bg-rose-500/15 text-rose-300 border-rose-500/30' : 'bg-slate-800 text-slate-500 border-slate-700'"
              >
                {{ inspectResult.rejected.exists ? "冷却拦截" : "未拦截" }}
              </span>
            </div>
            <div class="font-mono text-[10px] text-slate-500 break-all">{{ inspectResult.rejected.key }}</div>
            <div class="font-mono text-[11px] text-slate-400">
              值: {{ inspectResult.rejected.value ?? "—" }} · 剩余: {{ formatPttl(inspectResult.rejected.pttl_ms, inspectResult.rejected.exists) }}
            </div>
          </div>

          <!-- 4. Manticore實库核验 -->
          <div
            class="obs-inset rounded-xl p-3.5 space-y-1.5 border transition-colors"
            :class="inspectResult.manticore_exists ? 'border-cyan-500/35 bg-cyan-500/[0.04]' : 'border-slate-800'"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="font-medium text-white">Manticore 实库</span>
              <span
                class="px-2 py-0.5 rounded-md text-[10px] font-mono border"
                :class="inspectResult.manticore_exists ? 'bg-cyan-500/15 text-cyan-300 border-cyan-500/30' : 'bg-slate-800 text-slate-400 border-slate-700'"
              >
                {{ !inspectResult.manticore_checked ? "未检验" : inspectResult.manticore_exists ? "已在库中" : "库中无此记录" }}
              </span>
            </div>
            <div class="text-[11px] text-slate-200 truncate" :title="inspectResult.manticore_refine_name || inspectResult.manticore_raw_name || undefined">
              {{ inspectResult.manticore_exists ? (inspectResult.manticore_refine_name || inspectResult.manticore_raw_name) : "— 无匹配索引记录 —" }}
            </div>
            <div class="font-mono text-[11px] text-slate-400 flex flex-wrap items-center gap-2">
              <span v-if="inspectResult.manticore_hot !== null" class="text-amber-300">Hot {{ inspectResult.manticore_hot }}</span>
              <span v-if="inspectResult.manticore_updated_at">{{ formatDateTime(inspectResult.manticore_updated_at) }}</span>
              <span v-if="inspectResult.manticore_hot === null && !inspectResult.manticore_updated_at">ID {{ inspectResult.expected_id }}</span>
            </div>
          </div>
        </div>

        <!-- Inline Lock Clear Bar -->
        <div class="obs-inset rounded-xl p-3.5 flex flex-wrap items-center justify-between gap-3 text-xs">
          <span class="text-[11px] font-mono text-slate-400">
            异常状态人工解封：选择需要清除的 Redis 键类型后可执行即时清理
          </span>

          <div class="flex flex-wrap items-center gap-3">
            <label class="inline-flex items-center gap-1.5 text-xs text-slate-300 cursor-pointer">
              <input v-model="clearOpts.clear_pending" type="checkbox" class="obs-checkbox" />
              pending
            </label>
            <label class="inline-flex items-center gap-1.5 text-xs text-slate-300 cursor-pointer">
              <input v-model="clearOpts.clear_rejected" type="checkbox" class="obs-checkbox" />
              rejected
            </label>
            <label class="inline-flex items-center gap-1.5 text-xs text-amber-300 cursor-pointer">
              <input v-model="clearOpts.clear_indexed" type="checkbox" class="obs-checkbox" />
              indexed (慎用)
            </label>
            <button type="button" class="obs-btn obs-btn-danger px-3! py-1.5! text-xs! inline-flex items-center gap-1" :disabled="clearingLocks" @click="handleClearLocks">
              <Trash2 class="w-3.5 h-3.5" />
              {{ clearingLocks ? "清理中…" : "清理选定键" }}
            </button>
          </div>
        </div>
      </template>
    </section>

    <!-- Stream Preview Drawer -->
    <ObsDrawer v-model="previewVisible" :title="`Stream 消息抽样预览 · ${previewData?.stream_name ?? ''}`" max-width="820px">
      <div v-if="previewLoading" class="py-12 text-center text-xs font-mono text-slate-400">
        正在执行 XREVRANGE 读取最新队列消息…
      </div>
      <div v-else-if="previewData" class="space-y-2.5 text-xs">
        <div class="text-[11px] font-mono text-slate-400">
          队列当前长度 {{ fmtExact(previewData.total_len) }} · 展示最近 {{ previewData.items.length }} 条
        </div>
        <div v-for="item in previewData.items" :key="item.entry_id" class="obs-inset p-3 rounded-lg space-y-1">
          <div class="flex items-center justify-between font-mono text-[11px]">
            <span class="text-emerald-300">{{ item.entry_id }}</span>
            <span class="text-slate-400">{{ fmtBytes(item.payload_bytes) }} · {{ item.file_count ?? "?" }}f · {{ item.total_size ? fmtBytes(item.total_size) : "—" }}</span>
          </div>
          <div class="text-slate-100 break-all">{{ item.name || "（未携带 name 字段）" }}</div>
          <div class="font-mono text-[10px] text-slate-500">{{ item.info_hash ?? "—" }}</div>
        </div>
      </div>
    </ObsDrawer>

    <!-- Safe XTRIM Modal -->
    <ObsModal v-model="trimVisible" :title="`安全裁剪 Redis Stream · ${trimStreamName}`" max-width="460px">
      <div class="space-y-3 text-xs text-slate-300">
        <div class="obs-inset p-3 rounded-lg border-amber-500/30 text-[11px] text-amber-200">
          XTRIM 将永久保留最近 MAXLEN 条消息并裁掉历史旧消息。为防误操作，请在下方输入完整 Stream 名称确认。
        </div>
        <div class="grid grid-cols-2 gap-3 items-end">
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">保留上限 MAXLEN</label>
            <ObsNumberInput v-model="trimMaxLen" :min="0" :step="10000" />
          </div>
          <label class="inline-flex items-center gap-2 text-xs text-slate-200 cursor-pointer pb-1.5">
            <input v-model="trimApprox" type="checkbox" class="obs-checkbox" />
            使用 ~ 近似裁剪（高效）
          </label>
        </div>
        <div>
          <label class="block text-[11px] text-slate-400 mb-1">
            请输入 <code class="font-mono text-emerald-300">{{ trimStreamName }}</code> 以解锁裁剪按钮
          </label>
          <input v-model="trimConfirmInput" type="text" class="obs-input px-3 py-1.5 text-xs font-mono" />
        </div>
      </div>
      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 text-xs" @click="trimVisible = false">取消</button>
        <button type="button" class="obs-btn obs-btn-danger px-4 py-1.5 text-xs" :disabled="!trimConfirmed || trimSubmitting" @click="submitTrim">
          {{ trimSubmitting ? "裁剪中…" : "执行安全裁剪" }}
        </button>
      </template>
    </ObsModal>
  </div>
</template>
