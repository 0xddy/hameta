<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";

export interface ObsSelectOption {
  label: string;
  value: string | number;
  disabled?: boolean;
}

const props = withDefaults(
  defineProps<{
    modelValue: string | number | null | undefined;
    options: ObsSelectOption[];
    placeholder?: string;
    disabled?: boolean;
    mono?: boolean;
    triggerClass?: string;
    menuMinWidth?: number;
  }>(),
  {
    placeholder: "请选择...",
    disabled: false,
    mono: false,
    triggerClass: "",
    menuMinWidth: 160,
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: string | number): void;
  (e: "change", value: string | number): void;
}>();

const open = ref(false);
const triggerRef = ref<HTMLButtonElement | null>(null);
const menuRef = ref<HTMLDivElement | null>(null);
const menuStyle = ref<Record<string, string>>({});

const selectedLabel = computed(() => {
  const matched = props.options.find((opt) => opt.value === props.modelValue);
  if (matched) return matched.label;
  if (props.modelValue !== undefined && props.modelValue !== null && props.modelValue !== "") {
    return String(props.modelValue);
  }
  return props.placeholder;
});

function positionMenu() {
  const trg = triggerRef.value;
  if (!trg) return;
  const rect = trg.getBoundingClientRect();
  const width = Math.max(rect.width, props.menuMinWidth);
  const left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8));
  const estHeight = Math.min(256, props.options.length * 36 + 14);
  const menuHeight = menuRef.value?.offsetHeight || estHeight;
  const spaceBelow = window.innerHeight - rect.bottom;
  const top =
    spaceBelow < menuHeight + 10 && rect.top > menuHeight + 10
      ? Math.max(8, rect.top - menuHeight - 6)
      : Math.min(window.innerHeight - menuHeight - 8, rect.bottom + 6);

  menuStyle.value = {
    minWidth: `${Math.round(width)}px`,
    maxWidth: `${Math.max(Math.round(width), 340)}px`,
    left: `${Math.round(left)}px`,
    top: `${Math.round(top)}px`,
  };
}

async function toggleOpen() {
  if (props.disabled) return;
  if (open.value) {
    open.value = false;
    return;
  }
  open.value = true;
  await nextTick();
  positionMenu();
}

function selectOption(opt: ObsSelectOption) {
  if (opt.disabled) return;
  open.value = false;
  emit("update:modelValue", opt.value);
  emit("change", opt.value);
}

function handleDocClick(e: MouseEvent) {
  if (!open.value) return;
  const t = e.target as Node | null;
  if (triggerRef.value?.contains(t) || menuRef.value?.contains(t)) return;
  open.value = false;
}

function handleKeyDown(e: KeyboardEvent) {
  if (e.key === "Escape" && open.value) open.value = false;
}

function handleExternalScroll(e: Event) {
  if (!open.value) return;
  const t = e.target as Node | null;
  if (menuRef.value && t && menuRef.value.contains(t)) return;
  open.value = false;
}

function handleResize() {
  if (open.value) open.value = false;
}

onMounted(() => {
  document.addEventListener("click", handleDocClick);
  document.addEventListener("keydown", handleKeyDown);
  document.addEventListener("scroll", handleExternalScroll, true);
  window.addEventListener("resize", handleResize);
});

onUnmounted(() => {
  document.removeEventListener("click", handleDocClick);
  document.removeEventListener("keydown", handleKeyDown);
  document.removeEventListener("scroll", handleExternalScroll, true);
  window.removeEventListener("resize", handleResize);
});
</script>

<template>
  <div class="obs-select-wrap">
    <button
      ref="triggerRef"
      type="button"
      :disabled="disabled"
      class="obs-input obs-select-trigger rounded-lg px-3 py-2 text-xs"
      :class="[
        mono ? 'font-mono-num' : '',
        open ? 'obs-select-open' : '',
        disabled ? 'opacity-45 cursor-not-allowed' : '',
        triggerClass,
      ]"
      @click.stop="toggleOpen"
    >
      <span class="truncate" :class="modelValue === '' || modelValue === null || modelValue === undefined ? 'text-slate-500' : ''">
        {{ selectedLabel }}
      </span>
      <svg
        class="obs-select-chevron w-3.5 h-3.5"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2.2"
      >
        <polyline points="6 9 12 15 18 9" />
      </svg>
    </button>

    <Teleport to="body">
      <div
        v-if="open"
        ref="menuRef"
        role="listbox"
        class="fixed z-[95] obs-select-menu obs-scroll p-1.5 max-h-64 overflow-y-auto"
        :style="menuStyle"
      >
        <button
          v-for="opt in options"
          :key="String(opt.value)"
          type="button"
          :disabled="opt.disabled"
          class="obs-select-opt"
          :class="[
            opt.value === modelValue ? 'is-selected' : '',
            mono ? 'tabular-nums' : '',
          ]"
          @click.stop="selectOption(opt)"
        >
          <span class="truncate">{{ opt.label }}</span>
          <svg
            v-if="opt.value === modelValue"
            class="w-3.5 h-3.5 text-emerald-400 shrink-0"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.4"
          >
            <polyline points="20 6 9 17 4 12" />
          </svg>
        </button>
      </div>
    </Teleport>
  </div>
</template>
