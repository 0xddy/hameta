<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Lock, Unlock } from "lucide-vue-next";
import type { RedisServiceConfig, RedisTestReport } from "@/api/setup";

const props = withDefaults(
  defineProps<{
    modelValue: RedisServiceConfig;
    testReport?: RedisTestReport | null;
    lockIndexedPrefix?: boolean;
  }>(),
  {
    testReport: null,
    lockIndexedPrefix: true,
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: RedisServiceConfig): void;
}>();

const inputMode = ref<"fields" | "url">(props.modelValue.url.trim() ? "url" : "fields");
const prefixUnlocked = ref(false);

watch(
  () => props.modelValue.url,
  (next) => {
    if (next.trim() && inputMode.value !== "url") {
      inputMode.value = "url";
    }
  },
);

const model = computed({
  get: () => props.modelValue,
  set: (v) => emit("update:modelValue", v),
});

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
  <div class="space-y-4">
    <div class="flex items-center justify-between gap-3">
      <div class="text-sm font-medium text-slate-700">连接配置模式</div>
      <el-radio-group
        :model-value="inputMode"
        size="small"
        @update:model-value="(val: 'fields' | 'url') => switchMode(val)"
      >
        <el-radio-button value="fields">分段表单</el-radio-button>
        <el-radio-button value="url">Redis URL 模式</el-radio-button>
      </el-radio-group>
    </div>

    <template v-if="inputMode === 'url'">
      <el-form-item label="完整连接 URL">
        <el-input
          :model-value="model.url"
          type="password"
          show-password
          placeholder="redis://:password@127.0.0.1:6379/0 或 rediss://..."
          @update:model-value="(v: string) => updateField('url', v)"
        />
        <div class="mt-1 text-xs text-slate-500">
          支持随时点击右侧眼睛图标明文查看或直接修改密码。填写显式 URL 时将优先使用该连接串。
        </div>
      </el-form-item>
    </template>

    <template v-else>
      <div class="grid grid-cols-1 gap-4 md:grid-cols-3">
        <el-form-item label="主机地址 (Host)" class="md:col-span-2">
          <el-input
            :model-value="model.host"
            placeholder="127.0.0.1"
            @update:model-value="(v: string) => updateField('host', v)"
          />
        </el-form-item>
        <el-form-item label="端口 (Port)">
          <el-input-number
            :model-value="model.port"
            :min="1"
            :max="65535"
            class="w-full!"
            @update:model-value="(v: number | undefined) => updateField('port', v ?? 6379)"
          />
        </el-form-item>
      </div>

      <div class="grid grid-cols-1 gap-4 md:grid-cols-4">
        <el-form-item label="用户名 (ACL 可留空)">
          <el-input
            :model-value="model.username"
            placeholder="默认空"
            @update:model-value="(v: string) => updateField('username', v)"
          />
        </el-form-item>
        <el-form-item label="连接密码 (完整回显)" class="md:col-span-2">
          <el-input
            :model-value="model.password"
            type="password"
            show-password
            placeholder="无密码可留空"
            @update:model-value="(v: string) => updateField('password', v)"
          />
        </el-form-item>
        <el-form-item label="DB 索引 / TLS">
          <div class="flex items-center gap-3 w-full">
            <el-input-number
              :model-value="model.db"
              :min="0"
              :max="15"
              class="w-24!"
              @update:model-value="(v: number | undefined) => updateField('db', v ?? 0)"
            />
            <el-switch
              :model-value="model.tls"
              inline-prompt
              active-text="TLS"
              inactive-text="TCP"
              @update:model-value="(v: boolean) => updateField('tls', v)"
            />
          </div>
        </el-form-item>
      </div>
    </template>

    <el-divider content-position="left">连接池与超时参数</el-divider>

    <div class="grid grid-cols-2 gap-4 md:grid-cols-5">
      <el-form-item label="Crawler 连接池">
        <el-input-number
          :model-value="model.crawler_pool_size"
          :min="1"
          :max="2048"
          class="w-full!"
          @update:model-value="(v: number | undefined) => updateField('crawler_pool_size', v ?? 100)"
        />
      </el-form-item>
      <el-form-item label="Indexer 连接池">
        <el-input-number
          :model-value="model.indexer_pool_size"
          :min="1"
          :max="2048"
          class="w-full!"
          @update:model-value="(v: number | undefined) => updateField('indexer_pool_size', v ?? 128)"
        />
      </el-form-item>
      <el-form-item label="建连超时 (s)">
        <el-input-number
          :model-value="model.conn_timeout_secs"
          :min="1"
          :max="60"
          class="w-full!"
          @update:model-value="(v: number | undefined) => updateField('conn_timeout_secs', v ?? 3)"
        />
      </el-form-item>
      <el-form-item label="等待超时 (s)">
        <el-input-number
          :model-value="model.wait_timeout_secs"
          :min="1"
          :max="60"
          class="w-full!"
          @update:model-value="(v: number | undefined) => updateField('wait_timeout_secs', v ?? 2)"
        />
      </el-form-item>
      <el-form-item label="回收超时 (s)">
        <el-input-number
          :model-value="model.recycle_timeout_secs"
          :min="1"
          :max="60"
          class="w-full!"
          @update:model-value="(v: number | undefined) => updateField('recycle_timeout_secs', v ?? 3)"
        />
      </el-form-item>
    </div>

    <el-divider content-position="left">去重键前缀与 Streams 队列配置</el-divider>

    <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
      <el-form-item label="存量去重键前缀 (兼容线上已索引数据)">
        <div class="flex items-center gap-2 w-full">
          <el-input
            :model-value="model.indexed_key_prefix"
            :disabled="lockIndexedPrefix && !prefixUnlocked"
            @update:model-value="(v: string) => updateField('indexed_key_prefix', v)"
          />
          <el-button
            v-if="lockIndexedPrefix"
            size="small"
            :type="prefixUnlocked ? 'warning' : 'default'"
            @click="prefixUnlocked = !prefixUnlocked"
          >
            <component :is="prefixUnlocked ? Unlock : Lock" class="h-3.5 w-3.5 mr-1" />
            {{ prefixUnlocked ? "锁定" : "解锁编辑" }}
          </el-button>
        </div>
        <div v-if="prefixUnlocked" class="mt-1 text-xs text-amber-600">
          高危提示：修改前缀将导致无法命中存量 metadata:indexed:* 标记，引发存量种子重复抓取！
        </div>
      </el-form-item>

      <el-form-item label="Consumer Group 名称">
        <el-input
          :model-value="model.consumer_group"
          @update:model-value="(v: string) => updateField('consumer_group', v)"
        />
      </el-form-item>

      <el-form-item label="新种子 Stream 键名">
        <el-input
          :model-value="model.torrent_stream"
          @update:model-value="(v: string) => updateField('torrent_stream', v)"
        />
      </el-form-item>

      <el-form-item label="热度更新 Stream 键名">
        <el-input
          :model-value="model.hot_stream"
          @update:model-value="(v: string) => updateField('hot_stream', v)"
        />
      </el-form-item>

      <el-form-item label="Stream MAXLEN ~ 裁剪上限">
        <el-input-number
          :model-value="model.max_stream_len"
          :min="1000"
          :max="100000000"
          :step="100000"
          class="w-full!"
          @update:model-value="(v: number | undefined) => updateField('max_stream_len', v ?? 3000000)"
        />
      </el-form-item>

      <el-form-item label="单条消息最大字节 (Bytes)">
        <el-input-number
          :model-value="model.max_message_size"
          :min="1024"
          :max="67108864"
          :step="65536"
          class="w-full!"
          @update:model-value="(v: number | undefined) => updateField('max_message_size', v ?? 1048576)"
        />
      </el-form-item>
    </div>

    <el-alert
      v-if="testReport"
      :type="testReport.ok ? 'success' : 'error'"
      show-icon
      :closable="false"
      :title="testReport.message"
    >
      <template #default>
        <div class="mt-1 flex flex-wrap gap-4 text-xs">
          <span>延迟: {{ testReport.latency_ms }} ms</span>
          <span v-if="testReport.redis_version">版本: v{{ testReport.redis_version }}</span>
          <span v-if="testReport.used_memory_human">内存: {{ testReport.used_memory_human }}</span>
          <span v-if="testReport.dbsize !== null">Keys: {{ testReport.dbsize }}</span>
          <span>Streams: {{ testReport.streams_supported ? "支持" : "不支持" }}</span>
          <span>XAUTOCLAIM: {{ testReport.xautoclaim_supported ? "原生" : "XPENDING 回退" }}</span>
          <span>读写探针: {{ testReport.rw_probe_ok ? "通过" : "失败" }}</span>
        </div>
      </template>
    </el-alert>
  </div>
</template>
