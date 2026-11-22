<script setup lang="ts">
import { ref } from "vue";
import {
  AlertTriangle,
  Download,
  History,
  Play,
  ShieldCheck,
  TerminalSquare,
} from "lucide-vue-next";
import { ElMessage } from "element-plus";
import {
  executeReadonlySql,
  exportRowsToCsv,
  exportRowsToJson,
  type SqlQueryResponse,
} from "@/api/sql";

interface QueryHistoryEntry {
  sql: string;
  row_count: number;
  took_ms: number;
  at: string;
}

const sqlText = ref("SELECT id, info_hash, refine_name, file_count, hot, updated_at FROM torrents ORDER BY updated_at DESC LIMIT 20");
const maxRows = ref(200);
const running = ref(false);
const result = ref<SqlQueryResponse | null>(null);
const errorBanner = ref("");
const history = ref<QueryHistoryEntry[]>([]);

const templates = [
  { label: "最新入库 20 条", sql: "SELECT id, info_hash, refine_name, file_count, hot, updated_at FROM torrents ORDER BY updated_at DESC LIMIT 20" },
  { label: "热度 Top 20", sql: "SELECT id, info_hash, refine_name, hot, file_count FROM torrents ORDER BY hot DESC LIMIT 20" },
  { label: "文件数 Top 20", sql: "SELECT id, info_hash, raw_name, file_count, hot FROM torrents ORDER BY file_count DESC LIMIT 20" },
  { label: "DESCRIBE torrents", sql: "DESCRIBE torrents" },
  { label: "SHOW TABLE torrents STATUS", sql: "SHOW TABLE torrents STATUS" },
  { label: "SHOW TABLES", sql: "SHOW TABLES" },
];

function applyTemplate(tplSql: string) {
  sqlText.value = tplSql;
  void runQuery();
}

function formatCellValue(val: unknown): string {
  if (val === null || val === undefined) return "";
  if (typeof val === "object") return JSON.stringify(val);
  return String(val);
}

async function runQuery() {
  if (!sqlText.value.trim()) {
    ElMessage.warning("请输入只读 SQL 查询语句");
    return;
  }
  running.value = true;
  errorBanner.value = "";
  try {
    const res = await executeReadonlySql(sqlText.value, maxRows.value);
    result.value = res;
    history.value.unshift({
      sql: res.executed_sql,
      row_count: res.row_count,
      took_ms: res.took_ms,
      at: new Date().toLocaleTimeString("zh-CN", { hour12: false }),
    });
    if (history.value.length > 20) history.value.pop();
  } catch (err) {
    errorBanner.value = err instanceof Error ? err.message : "SQL 执行失败";
  } finally {
    running.value = false;
  }
}

function handleExportCsv() {
  if (!result.value) return;
  exportRowsToCsv(result.value.columns, result.value.rows);
}

function handleExportJson() {
  if (!result.value) return;
  exportRowsToJson(result.value.rows);
}
</script>

<template>
  <div class="max-w-7xl mx-auto space-y-5">
    <!-- 顶部声明与快捷模板 -->
    <div class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-3.5">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <div class="flex items-center gap-2">
            <TerminalSquare class="h-4 w-4 text-indigo-600" />
            <h2 class="text-base font-semibold text-slate-900">Manticore 只读 SQL 诊断控制台</h2>
            <el-tag size="small" type="success" effect="light">
              <span class="inline-flex items-center gap-1">
                <ShieldCheck class="h-3 w-3" />
                仅允许单条只读语句
              </span>
            </el-tag>
          </div>
          <p class="text-xs text-slate-500 mt-1">
            服务端拦截一切 `INSERT`/`UPDATE`/`DELETE`/`DROP`/`ALTER` 与分号多语句；未带 `LIMIT` 的 `SELECT` 自动注入上限，并施加 10s 独立读超时。
          </p>
        </div>

        <div class="flex items-center gap-2 text-xs">
          <span class="text-slate-500">默认最大行数:</span>
          <el-input-number v-model="maxRows" size="small" :min="1" :max="1000" style="width: 110px" />
          <el-button size="small" type="primary" :loading="running" @click="runQuery">
            <Play class="h-3.5 w-3.5 mr-1" />
            执行查询 (Ctrl+Enter)
          </el-button>
        </div>
      </div>

      <!-- 快捷只读模板 -->
      <div class="flex flex-wrap items-center gap-1.5 pt-2 border-t border-slate-100">
        <span class="text-xs text-slate-500 mr-1">常用诊断模板:</span>
        <el-button
          v-for="tpl in templates"
          :key="tpl.label"
          size="small"
          plain
          @click="applyTemplate(tpl.sql)"
        >
          {{ tpl.label }}
        </el-button>
      </div>

      <!-- 等宽深色编辑器 -->
      <textarea
        v-model="sqlText"
        rows="5"
        spellcheck="false"
        placeholder="输入 SELECT / SHOW / DESCRIBE / DESC / EXPLAIN 只读语句..."
        class="w-full rounded-lg border border-slate-300 bg-slate-950 text-slate-100 font-mono text-xs p-3.5 focus:outline-none focus:ring-2 focus:ring-indigo-500"
        @keydown.ctrl.enter.prevent="runQuery"
        @keydown.meta.enter.prevent="runQuery"
      />

      <div
        v-if="errorBanner"
        class="rounded-lg border border-rose-200 bg-rose-50 px-3.5 py-2.5 text-xs text-rose-800 flex items-center gap-2"
      >
        <AlertTriangle class="h-4 w-4 text-rose-600 shrink-0" />
        <span>{{ errorBanner }}</span>
      </div>
    </div>

    <!-- 查询结果表格 -->
    <div v-if="result" class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-3">
      <div class="flex flex-wrap items-center justify-between gap-2 text-xs">
        <div class="flex flex-wrap items-center gap-3 text-slate-600">
          <span>返回 <strong class="font-mono text-slate-900">{{ result.row_count }}</strong> 行</span>
          <span>耗时 <strong class="font-mono text-emerald-700">{{ result.took_ms }} ms</strong></span>
          <span class="font-mono text-[11px] bg-slate-100 px-2 py-0.5 rounded text-slate-700">
            实际执行: {{ result.executed_sql }}
          </span>
        </div>
        <div class="flex items-center gap-2">
          <el-button size="small" :disabled="result.rows.length === 0" @click="handleExportCsv">
            <Download class="h-3.5 w-3.5 mr-1" />
            导出 CSV
          </el-button>
          <el-button size="small" :disabled="result.rows.length === 0" @click="handleExportJson">
            <Download class="h-3.5 w-3.5 mr-1" />
            导出 JSON
          </el-button>
        </div>
      </div>

      <el-alert v-if="result.warning" type="warning" :title="result.warning" :closable="false" />

      <el-table :data="result.rows" size="small" border stripe max-height="480">
        <el-table-column
          v-for="col in result.columns"
          :key="col"
          :prop="col"
          :label="col"
          min-width="130"
          show-overflow-tooltip
        >
          <template #default="{ row }">
            <span class="font-mono text-xs">{{ formatCellValue(row[col]) }}</span>
          </template>
        </el-table-column>
      </el-table>
    </div>

    <!-- 本次会话执行历史 -->
    <div v-if="history.length > 0" class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-2.5">
      <div class="text-xs font-semibold text-slate-800 flex items-center gap-1.5">
        <History class="h-3.5 w-3.5 text-slate-500" />
        最近查询历史 (点击重填)
      </div>
      <div class="space-y-1.5">
        <div
          v-for="(item, idx) in history"
          :key="idx"
          class="flex items-center justify-between gap-3 rounded border border-slate-100 bg-slate-50/70 px-3 py-1.5 text-xs hover:border-indigo-200 cursor-pointer"
          @click="sqlText = item.sql"
        >
          <span class="font-mono text-slate-700 truncate">{{ item.sql }}</span>
          <span class="font-mono text-[11px] text-slate-400 shrink-0">
            {{ item.row_count }} 行 · {{ item.took_ms }}ms · {{ item.at }}
          </span>
        </div>
      </div>
    </div>
  </div>
</template>
