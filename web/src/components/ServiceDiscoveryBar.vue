<script setup lang="ts">
import { Check, Container, KeyRound, Radar, RefreshCw, Server } from "lucide-vue-next";
import type { DetectedManticoreService, DetectedRedisService } from "@/api/setup";

defineProps<{
  kind: "redis" | "manticore";
  loading: boolean;
  dockerAvailable: boolean;
  redisItems?: DetectedRedisService[];
  manticoreItems?: DetectedManticoreService[];
  activeKey?: string;
}>();

const emit = defineEmits<{
  (e: "rescan"): void;
  (e: "select-redis", item: DetectedRedisService): void;
  (e: "select-manticore", item: DetectedManticoreService): void;
}>();

function sourceBadge(source: string): string {
  if (source === "docker") return "Docker";
  if (source === "env") return "ENV";
  return "本机";
}
</script>

<template>
  <div class="obs-inset rounded-xl p-3.5 space-y-2.5 text-xs">
    <div class="flex items-center justify-between gap-2">
      <div class="flex items-center gap-2 text-slate-300 font-medium">
        <span class="h-2 w-2 rounded-full bg-cyan-400" :class="{ 'animate-pulse': loading }" />
        <Radar class="h-3.5 w-3.5 text-cyan-400" :class="{ 'animate-spin': loading }" />
        <span>自动检测候选服务</span>
        <span
          v-if="dockerAvailable"
          class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-cyan-500/15 text-cyan-300 border border-cyan-500/30 font-mono-num text-[10px]"
        >
          <Container class="h-3 w-3" />
          Docker 可用
        </span>
      </div>

      <button
        type="button"
        class="inline-flex items-center gap-1 text-cyan-400 hover:underline transition disabled:opacity-50 cursor-pointer"
        :disabled="loading"
        @click="emit('rescan')"
      >
        <RefreshCw class="h-3 w-3" :class="{ 'animate-spin': loading }" />
        {{ loading ? "扫描中..." : "重新检测" }}
      </button>
    </div>

    <!-- Redis 候选服务 -->
    <template v-if="kind === 'redis'">
      <div
        v-if="loading && (!redisItems || redisItems.length === 0)"
        class="text-xs text-slate-500 py-1"
      >
        正在探测本机与 Docker 容器...
      </div>
      <div v-else-if="!redisItems || redisItems.length === 0" class="text-xs text-slate-500 py-1">
        未检测到运行中的 Redis 实例，可手动填写下方连接参数。
      </div>
      <div v-else class="grid grid-cols-1 sm:grid-cols-2 gap-2">
        <button
          v-for="item in redisItems"
          :key="`${item.host}:${item.port}`"
          type="button"
          class="group text-left rounded-lg border p-2.5 transition cursor-pointer flex items-start justify-between gap-2"
          :class="
            activeKey === `${item.host}:${item.port}`
              ? 'border-emerald-500/45 bg-emerald-500/10'
              : 'border-slate-800 bg-[#080a0f] hover:border-slate-700'
          "
          @click="emit('select-redis', item)"
        >
          <div class="min-w-0 space-y-0.5">
            <div class="font-semibold text-slate-100 flex items-center gap-1.5">
              <span
                class="h-1.5 w-1.5 rounded-full shrink-0"
                :class="item.reachable ? 'bg-emerald-400' : 'bg-slate-600'"
              />
              <span class="truncate">{{ item.label }}</span>
              <span class="px-1 rounded bg-slate-800 text-[10px] text-slate-300 font-normal shrink-0">
                {{ sourceBadge(item.source) }}
              </span>
            </div>
            <div class="font-mono-num text-[11px] text-slate-400 flex flex-wrap items-center gap-x-1.5">
              <span>{{ item.host }}:{{ item.port }}</span>
              <span v-if="item.version">· v{{ item.version }}</span>
              <span v-if="item.latency_ms !== null">· {{ item.latency_ms }}ms</span>
            </div>
            <div v-if="item.container_image" class="text-[10px] text-slate-500 font-mono-num truncate">
              {{ item.container_image }}
            </div>
          </div>

          <div class="flex flex-col items-end gap-1 shrink-0">
            <span
              v-if="activeKey === `${item.host}:${item.port}`"
              class="inline-flex items-center gap-0.5 text-[11px] text-emerald-300 font-medium"
            >
              <Check class="h-3 w-3" />
              已填入
            </span>
            <span v-else class="text-[11px] text-cyan-400 opacity-0 group-hover:opacity-100 transition">
              填入
            </span>
            <span
              v-if="item.requires_auth"
              class="inline-flex items-center gap-0.5 text-[11px] text-amber-300"
            >
              <KeyRound class="h-2.5 w-2.5" />
              需密码
            </span>
          </div>
        </button>
      </div>
    </template>

    <!-- Manticore 候选服务 -->
    <template v-else>
      <div
        v-if="loading && (!manticoreItems || manticoreItems.length === 0)"
        class="text-xs text-slate-500 py-1"
      >
        正在探测本机与 Docker 容器...
      </div>
      <div
        v-else-if="!manticoreItems || manticoreItems.length === 0"
        class="text-xs text-slate-500 py-1"
      >
        未检测到运行中的 ManticoreSearch 实例，可手动填写下方 API 地址。
      </div>
      <div v-else class="grid grid-cols-1 sm:grid-cols-2 gap-2">
        <button
          v-for="item in manticoreItems"
          :key="item.url"
          type="button"
          class="group text-left rounded-lg border p-2.5 transition cursor-pointer flex items-start justify-between gap-2"
          :class="
            activeKey === item.url
              ? 'border-emerald-500/45 bg-emerald-500/10'
              : 'border-slate-800 bg-[#080a0f] hover:border-slate-700'
          "
          @click="emit('select-manticore', item)"
        >
          <div class="min-w-0 space-y-0.5">
            <div class="font-semibold text-slate-100 flex items-center gap-1.5">
              <span
                class="h-1.5 w-1.5 rounded-full shrink-0"
                :class="item.reachable ? 'bg-emerald-400' : 'bg-slate-600'"
              />
              <span class="truncate">{{ item.label }}</span>
              <span class="px-1 rounded bg-slate-800 text-[10px] text-slate-300 font-normal shrink-0">
                {{ sourceBadge(item.source) }}
              </span>
            </div>
            <div class="font-mono-num text-[11px] text-slate-400 truncate">
              {{ item.url }}
              <span v-if="item.latency_ms !== null">· {{ item.latency_ms }}ms</span>
            </div>
            <div v-if="item.container_image" class="text-[10px] text-slate-500 font-mono-num truncate">
              {{ item.container_image }}
            </div>
          </div>

          <div class="flex flex-col items-end gap-1 shrink-0">
            <span
              v-if="activeKey === item.url"
              class="inline-flex items-center gap-0.5 text-[11px] text-emerald-300 font-medium"
            >
              <Check class="h-3 w-3" />
              已填入
            </span>
            <span v-else class="text-[11px] text-cyan-400 opacity-0 group-hover:opacity-100 transition">
              填入
            </span>
            <span
              v-if="item.has_torrents_table === true"
              class="inline-flex items-center gap-0.5 px-1.5 py-0.5 rounded bg-emerald-500/15 text-emerald-300 text-[10px]"
            >
              <Server class="h-2.5 w-2.5" />
              含 torrents
            </span>
            <span
              v-else-if="item.has_torrents_table === false"
              class="px-1.5 py-0.5 rounded bg-amber-500/15 text-amber-300 text-[10px]"
            >
              空库
            </span>
            <span
              v-else-if="item.requires_auth"
              class="inline-flex items-center gap-0.5 text-[11px] text-amber-300"
            >
              <KeyRound class="h-2.5 w-2.5" />
              需鉴权
            </span>
          </div>
        </button>
      </div>
    </template>
  </div>
</template>
