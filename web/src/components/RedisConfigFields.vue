<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { CheckCircle2, ChevronDown, Lock, Unlock, XCircle } from "lucide-vue-next";
import type { RedisServiceConfig, RedisTestReport } from "@/api/setup";
import ObsNumberInput from "@/components/ui/ObsNumberInput.vue";
import { fmtCount } from "@/utils/format";

const props = withDefaults(
  defineProps<{
    modelValue: RedisServiceConfig;
    testReport?: RedisTestReport | null;
    lockIndexedPrefix?: boolean;
    compact?: boolean;
  }>(),
  {
    testReport: null,
    lockIndexedPrefix: true,
    compact: false,
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: RedisServiceConfig): void;
}>();

const inputMode = ref<"fields" | "url">(props.modelValue.url.trim() ? "url" : "fields");
const prefixUnlocked = ref(false);
const advancedOpen = ref(!props.compact);

watch(
  () => props.modelValue.url,
  (next) => {
    if (next.trim() && inputMode.value !== "url") {
      inputMode.value = "url";
    } else if (!next.trim() && inputMode.value === "url") {
      inputMode.value = "fields";
    }
  },
);

const model = computed(() => props.modelValue);

function switchMode(next: "fields" | "url") {
  inputMode.value = next;
  if (next === "fields") {
    emit("update:modelValue", { ...props.modelValue, url: "" });
  } else if (!props.modelValue.url.trim()) {
    const scheme = props.modelValue.tls ? "rediss" : "redis";
    const auth =
      props.modelValue.username || props.modelValue.password
        ? `${props.modelValue.username}:${props.modelValue.password}@`
        : "";
    const dbPart = props.modelValue.db > 0 ? `/${props.modelValue.db}` : "";
    emit("update:modelValue", {
      ...props.modelValue,
      url: `${scheme}://${auth}${props.modelValue.host}:${props.modelValue.port}${dbPart}`,
    });
  }
}

function updateField<K extends keyof RedisServiceConfig>(key: K, value: RedisServiceConfig[K]) {
  emit("update:modelValue", { ...props.modelValue, [key]: value });
}
</script>

<template>
  <div class="space-y-4 text-xs">
    <div class="flex items-center justify-between gap-3">
      <span class="font-medium text-slate-400">连接方式</span>
      <div class="inline-flex p-0.5 rounded-lg bg-[#070a11] border border-slate-800 text-[11px]">
        <button
          type="button"
          class="px-3 py-1 rounded-md cursor-pointer transition"
          :class="
            inputMode === 'fields'
              ? 'bg-emerald-500/20 text-emerald-300 font-semibold'
              : 'text-slate-400 hover:text-slate-200'
          "
          @click="switchMode('fields')"
        >
          表单参数
        </button>
        <button
          type="button"
          class="px-3 py-1 rounded-md cursor-pointer transition"
          :class="
            inputMode === 'url'
              ? 'bg-emerald-500/20 text-emerald-300 font-semibold'
              : 'text-slate-400 hover:text-slate-200'
          "
          @click="switchMode('url')"
        >
          连接 URL
        </button>
      </div>
    </div>

    <template v-if="inputMode === 'url'">
      <div>
        <label class="text-slate-400 block mb-1.5">Redis URL</label>
        <input
          :value="model.url"
          type="password"
          placeholder="redis://:password@127.0.0.1:6379/0"
          class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
          @input="updateField('url', ($event.target as HTMLInputElement).value)"
        />
      </div>
    </template>

    <template v-else>
      <div class="grid grid-cols-1 sm:grid-cols-12 gap-3">
        <div class="sm:col-span-6">
          <label class="text-slate-400 block mb-1.5">主机</label>
          <input
            :value="model.host"
            type="text"
            placeholder="127.0.0.1"
            class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
            @input="updateField('host', ($event.target as HTMLInputElement).value)"
          />
        </div>
        <div class="sm:col-span-3">
          <label class="text-slate-400 block mb-1.5">端口</label>
          <ObsNumberInput
            :model-value="model.port"
            :min="1"
            :max="65535"
            @update:model-value="(v) => updateField('port', v ?? 6379)"
          />
        </div>
        <div class="sm:col-span-3">
          <label class="text-slate-400 block mb-1.5">DB</label>
          <ObsNumberInput
            :model-value="model.db"
            :min="0"
            :max="15"
            @update:model-value="(v) => updateField('db', v ?? 0)"
          />
        </div>
      </div>

      <div class="grid grid-cols-1 sm:grid-cols-12 gap-3 items-end">
        <div class="sm:col-span-5">
          <label class="text-slate-400 block mb-1.5">密码</label>
          <input
            :value="model.password"
            type="password"
            placeholder="无密码留空"
            class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
            @input="updateField('password', ($event.target as HTMLInputElement).value)"
          />
        </div>
        <div class="sm:col-span-4">
          <label class="text-slate-400 block mb-1.5">用户名 (ACL)</label>
          <input
            :value="model.username"
            type="text"
            placeholder="默认空"
            class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
            @input="updateField('username', ($event.target as HTMLInputElement).value)"
          />
        </div>
        <div class="sm:col-span-3">
          <button
            type="button"
            class="w-full px-3 py-2 rounded-lg border flex items-center justify-between cursor-pointer transition"
            :class="
              model.tls
                ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/35'
                : 'bg-[#070a10] text-slate-400 border-slate-800 hover:text-slate-200'
            "
            @click="updateField('tls', !model.tls)"
          >
            <span>TLS 加密</span>
            <span class="font-mono-num font-semibold">{{ model.tls ? "ON" : "OFF" }}</span>
          </button>
        </div>
      </div>
    </template>

    <!-- 高级参数 -->
    <div class="obs-inset rounded-xl overflow-hidden">
      <button
        type="button"
        class="w-full flex items-center justify-between px-3.5 py-2.5 text-xs font-medium text-slate-300 hover:text-slate-100 transition cursor-pointer"
        @click="advancedOpen = !advancedOpen"
      >
        <span>高级参数（连接池、超时、Streams 与存量前缀）</span>
        <ChevronDown
          class="h-3.5 w-3.5 text-slate-500 transition-transform"
          :class="{ 'rotate-180': advancedOpen }"
        />
      </button>

      <div v-show="advancedOpen" class="p-4 border-t border-slate-800/80 space-y-3.5">
        <div class="grid grid-cols-2 sm:grid-cols-5 gap-3">
          <div>
            <label class="text-slate-400 block mb-1.5">Crawler 池</label>
            <ObsNumberInput
              :model-value="model.crawler_pool_size"
              :min="1"
              :max="2048"
              @update:model-value="(v) => updateField('crawler_pool_size', v ?? 100)"
            />
          </div>
          <div>
            <label class="text-slate-400 block mb-1.5">Indexer 池</label>
            <ObsNumberInput
              :model-value="model.indexer_pool_size"
              :min="1"
              :max="2048"
              @update:model-value="(v) => updateField('indexer_pool_size', v ?? 128)"
            />
          </div>
          <div>
            <label class="text-slate-400 block mb-1.5">建连超时 (s)</label>
            <ObsNumberInput
              :model-value="model.conn_timeout_secs"
              :min="1"
              :max="60"
              @update:model-value="(v) => updateField('conn_timeout_secs', v ?? 3)"
            />
          </div>
          <div>
            <label class="text-slate-400 block mb-1.5">等待超时 (s)</label>
            <ObsNumberInput
              :model-value="model.wait_timeout_secs"
              :min="1"
              :max="60"
              @update:model-value="(v) => updateField('wait_timeout_secs', v ?? 2)"
            />
          </div>
          <div>
            <label class="text-slate-400 block mb-1.5">回收超时 (s)</label>
            <ObsNumberInput
              :model-value="model.recycle_timeout_secs"
              :min="1"
              :max="60"
              @update:model-value="(v) => updateField('recycle_timeout_secs', v ?? 3)"
            />
          </div>
        </div>

        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3 font-mono-num">
          <div>
            <label class="text-slate-400 font-sans block mb-1.5">存量去重键前缀</label>
            <div class="flex items-center gap-2">
              <input
                :value="model.indexed_key_prefix"
                type="text"
                :disabled="lockIndexedPrefix && !prefixUnlocked"
                class="obs-input w-full rounded-lg px-2.5 py-1.5"
                @input="updateField('indexed_key_prefix', ($event.target as HTMLInputElement).value)"
              />
              <button
                v-if="lockIndexedPrefix"
                type="button"
                class="obs-btn-ghost p-1.5 rounded-lg cursor-pointer shrink-0"
                :class="prefixUnlocked ? 'border-amber-500/40! text-amber-300!' : ''"
                :title="prefixUnlocked ? '点击锁定前缀' : '点击解锁修改'"
                @click="prefixUnlocked = !prefixUnlocked"
              >
                <component :is="prefixUnlocked ? Unlock : Lock" class="h-3.5 w-3.5" />
              </button>
            </div>
          </div>

          <div>
            <label class="text-slate-400 font-sans block mb-1.5">Consumer Group</label>
            <input
              :value="model.consumer_group"
              type="text"
              class="obs-input w-full rounded-lg px-2.5 py-1.5"
              @input="updateField('consumer_group', ($event.target as HTMLInputElement).value)"
            />
          </div>

          <div>
            <label class="text-slate-400 font-sans block mb-1.5">新种子 Stream</label>
            <input
              :value="model.torrent_stream"
              type="text"
              class="obs-input w-full rounded-lg px-2.5 py-1.5"
              @input="updateField('torrent_stream', ($event.target as HTMLInputElement).value)"
            />
          </div>

          <div>
            <label class="text-slate-400 font-sans block mb-1.5">热度 Stream</label>
            <input
              :value="model.hot_stream"
              type="text"
              class="obs-input w-full rounded-lg px-2.5 py-1.5"
              @input="updateField('hot_stream', ($event.target as HTMLInputElement).value)"
            />
          </div>

          <div>
            <label class="text-slate-400 font-sans block mb-1.5">Stream MAXLEN</label>
            <ObsNumberInput
              :model-value="model.max_stream_len"
              :min="1000"
              :max="100000000"
              :step="100000"
              @update:model-value="(v) => updateField('max_stream_len', v ?? 3000000)"
            />
          </div>

          <div>
            <label class="text-slate-400 font-sans block mb-1.5">单条上限 (Bytes)</label>
            <ObsNumberInput
              :model-value="model.max_message_size"
              :min="1024"
              :max="67108864"
              :step="65536"
              @update:model-value="(v) => updateField('max_message_size', v ?? 1048576)"
            />
          </div>
        </div>
      </div>
    </div>

    <!-- 检测结果 -->
    <div
      v-if="testReport"
      class="rounded-xl border px-3.5 py-2.5 text-xs space-y-1.5"
      :class="
        testReport.ok
          ? 'border-emerald-500/30 bg-emerald-950/30 text-emerald-200'
          : 'border-rose-500/35 bg-rose-950/35 text-rose-200'
      "
    >
      <div class="flex items-center gap-2 font-medium">
        <CheckCircle2 v-if="testReport.ok" class="h-4 w-4 text-emerald-400 shrink-0" />
        <XCircle v-else class="h-4 w-4 text-rose-400 shrink-0" />
        <span>{{ testReport.message }}</span>
      </div>
      <div class="flex flex-wrap gap-4 font-mono-num text-[11px] opacity-90 pl-6">
        <span>{{ testReport.latency_ms }}ms</span>
        <span v-if="testReport.redis_version">v{{ testReport.redis_version }}</span>
        <span v-if="testReport.used_memory_human">{{ testReport.used_memory_human }}</span>
        <span v-if="testReport.dbsize !== null">{{ fmtCount(testReport.dbsize) }} keys</span>
        <span>Streams: {{ testReport.streams_supported ? "OK" : "不支持" }}</span>
        <span>读写探针: {{ testReport.rw_probe_ok ? "OK" : "失败" }}</span>
      </div>
    </div>
  </div>
</template>
