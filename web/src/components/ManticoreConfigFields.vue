<script setup lang="ts">
import { computed, ref } from "vue";
import { AlertTriangle, CheckCircle2, XCircle } from "lucide-vue-next";
import type { ManticoreAuthMode, ManticoreServiceConfig, ManticoreTestReport } from "@/api/setup";
import ObsNumberInput from "@/components/ui/ObsNumberInput.vue";
import { fmtBytes, fmtCount, fmtExact } from "@/utils/format";

const props = withDefaults(
  defineProps<{
    modelValue: ManticoreServiceConfig;
    testReport?: ManticoreTestReport | null;
    compact?: boolean;
  }>(),
  {
    testReport: null,
    compact: false,
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: ManticoreServiceConfig): void;
}>();

const showSecret = ref(false);

const MANTICORE_AUTH_MODES: Array<{
  id: ManticoreAuthMode;
  label: string;
  badge: string;
  badgeCls: string;
}> = [
  {
    id: "none",
    label: "无鉴权（内网直连）",
    badge: "无鉴权 · 内网直连",
    badgeCls: "bg-slate-800/90 text-slate-300 border-slate-700",
  },
  {
    id: "basic",
    label: "Basic Auth",
    badge: "Basic Auth · Nginx",
    badgeCls: "bg-emerald-500/15 text-emerald-300 border-emerald-500/30",
  },
  {
    id: "bearer",
    label: "Bearer Token",
    badge: "Bearer Token",
    badgeCls: "bg-cyan-500/15 text-cyan-300 border-cyan-500/30",
  },
  {
    id: "header",
    label: "自定义请求头",
    badge: "自定义请求头 API Key",
    badgeCls: "bg-amber-500/15 text-amber-300 border-amber-500/30",
  },
];

const model = computed(() => props.modelValue);

const effectiveAuthMode = computed<ManticoreAuthMode>(() => {
  const m = model.value.auth_mode || "none";
  if (m === "none" && (model.value.username.trim() !== "" || model.value.password !== "")) {
    return "basic";
  }
  return m;
});

const activeMeta = computed(
  () => MANTICORE_AUTH_MODES.find((m) => m.id === effectiveAuthMode.value) ?? MANTICORE_AUTH_MODES[0],
);

const secretInputType = computed(() => (showSecret.value ? "text" : "password"));

function updateField<K extends keyof ManticoreServiceConfig>(
  key: K,
  value: ManticoreServiceConfig[K],
) {
  emit("update:modelValue", { ...props.modelValue, [key]: value });
}

function setAuthMode(mode: ManticoreAuthMode) {
  const next: ManticoreServiceConfig = { ...props.modelValue, auth_mode: mode };
  if (mode === "none") {
    next.username = "";
    next.password = "";
  }
  emit("update:modelValue", next);
}
</script>

<template>
  <div class="space-y-4 text-xs">
    <div class="grid grid-cols-1 sm:grid-cols-3 gap-3.5">
      <div class="sm:col-span-2">
        <label class="text-[11px] font-medium text-slate-400 block mb-1.5">HTTP API 地址</label>
        <input
          :value="model.url"
          type="text"
          placeholder="http://127.0.0.1:9308 或 https://manticore.example.com"
          class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
          @input="updateField('url', ($event.target as HTMLInputElement).value)"
        />
      </div>
      <div>
        <label class="text-[11px] font-medium text-slate-400 block mb-1.5">
          {{ compact ? "目标表 (锁定保护)" : "目标表 (结构冻结)" }}
        </label>
        <input
          :value="model.table_name"
          type="text"
          disabled
          class="obs-input w-full rounded-lg px-3 py-2 font-mono-num opacity-80"
        />
      </div>
    </div>

    <!-- Nginx 反向代理鉴权分区 -->
    <div class="obs-inset rounded-xl p-4 space-y-4">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div>
          <div class="font-medium text-slate-200">反向代理鉴权 (Nginx / HTTP API)</div>
          <div class="text-[11px] text-slate-500 mt-1">
            Manticore 原生 HTTP API 无鉴权，对外暴露建议经 Nginx 反向代理授权
          </div>
        </div>
        <div class="flex items-center gap-2">
          <button
            v-if="effectiveAuthMode !== 'none'"
            type="button"
            class="obs-btn-ghost px-2.5 py-1 rounded-md text-[11px] cursor-pointer"
            @click="showSecret = !showSecret"
          >
            {{ showSecret ? "隐藏凭据" : "显示凭据" }}
          </button>
          <span
            class="px-2 py-0.5 rounded border text-[11px] font-mono-num"
            :class="activeMeta.badgeCls"
          >
            {{ activeMeta.badge }}
          </span>
        </div>
      </div>

      <!-- 4 态暗色分段轨道 -->
      <div class="p-1 rounded-xl bg-[#070a11] border border-slate-800/80 grid grid-cols-2 sm:grid-cols-4 gap-1">
        <button
          v-for="m in MANTICORE_AUTH_MODES"
          :key="m.id"
          type="button"
          class="px-2.5 py-1.5 rounded-lg border text-[11px] text-center transition cursor-pointer"
          :class="
            effectiveAuthMode === m.id
              ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/35 font-semibold'
              : 'border-transparent text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'
          "
          @click="setAuthMode(m.id)"
        >
          {{ m.label }}
        </button>
      </div>

      <!-- 模式对应输入与协议提示 -->
      <div class="space-y-3.5 pt-0.5">
        <template v-if="effectiveAuthMode === 'basic'">
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3.5">
            <div>
              <label class="text-[11px] font-medium text-slate-300 block mb-1.5">用户名 (auth_basic)</label>
              <input
                :value="model.username"
                type="text"
                placeholder="nginx auth_basic 用户名"
                class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
                @input="updateField('username', ($event.target as HTMLInputElement).value)"
              />
            </div>
            <div>
              <label class="text-[11px] font-medium text-slate-300 block mb-1.5">密码</label>
              <input
                :value="model.password"
                :type="secretInputType"
                placeholder="反向代理 Basic Auth 密码"
                class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
                @input="updateField('password', ($event.target as HTMLInputElement).value)"
              />
            </div>
          </div>
          <div class="mt-3.5 rounded-lg bg-[#070a11]/85 border border-slate-800/75 px-3 py-2.5 flex flex-wrap items-center gap-x-2.5 gap-y-1.5 text-[11px] leading-relaxed">
            <code class="px-1.5 py-0.5 rounded bg-emerald-500/10 border border-emerald-500/20 text-emerald-300 font-mono-num">
              Authorization: Basic &lt;base64(user:pass)&gt;
            </code>
            <span class="text-slate-400">适配 Nginx auth_basic / .htpasswd 基础认证</span>
          </div>
        </template>

        <template v-else-if="effectiveAuthMode === 'bearer'">
          <div>
            <label class="text-[11px] font-medium text-slate-300 block mb-1.5">Bearer Token</label>
            <input
              :value="model.bearer_token"
              :type="secretInputType"
              placeholder="输入 Token，自动兼容 Bearer 前缀"
              class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
              @input="updateField('bearer_token', ($event.target as HTMLInputElement).value)"
            />
          </div>
          <div class="mt-3.5 rounded-lg bg-[#070a11]/85 border border-slate-800/75 px-3 py-2.5 flex flex-wrap items-center gap-x-2.5 gap-y-1.5 text-[11px] leading-relaxed">
            <code class="px-1.5 py-0.5 rounded bg-cyan-500/10 border border-cyan-500/20 text-cyan-300 font-mono-num">
              Authorization: Bearer &lt;token&gt;
            </code>
            <span class="text-slate-400">适配 Nginx $http_authorization 或网关静态令牌鉴权</span>
          </div>
        </template>

        <template v-else-if="effectiveAuthMode === 'header'">
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3.5">
            <div>
              <label class="text-[11px] font-medium text-slate-300 block mb-1.5">请求头名称</label>
              <input
                :value="model.header_name"
                type="text"
                placeholder="X-API-Key"
                class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
                @input="updateField('header_name', ($event.target as HTMLInputElement).value)"
              />
            </div>
            <div>
              <label class="text-[11px] font-medium text-slate-300 block mb-1.5">请求头值 (API Key)</label>
              <input
                :value="model.header_value"
                :type="secretInputType"
                placeholder="输入自定义密钥值"
                class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
                @input="updateField('header_value', ($event.target as HTMLInputElement).value)"
              />
            </div>
          </div>
          <div class="mt-3.5 rounded-lg bg-[#070a11]/85 border border-slate-800/75 px-3 py-2.5 flex flex-wrap items-center gap-x-2.5 gap-y-1.5 text-[11px] leading-relaxed">
            <code class="px-1.5 py-0.5 rounded bg-amber-500/10 border border-amber-500/20 text-amber-300 font-mono-num">
              {{ model.header_name.trim() || "X-API-Key" }}: &lt;value&gt;
            </code>
            <span class="text-slate-400">适配 Nginx $http_x_api_key 自定义密钥鉴权</span>
          </div>
        </template>

        <template v-else>
          <div class="rounded-lg bg-[#070a11]/70 border border-slate-800/75 px-3 py-2.5 text-[11px] text-slate-400 leading-relaxed">
            不附加任何鉴权请求头，仅建议在受信任的内网 / 本机回环直接访问场景下使用（如
            <span class="font-mono-num text-slate-300">http://127.0.0.1:9308</span>）。
          </div>
        </template>
      </div>
    </div>

    <div class="grid grid-cols-1 sm:grid-cols-2 gap-3.5">
      <div>
        <label class="text-[11px] font-medium text-slate-400 block mb-1.5">请求超时 (秒)</label>
        <ObsNumberInput
          :model-value="model.request_timeout_secs"
          :min="1"
          :max="120"
          @update:model-value="(v) => updateField('request_timeout_secs', v ?? 10)"
        />
      </div>
      <div>
        <label class="text-[11px] font-medium text-slate-400 block mb-1.5">连接池上限</label>
        <ObsNumberInput
          :model-value="model.max_connections"
          :min="1"
          :max="10000"
          @update:model-value="(v) => updateField('max_connections', v ?? 1280)"
        />
      </div>
    </div>

    <!-- 连接与 Schema 校验结果 -->
    <div v-if="testReport" class="space-y-3">
      <div
        class="rounded-xl border px-3.5 py-2.5 text-xs space-y-1.5"
        :class="
          testReport.ok
            ? 'border-emerald-500/30 bg-emerald-950/30 text-emerald-200'
            : testReport.schema && !testReport.schema.table_exists
              ? 'border-amber-500/35 bg-amber-950/35 text-amber-200'
              : 'border-rose-500/35 bg-rose-950/35 text-rose-200'
        "
      >
        <div class="flex items-center gap-2 font-medium">
          <CheckCircle2 v-if="testReport.ok" class="h-4 w-4 text-emerald-400 shrink-0" />
          <AlertTriangle
            v-else-if="testReport.schema && !testReport.schema.table_exists"
            class="h-4 w-4 text-amber-400 shrink-0"
          />
          <XCircle v-else class="h-4 w-4 text-rose-400 shrink-0" />
          <span>{{ testReport.message }}</span>
        </div>
        <div class="flex flex-wrap gap-4 font-mono-num text-[11px] opacity-90 pl-6">
          <span>{{ testReport.latency_ms }}ms</span>
          <span
            v-if="
              testReport.status?.indexed_documents !== null &&
              testReport.status?.indexed_documents !== undefined
            "
            :title="fmtExact(testReport.status.indexed_documents, 'docs')"
          >
            {{ fmtCount(testReport.status.indexed_documents) }} docs
          </span>
          <span
            v-if="
              testReport.status?.disk_bytes !== null &&
              testReport.status?.disk_bytes !== undefined
            "
          >
            {{ fmtBytes(testReport.status.disk_bytes) }}
          </span>
        </div>
      </div>

      <div
        v-if="testReport.schema && testReport.schema.columns.length > 0"
        class="obs-inset rounded-xl overflow-hidden font-mono-num"
      >
        <div class="px-3.5 py-2 border-b border-slate-800 text-slate-300 font-sans font-medium">
          DESCRIBE torrents 存量表结构校验
        </div>
        <div class="grid grid-cols-1 sm:grid-cols-3 px-3.5 py-2.5 text-[11px] text-slate-400 gap-2">
          <div v-for="col in testReport.schema.columns" :key="col.field" class="truncate">
            <span class="text-slate-200">{{ col.field }}</span>
            <span class="text-slate-600"> · </span>
            <span class="text-cyan-300/90">{{ col.column_type }}</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
