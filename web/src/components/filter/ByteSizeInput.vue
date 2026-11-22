<script setup lang="ts">
import { computed, ref, watch } from "vue";

type UnitKey = "B" | "KB" | "MB" | "GB" | "TB";

const UNIT_FACTOR: Record<UnitKey, number> = {
  B: 1,
  KB: 1024,
  MB: 1024 * 1024,
  GB: 1024 * 1024 * 1024,
  TB: 1024 * 1024 * 1024 * 1024,
};

const props = withDefaults(
  defineProps<{
    modelValue: number;
    min?: number;
    disabled?: boolean;
  }>(),
  {
    min: 0,
    disabled: false,
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: number): void;
}>();

function pickBestUnit(bytes: number): UnitKey {
  if (!bytes || bytes <= 0) return "MB";
  const order: UnitKey[] = ["TB", "GB", "MB", "KB", "B"];
  for (const unit of order) {
    const factor = UNIT_FACTOR[unit];
    if (bytes >= factor && bytes % factor === 0) {
      return unit;
    }
  }
  if (bytes >= UNIT_FACTOR.TB) return "TB";
  if (bytes >= UNIT_FACTOR.GB) return "GB";
  if (bytes >= UNIT_FACTOR.MB) return "MB";
  if (bytes >= UNIT_FACTOR.KB) return "KB";
  return "B";
}

const unit = ref<UnitKey>(pickBestUnit(props.modelValue));

watch(
  () => props.modelValue,
  (val) => {
    const currentFactor = UNIT_FACTOR[unit.value];
    if (val > 0 && (val < currentFactor || val % currentFactor !== 0)) {
      unit.value = pickBestUnit(val);
    }
  },
);

const displayValue = computed<number>({
  get() {
    const factor = UNIT_FACTOR[unit.value];
    return Number((props.modelValue / factor).toFixed(2));
  },
  set(val: number) {
    const safeVal = Number.isFinite(val) ? val : 0;
    const rawBytes = Math.round(safeVal * UNIT_FACTOR[unit.value]);
    emit("update:modelValue", Math.max(props.min, rawBytes));
  },
});

function handleUnitChange(nextUnit: UnitKey) {
  const prevFactor = UNIT_FACTOR[unit.value];
  const currentNum = props.modelValue / prevFactor;
  unit.value = nextUnit;
  const nextBytes = Math.round(currentNum * UNIT_FACTOR[nextUnit]);
  emit("update:modelValue", Math.max(props.min, nextBytes));
}
</script>

<template>
  <div class="inline-flex items-center gap-1.5">
    <el-input-number
      v-model="displayValue"
      :min="0"
      :precision="unit === 'B' ? 0 : 2"
      :step="1"
      :disabled="disabled"
      size="small"
      controls-position="right"
      style="width: 118px"
    />
    <el-select
      :model-value="unit"
      size="small"
      :disabled="disabled"
      style="width: 72px"
      @change="handleUnitChange"
    >
      <el-option label="B" value="B" />
      <el-option label="KB" value="KB" />
      <el-option label="MB" value="MB" />
      <el-option label="GB" value="GB" />
      <el-option label="TB" value="TB" />
    </el-select>
  </div>
</template>
