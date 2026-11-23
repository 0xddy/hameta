<script setup lang="ts">
import { computed, reactive, ref, watch } from "vue";
import { ElMessage } from "element-plus";
import {
  AlertTriangle,
  CheckCircle2,
  Copy,
  Cpu,
  RefreshCw,
  Server,
  ShieldAlert,
  SlidersHorizontal,
} from "lucide-vue-next";
import { ApiError } from "@/api/client";
import {
  getNofileSettings,
  updateNofileSettings,
  type NofilePresetItem,
  type NofileStatusResponse,
  type UpdateNofileResponse,
} from "@/api/settings";
import ObsModal from "@/components/ui/ObsModal.vue";
import { copyToClipboard, fmtCount, fmtExact } from "@/utils/format";

const props = defineProps<{
  modelValue: boolean;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: boolean): void;
  (e: "updated", status: NofileStatusResponse): void;
}>();

const loading = ref(false);
const saving = ref(false);
const loadError = ref("");
const submitError = ref("");
const status = ref<NofileStatusResponse | null>(null);
const lastResult = ref<UpdateNofileResponse | null>(null);
const syncSoftHard = ref(true);

const form = reactive({
  apply_process: true,
  process_soft: 262144,
  process_hard: 262144,
  apply_to_workers: true,
  persist_app_config: true,
  apply_system: false,
  system_file_max: 1048576,
  system_nr_open: 1048576,
  persist_system_files: true,
});

const visible = computed({
  get: () => props.modelValue,
  set: (v: boolean) => emit("update:modelValue", v),
});

const hardLowerWarning = computed(() => {
  const curHard = status.value?.process_hard;
  if (!form.apply_process || curHard == null) return false;
  return form.process_hard < curHard && !status.value?.is_root;
});

function populateFormFromStatus(st: NofileStatusResponse) {
  const curSoft = st.process_soft ?? st.saved_config?.process_soft ?? 262144;
  const curHard = st.process_hard ?? st.saved_config?.process_hard ?? curSoft;
  form.apply_process = true;
  form.process_soft = curSoft;
  form.process_hard = curHard;
  syncSoftHard.value = curSoft === curHard;
  form.apply_to_workers = true;
  form.persist_app_config = true;

  form.apply_system = st.system_supported && st.can_write_sysctl;
  form.system_file_max = st.system_file_max ?? st.saved_config?.system_file_max ?? 1048576;
  form.system_nr_open = st.system_nr_open ?? st.saved_config?.system_nr_open ?? 1048576;
  form.persist_system_files = st.can_write_limits_d;
}

async function fetchStatus(resetForm = false) {
  loading.value = true;
  loadError.value = "";
  try {
    const res = await getNofileSettings();
    status.value = res;
    if (resetForm) {
      populateFormFromStatus(res);
    }
  } catch (err) {
    loadError.value = err instanceof Error ? err.message : "读取 NOFILE 状态失败";
  } finally {
    loading.value = false;
  }
}

function applyPreset(preset: NofilePresetItem) {
  form.apply_process = true;
  form.process_soft = preset.process_soft;
  form.process_hard = preset.process_hard;
  syncSoftHard.value = preset.process_soft === preset.process_hard;
  if (status.value?.system_supported) {
    form.system_file_max = Math.max(
      preset.system_file_max,
      status.value.system_file_max ?? preset.system_file_max,
    );
    form.system_nr_open = Math.max(
      preset.system_nr_open,
      status.value.system_nr_open ?? preset.system_nr_open,
    );
  }
  submitError.value = "";
}

function onSoftInput(val: number) {
  form.process_soft = val;
  if (syncSoftHard.value || form.process_hard < val) {
    form.process_hard = val;
  }
}

function onHardInput(val: number) {
  form.process_hard = val;
  if (syncSoftHard.value) {
    form.process_soft = val;
  }
}

async function handleSubmit() {
  submitError.value = "";
  if (!form.apply_process && !form.apply_system) {
    submitError.value = "请至少勾选「程序 File Limit」或「系统内核 File Limit」之一";
    return;
  }
  if (form.apply_process) {
    if (!Number.isFinite(form.process_soft) || form.process_soft < 1024) {
      submitError.value = "程序软上限不能小于 1024";
      return;
    }
    if (!Number.isFinite(form.process_hard) || form.process_hard < form.process_soft) {
      submitError.value = "程序硬上限不能小于软上限";
      return;
    }
  }
  if (form.apply_system) {
    if (!Number.isFinite(form.system_file_max) || form.system_file_max < 65536) {
      submitError.value = "系统 fs.file-max 不能小于 65536";
      return;
    }
    if (!Number.isFinite(form.system_nr_open) || form.system_nr_open < 65536) {
      submitError.value = "系统 fs.nr_open 不能小于 65536";
      return;
    }
    if (form.system_file_max < form.system_nr_open) {
      submitError.value = "系统 fs.file-max 不能小于 fs.nr_open";
      return;
    }
    if (form.apply_process && form.system_nr_open < form.process_hard) {
      submitError.value = "系统 fs.nr_open 不能小于程序硬上限";
      return;
    }
  }

  saving.value = true;
  try {
    const res = await updateNofileSettings({
      apply_process: form.apply_process,
      process_soft: form.apply_process ? Math.floor(form.process_soft) : null,
      process_hard: form.apply_process ? Math.floor(form.process_hard) : null,
      apply_to_workers: form.apply_to_workers,
      persist_app_config: form.persist_app_config,
      apply_system: form.apply_system,
      system_file_max: form.apply_system ? Math.floor(form.system_file_max) : null,
      system_nr_open: form.apply_system ? Math.floor(form.system_nr_open) : null,
      persist_system_files: form.persist_system_files,
    });
    lastResult.value = res;
    status.value = res.status;
    if (res.status.process_soft != null) {
      form.process_soft = res.status.process_soft;
    }
    if (res.status.process_hard != null) {
      form.process_hard = res.status.process_hard;
    }
    if (res.status.system_file_max != null) {
      form.system_file_max = res.status.system_file_max;
    }
    if (res.status.system_nr_open != null) {
      form.system_nr_open = res.status.system_nr_open;
    }
    emit("updated", res.status);

    if (res.ok && res.warnings.length === 0) {
      ElMessage.success("File Limit 配置已成功应用");
    } else if (res.process_applied || res.system_applied) {
      ElMessage.warning("File Limit 已部分生效，请查看弹窗内权限与告警说明");
    } else {
      ElMessage.warning("未能直接修改内核/进程限制，请参考弹窗内的手动配置命令");
    }
  } catch (err) {
    submitError.value = err instanceof ApiError ? err.message : err instanceof Error ? err.message : "提交失败";
  } finally {
    saving.value = false;
  }
}

async function copyManualCommands() {
  const cmds = lastResult.value?.manual_commands ?? [];
  if (cmds.length === 0) return;
  const ok = await copyToClipboard(cmds.join("\n"));
  if (ok) ElMessage.success("已复制命令行配置脚本");
  else ElMessage.warning("剪贴板复制失败");
}

function fmtLimit(val: number | null | undefined): string {
  if (val == null) return "N/A";
  return `${fmtCount(val)} (${val.toLocaleString("en-US")})`;
}

watch(
  () => props.modelValue,
  (open) => {
    if (open) {
      lastResult.value = null;
      submitError.value = "";
      void fetchStatus(true);
    }
  },
);
</script>

<template>
  <ObsModal
    v-model="visible"
    title="文件描述符上限管理 (NOFILE LIMIT)"
    subtitle="同时查看与调整程序进程 RLIMIT_NOFILE（含运行中子进程热更新）及操作系统内核文件句柄上限"
    max-width="700px"
  >
    <div class="space-y-4">
      <!-- 顶部环境与刷新栏 -->
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div v-if="status" class="flex flex-wrap items-center gap-1.5 font-mono-num text-[11px]">
          <span class="px-2 py-0.5 rounded bg-slate-800/80 border border-slate-700/80 text-slate-300 uppercase">
            {{ status.platform }}
          </span>
          <span
            class="px-2 py-0.5 rounded border"
            :class="
              status.is_root
                ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30'
                : 'bg-amber-500/15 text-amber-300 border-amber-500/30'
            "
          >
            {{ status.is_root ? "ROOT 权限" : "非 ROOT 用户" }}
          </span>
          <span
            v-if="status.in_container"
            class="px-2 py-0.5 rounded bg-cyan-500/15 text-cyan-300 border border-cyan-500/30"
          >
            容器环境
          </span>
          <span class="text-slate-500">Panel PID #{{ status.panel_pid }}</span>
        </div>
        <div v-else class="text-slate-500 text-xs">正在探测本机与进程限制...</div>

        <button
          type="button"
          class="obs-btn-ghost px-2.5 py-1 rounded-lg text-[11px] inline-flex items-center gap-1.5 cursor-pointer"
          :disabled="loading"
          @click="fetchStatus(false)"
        >
          <RefreshCw class="w-3.5 h-3.5" :class="{ 'animate-spin': loading }" />
          重新探测
        </button>
      </div>

      <div
        v-if="loadError"
        class="rounded-xl border border-rose-500/40 bg-rose-950/35 px-3.5 py-2.5 text-xs text-rose-200"
      >
        {{ loadError }}
      </div>

      <!-- 实时遥测双卡片：程序级 vs 系统级 -->
      <div v-if="status" class="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <div class="obs-inset rounded-xl p-3.5 space-y-2">
          <div class="flex items-center justify-between text-[11px]">
            <span class="inline-flex items-center gap-1.5 font-semibold text-slate-200">
              <Cpu class="w-3.5 h-3.5 text-emerald-400" />
              程序当前 RLIMIT_NOFILE
            </span>
            <span
              class="font-mono-num text-[10px] px-1.5 py-0.5 rounded"
              :class="
                status.supported
                  ? 'bg-emerald-500/15 text-emerald-300'
                  : 'bg-slate-800 text-slate-400'
              "
            >
              {{ status.supported ? "支持在线调整" : "平台不支持" }}
            </span>
          </div>
          <div class="grid grid-cols-2 gap-2 font-mono-num pt-1">
            <div>
              <div class="text-[10px] text-slate-500">软上限 (Soft)</div>
              <div
                class="text-sm font-bold text-emerald-300 mt-0.5"
                :title="status.process_soft != null ? fmtExact(status.process_soft) : undefined"
              >
                {{ status.process_soft != null ? fmtCount(status.process_soft) : "N/A" }}
              </div>
              <div v-if="status.process_soft != null" class="text-[10px] text-slate-500">
                {{ status.process_soft.toLocaleString("en-US") }}
              </div>
            </div>
            <div>
              <div class="text-[10px] text-slate-500">硬上限 (Hard)</div>
              <div
                class="text-sm font-bold text-cyan-300 mt-0.5"
                :title="status.process_hard != null ? fmtExact(status.process_hard) : undefined"
              >
                {{ status.process_hard != null ? fmtCount(status.process_hard) : "N/A" }}
              </div>
              <div v-if="status.process_hard != null" class="text-[10px] text-slate-500">
                {{ status.process_hard.toLocaleString("en-US") }}
              </div>
            </div>
          </div>
          <div class="pt-1.5 border-t border-slate-800/70 flex items-center justify-between text-[11px] font-mono-num text-slate-400">
            <span>Panel 已打开 FD</span>
            <span class="text-slate-200">
              {{ status.process_open_fds != null ? status.process_open_fds.toLocaleString("en-US") : "N/A" }}
            </span>
          </div>
        </div>

        <div class="obs-inset rounded-xl p-3.5 space-y-2">
          <div class="flex items-center justify-between text-[11px]">
            <span class="inline-flex items-center gap-1.5 font-semibold text-slate-200">
              <Server class="w-3.5 h-3.5 text-cyan-400" />
              操作系统内核上限
            </span>
            <span
              class="font-mono-num text-[10px] px-1.5 py-0.5 rounded"
              :class="
                status.can_write_sysctl
                  ? 'bg-emerald-500/15 text-emerald-300'
                  : 'bg-amber-500/15 text-amber-300'
              "
            >
              {{
                !status.system_supported
                  ? "仅限 Linux"
                  : status.can_write_sysctl
                    ? "/proc/sys 可写"
                    : "/proc/sys 只读"
              }}
            </span>
          </div>
          <div class="grid grid-cols-2 gap-2 font-mono-num pt-1">
            <div>
              <div class="text-[10px] text-slate-500">fs.file-max</div>
              <div
                class="text-sm font-bold text-slate-100 mt-0.5"
                :title="status.system_file_max != null ? fmtExact(status.system_file_max) : undefined"
              >
                {{ status.system_file_max != null ? fmtCount(status.system_file_max) : "N/A" }}
              </div>
              <div v-if="status.system_file_max != null" class="text-[10px] text-slate-500 truncate">
                {{ status.system_file_max.toLocaleString("en-US") }}
              </div>
            </div>
            <div>
              <div class="text-[10px] text-slate-500">fs.nr_open</div>
              <div
                class="text-sm font-bold text-slate-100 mt-0.5"
                :title="status.system_nr_open != null ? fmtExact(status.system_nr_open) : undefined"
              >
                {{ status.system_nr_open != null ? fmtCount(status.system_nr_open) : "N/A" }}
              </div>
              <div v-if="status.system_nr_open != null" class="text-[10px] text-slate-500 truncate">
                {{ status.system_nr_open.toLocaleString("en-US") }}
              </div>
            </div>
          </div>
          <div class="pt-1.5 border-t border-slate-800/70 flex items-center justify-between text-[11px] font-mono-num text-slate-400">
            <span>系统已分配 FD</span>
            <span class="text-slate-200">
              {{ status.system_allocated_fds != null ? status.system_allocated_fds.toLocaleString("en-US") : "N/A" }}
            </span>
          </div>
        </div>
      </div>

      <!-- 快捷推荐档位 -->
      <div v-if="status && status.recommended_presets.length > 0" class="space-y-1.5">
        <div class="flex items-center justify-between text-[11px] text-slate-400">
          <span class="inline-flex items-center gap-1.5">
            <SlidersHorizontal class="w-3.5 h-3.5 text-slate-500" />
            快捷档位填充
          </span>
          <span v-if="status.saved_config" class="font-mono-num text-[10px] text-slate-500">
            数据库已存目标: {{ fmtCount(status.saved_config.process_soft) }} / {{ fmtCount(status.saved_config.process_hard) }}
          </span>
        </div>
        <div class="grid grid-cols-2 sm:grid-cols-4 gap-2">
          <button
            v-for="preset in status.recommended_presets"
            :key="preset.id"
            type="button"
            class="rounded-xl border px-2.5 py-2 text-left transition cursor-pointer"
            :class="
              form.process_soft === preset.process_soft && form.process_hard === preset.process_hard
                ? 'border-emerald-500/50 bg-emerald-500/12 text-emerald-200'
                : 'border-slate-800 bg-[#090d15] text-slate-300 hover:border-slate-700 hover:bg-slate-800/40'
            "
            :title="preset.description"
            @click="applyPreset(preset)"
          >
            <div class="flex items-center justify-between gap-1">
              <span class="font-mono-num font-semibold text-xs">{{ preset.label }}</span>
            </div>
            <div class="text-[10px] text-slate-400 mt-0.5 truncate">{{ preset.badge }}</div>
          </button>
        </div>
      </div>

      <!-- 区块一：程序级 File Limit -->
      <div class="obs-inset rounded-xl p-3.5 space-y-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <label class="inline-flex items-center gap-2 cursor-pointer select-none">
            <input
              v-model="form.apply_process"
              type="checkbox"
              class="accent-emerald-500 rounded cursor-pointer"
            />
            <span class="text-xs font-semibold text-slate-100">修改程序 File Limit (RLIMIT_NOFILE)</span>
          </label>
          <label
            v-if="form.apply_process"
            class="inline-flex items-center gap-1.5 text-[11px] text-slate-400 cursor-pointer select-none"
          >
            <input
              v-model="syncSoftHard"
              type="checkbox"
              class="accent-cyan-500 rounded cursor-pointer"
              @change="syncSoftHard && (form.process_hard = form.process_soft)"
            />
            软硬上限保持一致
          </label>
        </div>

        <div v-if="form.apply_process" class="space-y-3">
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div>
              <label class="text-[11px] text-slate-400 block mb-1">
                软上限 Soft Limit
                <span class="font-mono-num text-slate-500">(1,024 ~ 10,485,760)</span>
              </label>
              <input
                :value="form.process_soft"
                type="number"
                min="1024"
                max="10485760"
                step="1024"
                class="obs-input w-full rounded-lg px-3 py-1.5 font-mono-num text-xs"
                @input="onSoftInput(Number(($event.target as HTMLInputElement).value) || 0)"
              />
            </div>
            <div>
              <label class="text-[11px] text-slate-400 block mb-1">
                硬上限 Hard Limit
                <span class="font-mono-num text-slate-500">(>= Soft Limit)</span>
              </label>
              <input
                :value="form.process_hard"
                type="number"
                min="1024"
                max="10485760"
                step="1024"
                class="obs-input w-full rounded-lg px-3 py-1.5 font-mono-num text-xs"
                @input="onHardInput(Number(($event.target as HTMLInputElement).value) || 0)"
              />
            </div>
          </div>

          <div
            v-if="hardLowerWarning"
            class="rounded-lg border border-amber-500/40 bg-amber-950/35 px-3 py-2 text-[11px] text-amber-200 flex items-start gap-2"
          >
            <AlertTriangle class="w-3.5 h-3.5 text-amber-400 shrink-0 mt-0.5" />
            <span>非 root 进程调低 Hard Limit 后无法再自行调高，请确认目标硬上限值。</span>
          </div>

          <div class="flex flex-wrap items-center gap-x-5 gap-y-2 text-[11px] text-slate-300">
            <label class="inline-flex items-center gap-1.5 cursor-pointer select-none">
              <input
                v-model="form.apply_to_workers"
                type="checkbox"
                class="accent-emerald-500 rounded cursor-pointer"
              />
              <span>通过 Linux prlimit 热更新所有运行中子进程（{{ status?.running_workers.length ?? 0 }} 个）</span>
            </label>
            <label class="inline-flex items-center gap-1.5 cursor-pointer select-none">
              <input
                v-model="form.persist_app_config"
                type="checkbox"
                class="accent-emerald-500 rounded cursor-pointer"
              />
              <span>保存到面板数据库（下次启动自动应用）</span>
            </label>
          </div>

          <!-- 运行中 Worker 子进程实时 NOFILE 表 -->
          <div
            v-if="status && status.running_workers.length > 0"
            class="rounded-lg border border-slate-800/80 bg-[#070a10] overflow-hidden"
          >
            <div class="px-3 py-1.5 bg-[#0b101a] border-b border-slate-800/70 text-[10px] font-mono-num text-slate-400 flex items-center justify-between">
              <span>运行中受管子进程 NOFILE 状态</span>
              <span>{{ status.running_workers.length }} PIDs</span>
            </div>
            <div class="divide-y divide-slate-800/60 max-h-32 overflow-y-auto obs-scroll font-mono-num text-[11px]">
              <div
                v-for="w in status.running_workers"
                :key="w.id"
                class="px-3 py-1.5 flex items-center justify-between gap-2"
              >
                <div class="flex items-center gap-2 min-w-0">
                  <span
                    class="px-1.5 py-0.5 rounded text-[9px] uppercase shrink-0"
                    :class="
                      w.role === 'crawler'
                        ? 'bg-emerald-500/15 text-emerald-300'
                        : 'bg-cyan-500/15 text-cyan-300'
                    "
                  >
                    {{ w.role }}
                  </span>
                  <span class="text-slate-200 truncate">{{ w.id }}</span>
                  <span class="text-slate-500 shrink-0">#{{ w.pid }}</span>
                </div>
                <div class="flex items-center gap-3 shrink-0 text-[10px]">
                  <span class="text-slate-400">
                    FD {{ w.open_fds != null ? w.open_fds : "-" }}
                  </span>
                  <span class="text-emerald-300">
                    {{ w.soft_limit != null ? fmtCount(w.soft_limit) : "N/A" }}
                    <span class="text-slate-600">/</span>
                    {{ w.hard_limit != null ? fmtCount(w.hard_limit) : "N/A" }}
                  </span>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 区块二：系统级 File Limit -->
      <div class="obs-inset rounded-xl p-3.5 space-y-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <label class="inline-flex items-center gap-2 cursor-pointer select-none">
            <input
              v-model="form.apply_system"
              type="checkbox"
              :disabled="status ? !status.system_supported : false"
              class="accent-cyan-500 rounded cursor-pointer"
            />
            <span class="text-xs font-semibold text-slate-100">修改系统内核 File Limit (fs.file-max / fs.nr_open)</span>
          </label>
          <span v-if="status && !status.can_write_sysctl" class="text-[10px] font-mono-num text-amber-300">
            {{ status.in_container ? "容器 /proc/sys 只读，提交将生成命令" : "需 root 权限" }}
          </span>
        </div>

        <div v-if="form.apply_system" class="space-y-3">
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div>
              <label class="text-[11px] text-slate-400 block mb-1">
                fs.file-max
                <span class="font-mono-num text-slate-500">(系统全局最大句柄)</span>
              </label>
              <input
                v-model.number="form.system_file_max"
                type="number"
                min="65536"
                max="100000000"
                step="65536"
                class="obs-input w-full rounded-lg px-3 py-1.5 font-mono-num text-xs"
              />
            </div>
            <div>
              <label class="text-[11px] text-slate-400 block mb-1">
                fs.nr_open
                <span class="font-mono-num text-slate-500">(单进程硬上限顶值)</span>
              </label>
              <input
                v-model.number="form.system_nr_open"
                type="number"
                min="65536"
                max="10485760"
                step="65536"
                class="obs-input w-full rounded-lg px-3 py-1.5 font-mono-num text-xs"
              />
            </div>
          </div>

          <label class="inline-flex items-center gap-1.5 text-[11px] text-slate-300 cursor-pointer select-none">
            <input
              v-model="form.persist_system_files"
              type="checkbox"
              class="accent-cyan-500 rounded cursor-pointer"
            />
            <span>
              持久化写入 <code class="text-cyan-300">/etc/sysctl.d/99-hameta-nofile.conf</code> 与
              <code class="text-cyan-300">/etc/security/limits.d/99-hameta-nofile.conf</code>（重启系统仍生效）
            </span>
          </label>
        </div>

        <p v-else class="text-[11px] text-slate-500 leading-relaxed">
          {{
            status?.system_supported
              ? "如需突破内核默认单进程上限（通常 1,048,576）或同步持久化系统参数，可勾选此项；若在只读容器内运行，系统会给出可直接复制执行的命令。"
              : "当前非 Linux 平台，系统级 /proc/sys 内核参数仅在 Linux 操作系统可用。"
          }}
        </p>
      </div>

      <!-- 表单校验错误 -->
      <div
        v-if="submitError"
        class="rounded-xl border border-rose-500/40 bg-rose-950/40 px-3.5 py-2.5 text-xs text-rose-200 flex items-start gap-2"
      >
        <ShieldAlert class="w-4 h-4 text-rose-400 shrink-0 mt-0.5" />
        <span>{{ submitError }}</span>
      </div>

      <!-- 执行结果回执 -->
      <div
        v-if="lastResult"
        class="rounded-xl border p-3.5 space-y-2.5 text-xs"
        :class="
          lastResult.ok && lastResult.warnings.length === 0
            ? 'border-emerald-500/35 bg-emerald-950/25'
            : 'border-amber-500/35 bg-amber-950/20'
        "
      >
        <div class="flex items-center gap-2 font-semibold">
          <CheckCircle2
            v-if="lastResult.ok && lastResult.warnings.length === 0"
            class="w-4 h-4 text-emerald-400 shrink-0"
          />
          <AlertTriangle v-else class="w-4 h-4 text-amber-400 shrink-0" />
          <span :class="lastResult.ok && lastResult.warnings.length === 0 ? 'text-emerald-200' : 'text-amber-200'">
            {{ lastResult.ok && lastResult.warnings.length === 0 ? "配置已全部生效" : "执行完成（存在部分提示或权限受限项）" }}
          </span>
        </div>

        <div class="space-y-1 text-[11px] font-mono-num text-slate-300">
          <div>· 程序级: {{ lastResult.process_message }}</div>
          <div>· 系统级: {{ lastResult.system_message }}</div>
          <div v-if="lastResult.persisted_app_config" class="text-emerald-300">
            · 已保存到面板数据库（下次重启自动应用）
          </div>
          <div v-if="lastResult.persisted_files.length > 0" class="text-cyan-300">
            · 已写入系统配置: {{ lastResult.persisted_files.join(", ") }}
          </div>
        </div>

        <div v-if="lastResult.worker_results.length > 0" class="space-y-1 pt-1 border-t border-slate-800/70">
          <div class="text-[10px] text-slate-400">受管子进程热更新结果 (prlimit)：</div>
          <div
            v-for="wr in lastResult.worker_results"
            :key="wr.id"
            class="flex items-center justify-between gap-2 font-mono-num text-[11px]"
          >
            <span class="text-slate-200">{{ wr.id }} (#{{ wr.pid }})</span>
            <span :class="wr.ok ? 'text-emerald-300' : 'text-amber-300'">
              {{ wr.message }} ({{ fmtLimit(wr.soft_limit) }})
            </span>
          </div>
        </div>

        <div v-if="lastResult.manual_commands.length > 0" class="space-y-1.5 pt-1 border-t border-slate-800/70">
          <div class="flex items-center justify-between text-[11px] text-amber-200">
            <span>权限受限时的手动配置命令：</span>
            <button
              type="button"
              class="obs-btn-ghost px-2 py-0.5 rounded text-[10px] inline-flex items-center gap-1 cursor-pointer"
              @click="copyManualCommands"
            >
              <Copy class="w-3 h-3" />
              复制命令
            </button>
          </div>
          <pre class="rounded-lg bg-[#06080d] border border-slate-800 p-2.5 font-mono-num text-[10px] leading-relaxed text-cyan-200 overflow-x-auto obs-scroll">{{ lastResult.manual_commands.join("\n") }}</pre>
        </div>
      </div>
    </div>

    <template #footer>
      <button
        type="button"
        class="obs-btn-ghost px-3.5 py-1.5 rounded-lg text-xs cursor-pointer"
        @click="visible = false"
      >
        关闭
      </button>
      <button
        type="button"
        :disabled="saving || loading"
        class="obs-btn-primary px-4 py-1.5 rounded-lg text-xs cursor-pointer"
        @click="handleSubmit"
      >
        {{ saving ? "正在应用..." : "应用并保存" }}
      </button>
    </template>
  </ObsModal>
</template>
