<script setup lang="ts">
import { computed, onUnmounted, ref } from "vue";
import { ElMessage } from "element-plus";
import {
  AlertTriangle,
  Download,
  Play,
  Terminal,
} from "lucide-vue-next";
import { ApiError } from "@/api/client";
import {
  executeReadonlySql,
  exportRowsToCsv,
  exportRowsToJson,
  type SqlQueryResponse,
} from "@/api/sql";
import ObsSelect from "@/components/ui/ObsSelect.vue";

const TEMPLATES: Array<{ label: string; sql: string }> = [
  {
    label: "最新入库 20 条",
    sql: "SELECT id, info_hash, refine_name, raw_name, file_count, total_size, hot, updated_at FROM torrents ORDER BY updated_at DESC LIMIT 20;",
  },
  {
    label: "按体积 Top 20",
    sql: "SELECT id, info_hash, refine_name, file_count, total_size, hot, updated_at FROM torrents ORDER BY total_size DESC LIMIT 20;",
  },
  {
    label: "热度 Top 20",
    sql: "SELECT id, info_hash, refine_name, file_count, total_size, hot, updated_at FROM torrents ORDER BY hot DESC LIMIT 20;",
  },
  {
    label: "添加 total_size 列",
    sql: "ALTER TABLE torrents ADD COLUMN total_size bigint;",
  },
  {
    label: "DESCRIBE torrents",
    sql: "DESCRIBE torrents;",
  },
  {
    label: "SHOW TABLES",
    sql: "SHOW TABLES;",
  },
  {
    label: "SHOW TABLE torrents STATUS",
    sql: "SHOW TABLE torrents STATUS;",
  },
];

const MAX_ROWS_OPTIONS = [
  { label: "最多 50 行", value: 50 },
  { label: "最多 200 行", value: 200 },
  { label: "最多 500 行", value: 500 },
  { label: "最多 1000 行", value: 1000 },
];

const TIMEOUT_OPTIONS = [
  { label: "自动 (DDL 10m / 查询 60s)", value: 0 },
  { label: "超时 60 秒", value: 60 },
  { label: "超时 5 分钟", value: 300 },
  { label: "超时 10 分钟", value: 600 },
  { label: "超时 30 分钟", value: 1800 },
];

const HEAVY_DDL_KEYWORDS = new Set([
  "ALTER",
  "OPTIMIZE",
  "FLUSH",
  "TRUNCATE",
  "RELOAD",
  "CREATE",
  "DROP",
  "ATTACH",
  "IMPORT",
  "FREEZE",
  "UNFREEZE",
]);

const sqlText = ref(TEMPLATES[0].sql);
const maxRows = ref(200);
const timeoutSecs = ref(0);
const running = ref(false);
const elapsedSecs = ref(0);
const result = ref<SqlQueryResponse | null>(null);
const queryError = ref("");
const queryTimeoutHint = ref(false);
let elapsedTimer: ReturnType<typeof setInterval> | null = null;

function stopElapsedTimer() {
  if (elapsedTimer) {
    clearInterval(elapsedTimer);
    elapsedTimer = null;
  }
}

const currentFirstKeyword = computed(() => {
  const first = sqlText.value.trim().split(/\s+/)[0] ?? "";
  return first.toUpperCase();
});

const isHeavyDdlSql = computed(() => HEAVY_DDL_KEYWORDS.has(currentFirstKeyword.value));

const columns = computed(() => {
  if (!result.value) return [];
  if (result.value.columns.length > 0) return result.value.columns;
  const first = result.value.rows[0];
  return first ? Object.keys(first) : [];
});

async function runQuery(overrideSql?: string) {
  const targetSql = typeof overrideSql === "string" ? overrideSql : sqlText.value;
  if (!targetSql.trim()) {
    ElMessage.warning("请输入 SQL 语句");
    return;
  }
  if (typeof overrideSql === "string") {
    sqlText.value = overrideSql;
  }
  const keyword = (targetSql.trim().split(/\s+/)[0] ?? "").toUpperCase();
  const ddl = HEAVY_DDL_KEYWORDS.has(keyword);

  running.value = true;
  elapsedSecs.value = 0;
  queryError.value = "";
  queryTimeoutHint.value = false;
  stopElapsedTimer();
  const startedAt = Date.now();
  elapsedTimer = setInterval(() => {
    elapsedSecs.value = Math.floor((Date.now() - startedAt) / 1000);
  }, 1000);

  try {
    result.value = await executeReadonlySql(
      targetSql,
      maxRows.value,
      timeoutSecs.value > 0 ? timeoutSecs.value : undefined,
    );
  } catch (e) {
    const isTimeoutStatus =
      e instanceof ApiError &&
      (e.code === "MANTICORE_SQL_TIMEOUT" ||
        e.status === 504 ||
        e.status === 524 ||
        e.status === 408);
    const msg = e instanceof Error ? e.message : "SQL 查询执行失败";
    const looksLikeTimeout =
      isTimeoutStatus ||
      /timeout|timed out|超时|504|524/i.test(msg);

    if (looksLikeTimeout && ddl) {
      queryTimeoutHint.value = true;
      queryError.value =
        e instanceof ApiError && e.code === "MANTICORE_SQL_TIMEOUT"
          ? e.message
          : `${msg}（可能由反向代理或网关超时触发）。注意：${keyword} 等大表 DDL 在 Manticore 服务端通常仍在后台继续执行并最终成功，请勿重复提交，可点击下方按钮检查表结构或后台线程状态。`;
    } else {
      queryError.value = msg;
    }
  } finally {
    stopElapsedTimer();
    running.value = false;
  }
}

onUnmounted(() => {
  stopElapsedTimer();
});

function formatCell(v: unknown): string {
  if (v === null || v === undefined) return "NULL";
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}
</script>

<template>
  <div class="space-y-4">
    <!-- SQL Editor Card -->
    <section class="obs-card rounded-xl p-4 space-y-3">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="flex items-center gap-2">
          <Terminal class="w-4 h-4 text-emerald-400" />
          <h2 class="text-sm font-semibold text-white">Manticore SQL 终端</h2>
          <span class="px-2 py-0.5 rounded-md bg-amber-500/10 border border-amber-500/30 text-[10px] font-mono text-amber-300">
            FULL SQL · ADMIN
          </span>
        </div>

        <div class="flex flex-wrap items-center gap-1.5">
          <button
            v-for="tpl in TEMPLATES"
            :key="tpl.label"
            type="button"
            class="obs-btn obs-btn-ghost px-2.5! py-1! text-[11px]!"
            @click="sqlText = tpl.sql"
          >
            {{ tpl.label }}
          </button>
        </div>
      </div>

      <textarea
        v-model="sqlText"
        rows="5"
        placeholder="输入 Manticore SQL（支持 SELECT / SHOW / DESCRIBE / ALTER / INSERT / UPDATE / DELETE 等 · Ctrl + Enter 执行）"
        class="w-full rounded-lg bg-[#07090e] border border-slate-800 focus:border-cyan-500/60 focus:outline-none p-3 text-xs font-mono text-cyan-200 leading-relaxed obs-scroll"
        @keydown.ctrl.enter.prevent="runQuery()"
        @keydown.meta.enter.prevent="runQuery()"
      />

      <div class="flex flex-wrap items-center justify-between gap-2 text-xs">
        <span class="text-[11px] text-slate-400 font-mono">
          <template v-if="isHeavyDdlSql">
            检测到 DDL/维护语句（默认放宽至 10 分钟超时）；若经反代访问出现 504，Manticore 仍会在后台继续执行完成
          </template>
          <template v-else>
            已放行全部单条 Manticore SQL（含 ALTER / DDL / DML，SELECT 自动限制行数）· Ctrl+Enter 执行
          </template>
        </span>

        <div class="flex flex-wrap items-center gap-2">
          <ObsSelect
            v-model="timeoutSecs"
            :options="TIMEOUT_OPTIONS"
            mono
            class="w-52"
            trigger-class="px-2.5! py-1.5! text-xs!"
          />
          <ObsSelect
            v-model="maxRows"
            :options="MAX_ROWS_OPTIONS"
            mono
            class="w-32"
            trigger-class="px-2.5! py-1.5! text-xs!"
          />
          <button
            type="button"
            class="obs-btn obs-btn-primary px-4! py-1.5! text-xs! inline-flex items-center gap-1.5"
            :disabled="running"
            @click="runQuery()"
          >
            <Play class="w-3.5 h-3.5" />
            {{ running ? `执行中… ${elapsedSecs}s` : "执行 SQL" }}
          </button>
        </div>
      </div>
    </section>

    <div
      v-if="queryError"
      class="obs-card rounded-xl p-4 text-xs font-mono space-y-2.5"
      :class="
        queryTimeoutHint
          ? 'border-amber-500/40! text-amber-200 bg-amber-500/[0.06]'
          : 'border-rose-500/35! text-rose-300'
      "
    >
      <div class="flex items-start gap-2">
        <AlertTriangle
          v-if="queryTimeoutHint"
          class="w-4 h-4 text-amber-400 shrink-0 mt-0.5"
        />
        <span class="leading-relaxed">{{ queryError }}</span>
      </div>
      <div v-if="queryTimeoutHint" class="flex flex-wrap items-center gap-2 pt-1">
        <button
          type="button"
          class="obs-btn obs-btn-ghost px-2.5! py-1! text-[11px]!"
          :disabled="running"
          @click="runQuery('DESCRIBE torrents;')"
        >
          立即检查 DESCRIBE torrents;
        </button>
        <button
          type="button"
          class="obs-btn obs-btn-ghost px-2.5! py-1! text-[11px]!"
          :disabled="running"
          @click="runQuery('SHOW THREADS;')"
        >
          查看 Manticore 后台线程 SHOW THREADS;
        </button>
      </div>
    </div>

    <!-- Results Card -->
    <section v-if="result" class="obs-card rounded-xl overflow-hidden">
      <div class="px-4 py-3 border-b border-slate-800 bg-[#0b0f17] flex flex-wrap items-center justify-between gap-2 text-xs">
        <div class="flex flex-wrap items-center gap-3 font-mono-num">
          <span class="text-emerald-300 font-semibold">返回 {{ result.row_count }} 行</span>
          <span v-if="result.total !== null" class="text-slate-400">Total {{ result.total }}</span>
          <span class="text-sky-300">耗时 {{ result.took_ms }} ms</span>
        </div>

        <div class="flex items-center gap-2">
          <button
            type="button"
            class="obs-btn obs-btn-ghost px-2.5! py-1! text-[11px]! inline-flex items-center gap-1"
            :disabled="result.rows.length === 0"
            @click="exportRowsToCsv(columns, result.rows)"
          >
            <Download class="w-3 h-3 text-emerald-400" />
            导出 CSV
          </button>
          <button
            type="button"
            class="obs-btn obs-btn-ghost px-2.5! py-1! text-[11px]! inline-flex items-center gap-1"
            :disabled="result.rows.length === 0"
            @click="exportRowsToJson(result.rows)"
          >
            <Download class="w-3 h-3 text-sky-400" />
            导出 JSON
          </button>
        </div>
      </div>

      <div
        v-if="result.warning"
        class="px-4 py-2 bg-amber-500/10 border-b border-amber-500/30 text-[11px] font-mono text-amber-200 flex items-center gap-2"
      >
        <AlertTriangle class="w-3.5 h-3.5 shrink-0" />
        {{ result.warning }}
      </div>

      <div class="overflow-x-auto max-h-[540px] obs-scroll">
        <table v-if="columns.length > 0" class="w-full text-left border-collapse text-xs font-mono">
          <thead class="sticky top-0 bg-[#0b0f17] border-b border-slate-800 text-[10px] uppercase tracking-wider text-slate-400">
            <tr>
              <th v-for="col in columns" :key="col" class="px-3.5 py-2.5 whitespace-nowrap">{{ col }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(row, rIdx) in result.rows" :key="rIdx" class="border-b border-slate-800/50 hover:bg-slate-800/35">
              <td
                v-for="col in columns"
                :key="col"
                class="px-3.5 py-2 text-[11px] max-w-md truncate"
                :class="
                  col === 'info_hash'
                    ? 'text-cyan-300'
                    : col === 'hot'
                      ? 'text-amber-300 font-mono-num'
                      : 'text-slate-200'
                "
                :title="formatCell(row[col])"
              >
                {{ formatCell(row[col]) }}
              </td>
            </tr>
          </tbody>
        </table>
        <div v-else class="py-12 text-center text-xs text-slate-500">执行成功（返回 0 行结果）</div>
      </div>
    </section>
  </div>
</template>
