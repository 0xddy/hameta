<script setup lang="ts">
import { computed } from "vue";
import { fmtCount, fmtExact } from "@/utils/format";

export interface HorizontalBarItem {
  label: string;
  value: number;
}

const props = defineProps<{
  items: HorizontalBarItem[];
  color: string;
}>();

const maxVal = computed(() => Math.max(...props.items.map((x) => x.value), 1));
</script>

<template>
  <div class="space-y-2 text-xs font-mono-num">
    <div v-for="item in items" :key="item.label" class="flex items-center gap-3">
      <span class="w-36 text-slate-400 truncate" :title="item.label">{{ item.label }}</span>
      <div class="flex-1 h-2 rounded-full bg-slate-800 overflow-hidden">
        <div
          class="h-full rounded-full transition-all duration-300"
          :style="{
            width: `${item.value > 0 ? Math.max(2, Math.round((item.value / maxVal) * 100)) : 0}%`,
            background: color,
          }"
        />
      </div>
      <span class="w-16 text-right text-slate-200" :title="fmtExact(item.value)">
        {{ fmtCount(item.value) }}
      </span>
    </div>
  </div>
</template>
