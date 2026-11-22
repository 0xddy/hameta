<script setup lang="ts">
import { computed } from "vue";
import { ShieldCheck } from "lucide-vue-next";
import type { ManticoreServiceConfig, ManticoreTestReport } from "@/api/setup";

const props = defineProps<{
  modelValue: ManticoreServiceConfig;
  testReport?: ManticoreTestReport | null;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: ManticoreServiceConfig): void;
}>();

const model = computed(() => props.modelValue);

function updateField<K extends keyof ManticoreServiceConfig>(
  key: K,
  value: ManticoreServiceConfig[K],
) {
  emit("update:modelValue", { ...props.modelValue, [key]: value });
}
</script>

<template>
  <div class="space-y-4">
    <el-alert
      type="info"
      :closable="false"
      show-icon
      title="存量表保护锁定：目标表固定为 torrents，hameta 绝不会执行任何 ALTER / DROP 操作"
    />

    <div class="grid grid-cols-1 gap-4 md:grid-cols-3">
      <el-form-item label="Manticore HTTP API URL" class="md:col-span-2">
        <el-input
          :model-value="model.url"
          placeholder="http://127.0.0.1:9308"
          @update:model-value="(v: string) => updateField('url', v)"
        />
      </el-form-item>

      <el-form-item label="锁定目标表名">
        <el-input :model-value="model.table_name" disabled>
          <template #prefix>
            <ShieldCheck class="h-4 w-4 text-emerald-500" />
          </template>
        </el-input>
      </el-form-item>
    </div>

    <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
      <el-form-item label="HTTP Basic Auth 用户名 (无鉴权留空)">
        <el-input
          :model-value="model.username"
          placeholder="与密码成对填写或同时留空"
          @update:model-value="(v: string) => updateField('username', v)"
        />
      </el-form-item>
      <el-form-item label="HTTP Basic Auth 密码 (完整回显)">
        <el-input
          :model-value="model.password"
          type="password"
          show-password
          placeholder="无鉴权留空，点击右侧眼睛图标明文查看"
          @update:model-value="(v: string) => updateField('password', v)"
        />
      </el-form-item>
    </div>

    <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
      <el-form-item label="HTTP 请求超时 (秒)">
        <el-input-number
          :model-value="model.request_timeout_secs"
          :min="1"
          :max="120"
          class="w-full!"
          @update:model-value="(v: number | undefined) => updateField('request_timeout_secs', v ?? 10)"
        />
      </el-form-item>

      <el-form-item label="HTTP 连接池空闲上限 (max_connections)">
        <el-input-number
          :model-value="model.max_connections"
          :min="1"
          :max="10000"
          class="w-full!"
          @update:model-value="(v: number | undefined) => updateField('max_connections', v ?? 1280)"
        />
      </el-form-item>
    </div>

    <div v-if="testReport" class="space-y-3">
      <el-alert
        :type="
          testReport.ok
            ? 'success'
            : testReport.schema && !testReport.schema.table_exists
              ? 'warning'
              : 'error'
        "
        show-icon
        :closable="false"
        :title="testReport.message"
      >
        <template #default>
          <div class="mt-1 flex flex-wrap gap-4 text-xs">
            <span>HTTP 耗时: {{ testReport.latency_ms }} ms</span>
            <span v-if="testReport.status?.indexed_documents !== null && testReport.status?.indexed_documents !== undefined">
              存量文档: {{ testReport.status.indexed_documents.toLocaleString() }} 条
            </span>
            <span v-if="testReport.status?.disk_bytes !== null && testReport.status?.disk_bytes !== undefined">
              磁盘占用: {{ (testReport.status.disk_bytes / 1024 / 1024).toFixed(2) }} MiB
            </span>
          </div>
        </template>
      </el-alert>

      <div
        v-if="testReport.schema && testReport.schema.columns.length > 0"
        class="rounded-lg border border-slate-200 overflow-hidden"
      >
        <el-table :data="testReport.schema.columns" size="small" stripe>
          <el-table-column prop="field" label="字段名 (DESCRIBE torrents)" min-width="140" />
          <el-table-column prop="column_type" label="类型" min-width="110" />
          <el-table-column prop="properties" label="属性" min-width="160" />
        </el-table>
      </div>
    </div>
  </div>
</template>
