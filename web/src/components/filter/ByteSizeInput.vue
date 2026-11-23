<script setup lang="ts">
import { computed, ref, watch } from "vue";
import ObsNumberInput from "@/components/ui/ObsNumberInput.vue";
import ObsSelect from "@/components/ui/ObsSelect.vue";

const props = withDefaults(
  defineProps<{
    modelValue: number | null;
    allowEmpty?: boolean;
    min?: number;
    placeholder?: string;
  }>(),
  {
    allowEmpty: false,
    min: 0,
    placeholder: "未限制",
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: number | null): void;
}>();

type SizeUnit = "B" | "KB" | "MB" | "GB";

const UNIT_FACTOR: Record<SizeUnit, number> = {
  B: 1,
  KB: 1024,
  MB: 1024 * 1024,
  GB: 1024 * 1024 * 1024,
};

const UNIT_OPTIONS = [
  { label: "B", value: "B" },
  { label: "KB", value: "KB" },
  { label: "MB", value: "MB" },
  { label: "GB", value: "GB" },
];

function detectBestUnit(bytes: number | null): SizeUnit {
  if (bytes === null || bytes <= 0) return "MB";
  if (bytes % UNIT_FACTOR.GB === 0) return "GB";
  if (bytes % UNIT_FACTOR.MB === 0) return "MB";
  if (bytes % UNIT_FACTOR.KB === 0) return "KB";
  return "B";
}

const unit = ref<SizeUnit>(detectBestUnit(props.modelValue));
const displayValue = ref<number | undefined>(
  props.modelValue === null ? undefined : Number((props.modelValue / UNIT_FACTOR[unit.value]).toFixed(2)),
);

watch(
  () => props.modelValue,
  (next) => {
    if (next === null) {
      displayValue.value = undefined;
      return;
    }
    const factor = UNIT_FACTOR[unit.value];
    const currentBytes = displayValue.value === undefined ? null : Math.round(displayValue.value * factor);
    if (currentBytes !== next) {
      unit.value = detectBestUnit(next);
      displayValue.value = Number((next / UNIT_FACTOR[unit.value]).toFixed(2));
    }
  },
);

const scaledBytes = computed(() => {
  if (displayValue.value === undefined || !Number.isFinite(displayValue.value)) return null;
  return Math.max(props.min, Math.round(displayValue.value * UNIT_FACTOR[unit.value]));
});

function emitValue() {
  if (props.allowEmpty && displayValue.value === undefined) {
    emit("update:modelValue", null);
    return;
  }
  emit("update:modelValue", scaledBytes.value ?? props.min);
}

function onUnitChange() {
  if (props.modelValue !== null) {
    displayValue.value = Number((props.modelValue / UNIT_FACTOR[unit.value]).toFixed(2));
  }
  emitValue();
}
</script>

<template>
  <div class="flex items-center gap-1.5">
    <div class="flex-1 min-w-[96px]">
      <ObsNumberInput
        v-model="displayValue"
        :min="0"
        :step="unit === 'B' ? 512 : 1"
        :allow-empty="allowEmpty"
        :placeholder="placeholder"
        @change="emitValue"
      />
    </div>
    <div class="w-20 shrink-0">
      <ObsSelect
        v-model="unit"
        :options="UNIT_OPTIONS"
        mono
        trigger-class="text-xs py-1.5"
        @change="onUnitChange"
      />
    </div>
  </div>
</template>
