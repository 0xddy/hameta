<script setup lang="ts">
import { computed, ref } from "vue";

const props = withDefaults(
  defineProps<{
    modelValue: number | undefined | null;
    min?: number;
    max?: number;
    step?: number;
    placeholder?: string;
    disabled?: boolean;
    allowEmpty?: boolean;
    inputClass?: string;
  }>(),
  {
    min: undefined,
    max: undefined,
    step: 1,
    placeholder: "",
    disabled: false,
    allowEmpty: false,
    inputClass: "",
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: number | undefined): void;
  (e: "change", value: number | undefined): void;
}>();

const inputRef = ref<HTMLInputElement | null>(null);

const displayVal = computed(() => {
  if (props.modelValue === undefined || props.modelValue === null || Number.isNaN(props.modelValue)) {
    return "";
  }
  return String(props.modelValue);
});

const stepDecimals = computed(() => {
  const s = String(props.step || 1);
  const dot = s.indexOf(".");
  return dot >= 0 ? s.length - dot - 1 : 0;
});

function clampVal(n: number): number {
  let out = n;
  if (props.min !== undefined && out < props.min) out = props.min;
  if (props.max !== undefined && out > props.max) out = props.max;
  const factor = Math.pow(10, stepDecimals.value);
  return stepDecimals.value > 0 ? Math.round(out * factor) / factor : Math.round(out);
}

function handleInput(e: Event) {
  const raw = (e.target as HTMLInputElement).value.trim();
  if (raw === "") {
    if (props.allowEmpty) {
      emit("update:modelValue", undefined);
      emit("change", undefined);
    }
    return;
  }
  const n = Number(raw);
  if (Number.isFinite(n)) {
    emit("update:modelValue", n);
    emit("change", n);
  }
}

function handleBlur(e: Event) {
  const raw = (e.target as HTMLInputElement).value.trim();
  if (raw === "") {
    if (props.allowEmpty) {
      emit("update:modelValue", undefined);
    } else {
      const fallback = props.min !== undefined ? props.min : 0;
      emit("update:modelValue", fallback);
      emit("change", fallback);
    }
    return;
  }
  const n = Number(raw);
  if (Number.isFinite(n)) {
    const clamped = clampVal(n);
    if (clamped !== props.modelValue) {
      emit("update:modelValue", clamped);
      emit("change", clamped);
    }
  }
}

function stepBy(delta: 1 | -1) {
  if (props.disabled) return;
  let base: number;
  if (props.modelValue === undefined || props.modelValue === null || Number.isNaN(props.modelValue)) {
    const ph = Number(props.placeholder);
    base = props.placeholder !== "" && Number.isFinite(ph) ? ph : props.min ?? 0;
  } else {
    base = props.modelValue;
  }
  const next = clampVal(base + delta * (props.step || 1));
  emit("update:modelValue", next);
  emit("change", next);
}
</script>

<template>
  <div class="obs-num-wrap w-full">
    <input
      ref="inputRef"
      type="number"
      :value="displayVal"
      :min="min"
      :max="max"
      :step="step"
      :placeholder="placeholder"
      :disabled="disabled"
      class="obs-input w-full rounded-lg px-3 py-2 text-xs font-mono-num"
      :class="inputClass"
      @input="handleInput"
      @blur="handleBlur"
    />
    <div class="obs-num-spin">
      <button type="button" tabindex="-1" :disabled="disabled" aria-label="增加" @click="stepBy(1)">
        <svg class="w-2.5 h-2.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6">
          <polyline points="18 15 12 9 6 15" />
        </svg>
      </button>
      <button type="button" tabindex="-1" :disabled="disabled" aria-label="减少" @click="stepBy(-1)">
        <svg class="w-2.5 h-2.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6">
          <polyline points="6 9 12 15 18 9" />
        </svg>
      </button>
    </div>
  </div>
</template>
