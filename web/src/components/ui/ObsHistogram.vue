<script setup lang="ts">
import { computed } from "vue";
import { fmtCount, fmtExact } from "@/utils/format";

export interface HistogramBarItem {
  label: string;
  value: number;
}

const props = defineProps<{
  bars: HistogramBarItem[];
  color: string;
}>();

const maxVal = computed(() => Math.max(...props.bars.map((b) => b.value), 1));
</script>

<template>
  <div
    class="grid gap-2.5 h-36 pt-2 items-end w-full"
    :style="{ gridTemplateColumns: `repeat(${Math.max(1, bars.length)}, minmax(0, 1fr))` }"
  >
    <div
      v-for="(b, idx) in bars"
      :key="idx"
      class="h-full flex flex-col items-center justify-end gap-1 min-w-0 group"
      :title="`${b.label}: ${fmtExact(b.value)}`"
    >
      <span class="text-[10px] font-mono-num text-slate-300 leading-none truncate max-w-full">
        {{ fmtCount(b.value) }}
      </span>
      <div class="w-full flex-1 flex items-end min-h-0 px-0.5">
        <div
          class="w-full rounded-t-md transition-opacity opacity-85 group-hover:opacity-100"
          :style="{
            height: `${Math.max(5, Math.round((b.value / maxVal) * 100))}%`,
            minHeight: '4px',
            background: `linear-gradient(180deg, ${color}, ${color}99)`,
          }"
        />
      </div>
      <span class="text-[10px] font-mono-num text-slate-500 leading-none truncate max-w-full">
        {{ b.label }}
      </span>
    </div>
  </div>
</template>
