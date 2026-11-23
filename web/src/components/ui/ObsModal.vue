<script setup lang="ts">
import { onMounted, onUnmounted } from "vue";

const props = withDefaults(
  defineProps<{
    modelValue: boolean;
    title?: string;
    subtitle?: string;
    maxWidth?: string;
    closeOnBackdrop?: boolean;
  }>(),
  {
    title: "",
    subtitle: "",
    maxWidth: "520px",
    closeOnBackdrop: true,
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: boolean): void;
}>();

function close() {
  emit("update:modelValue", false);
}

function handleKeyDown(e: KeyboardEvent) {
  if (e.key === "Escape" && props.modelValue) {
    close();
  }
}

onMounted(() => document.addEventListener("keydown", handleKeyDown));
onUnmounted(() => document.removeEventListener("keydown", handleKeyDown));
</script>

<template>
  <Teleport to="body">
    <div
      v-if="modelValue"
      class="theme-obsidian fixed inset-0 z-50 flex items-center justify-center bg-black/65 backdrop-blur-xs p-4"
      @click.self="closeOnBackdrop && close()"
    >
      <div
        class="modal-enter obs-card w-full rounded-2xl overflow-hidden shadow-2xl flex flex-col max-h-[90vh]"
        :style="{ maxWidth }"
      >
        <div class="px-5 py-4 border-b border-slate-800 flex items-center justify-between gap-3 shrink-0">
          <slot name="header">
            <div class="min-w-0">
              <h3 class="text-sm font-semibold text-slate-100 truncate">{{ title }}</h3>
              <p v-if="subtitle" class="text-[11px] text-slate-500 mt-0.5 truncate">{{ subtitle }}</p>
            </div>
          </slot>
          <button
            type="button"
            class="obs-btn-ghost px-2.5 py-1 rounded text-xs cursor-pointer shrink-0"
            @click="close"
          >
            ✕
          </button>
        </div>

        <div class="p-5 overflow-y-auto obs-scroll text-xs">
          <slot />
        </div>

        <div
          v-if="$slots.footer"
          class="px-5 py-3.5 border-t border-slate-800 bg-[#0a0e16] flex flex-wrap items-center justify-end gap-2 shrink-0"
        >
          <slot name="footer" />
        </div>
      </div>
    </div>
  </Teleport>
</template>
