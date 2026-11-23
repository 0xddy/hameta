<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { ElMessage } from "element-plus";
import { ChevronDown, KeyRound, LogOut, Menu, RefreshCw, Search, X } from "lucide-vue-next";
import { changePassword } from "@/api/auth";
import { ApiError } from "@/api/client";
import ObsModal from "@/components/ui/ObsModal.vue";
import { useAuthStore } from "@/stores/auth";
import { useTelemetryStore } from "@/stores/telemetry";
import { fmtBytes, fmtCount, fmtExact } from "@/utils/format";

const route = useRoute();
const router = useRouter();
const authStore = useAuthStore();
const telemetry = useTelemetryStore();

const mobileMenuOpen = ref(false);
const pwdModalVisible = ref(false);
const pwdSubmitting = ref(false);
const pwdError = ref("");
const pwdForm = reactive({
  current_password: "",
  new_password: "",
  confirm_password: "",
});

const liveClock = ref(new Date().toTimeString().slice(0, 8));
let clockTimer: number | null = null;

const snap = computed(() => telemetry.snapshot);
const headerTitle = computed(() => String(route.meta.title ?? "总览态势"));
const headerSub = computed(() => String(route.meta.sub ?? "SINGLE-HOST MULTI-PROCESS TELEMETRY"));

const totalRssBytes = computed(() => snap.value?.kpi.total_rss_bytes ?? 0);
const activePids = computed(() => {
  const ws = snap.value?.workers ?? [];
  return ws.filter((w) => w.pid !== null && w.runtime_state === "running").length;
});
const rssPercent = computed(() => {
  if (!totalRssBytes.value) return 0;
  // 以 8 GiB 作为条形参考标尺，最小可见 4%，最大 100%
  return Math.min(100, Math.max(4, Math.round((totalRssBytes.value / (8 * 1024 * 1024 * 1024)) * 100)));
});

const consoleBadgeClass = computed(() => {
  if (telemetry.consoleBadge === "OFF") {
    return "bg-rose-500/15 text-rose-300 border-rose-500/25";
  }
  if (telemetry.consoleBadge === "PAUSE") {
    return "bg-amber-500/15 text-amber-300 border-amber-500/25";
  }
  return "bg-emerald-500/15 text-emerald-300 border-emerald-500/25";
});

function isRouteActive(path: string, matchPrefix?: string): boolean {
  if (matchPrefix) {
    return route.path === path || route.path.startsWith(`${matchPrefix}/`);
  }
  return route.path === path;
}

function isDiagnosticsActive(): boolean {
  return route.path === "/diagnostics" || /^\/workers\/[^/]+\/diagnostics/.test(route.path);
}

function goSearchTorrents() {
  mobileMenuOpen.value = false;
  void router.push({ path: "/torrents", query: { focus: String(Date.now()) } });
}

function handleGlobalKeydown(e: KeyboardEvent) {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
    e.preventDefault();
    goSearchTorrents();
  }
}

async function handleManualRefresh() {
  try {
    await telemetry.refreshSnapshot();
    ElMessage.success("已触发全链路快照刷新");
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "刷新失败");
  }
}

function handleAccountCommand(cmd: string) {
  if (cmd === "password") {
    pwdForm.current_password = "";
    pwdForm.new_password = "";
    pwdForm.confirm_password = "";
    pwdError.value = "";
    pwdModalVisible.value = true;
  } else if (cmd === "logout") {
    void handleLogout();
  }
}

async function handleLogout() {
  telemetry.disconnect(true);
  await authStore.logout();
  ElMessage.success("已退出登录");
  await router.replace("/login");
}

async function submitPasswordChange() {
  pwdError.value = "";
  if (!pwdForm.current_password) {
    pwdError.value = "请输入当前密码";
    return;
  }
  if (pwdForm.new_password.length < 6) {
    pwdError.value = "新密码至少 6 位";
    return;
  }
  if (pwdForm.new_password !== pwdForm.confirm_password) {
    pwdError.value = "两次输入的新密码不一致";
    return;
  }

  pwdSubmitting.value = true;
  try {
    const res = await changePassword(pwdForm.current_password, pwdForm.new_password);
    ElMessage.success(res.message);
    pwdModalVisible.value = false;
  } catch (err) {
    pwdError.value = err instanceof ApiError ? err.message : "修改失败";
  } finally {
    pwdSubmitting.value = false;
  }
}

onMounted(() => {
  telemetry.connect();
  clockTimer = window.setInterval(() => {
    liveClock.value = new Date().toTimeString().slice(0, 8);
  }, 1000);
  window.addEventListener("keydown", handleGlobalKeydown);
});

onUnmounted(() => {
  telemetry.disconnect();
  if (clockTimer !== null) {
    window.clearInterval(clockTimer);
    clockTimer = null;
  }
  window.removeEventListener("keydown", handleGlobalKeydown);
});
</script>

<template>
  <div class="min-h-screen flex obs-grid-bg relative">
    <!-- 移动端侧栏遮罩 -->
    <div
      v-if="mobileMenuOpen"
      class="fixed inset-0 z-30 bg-black/60 backdrop-blur-xs lg:hidden"
      @click="mobileMenuOpen = false"
    />

    <!-- 左侧常驻深空遥测导航栏 -->
    <aside
      class="fixed lg:sticky top-0 left-0 z-40 h-screen w-[232px] shrink-0 bg-[#0b0e15]/95 backdrop-blur-xl border-r border-slate-800/80 flex flex-col transition-transform duration-200 lg:translate-x-0"
      :class="mobileMenuOpen ? 'translate-x-0' : '-translate-x-full'"
    >
      <!-- 品牌标识 -->
      <div class="h-14 px-4 flex items-center justify-between border-b border-slate-800/75">
        <RouterLink to="/" class="flex items-center gap-2.5 min-w-0" @click="mobileMenuOpen = false">
          <div class="h-7 w-7 rounded-lg bg-gradient-to-br from-emerald-400/90 to-cyan-500/90 p-[1px] shrink-0">
            <div class="h-full w-full rounded-[7px] bg-[#0b0e15] flex items-center justify-center font-mono-num font-bold text-xs text-emerald-400">
              H
            </div>
          </div>
          <div class="min-w-0">
            <div class="font-semibold text-[14px] tracking-wider text-slate-100 leading-none">HAMETA</div>
            <div class="font-mono-num text-[10px] text-slate-500 mt-0.5 truncate">
              v{{ authStore.version }} · rust
            </div>
          </div>
        </RouterLink>

        <div class="flex items-center gap-2 shrink-0">
          <span
            class="h-2 w-2 rounded-full bg-emerald-400 text-emerald-400 pulse-ring"
            title="本机进程守护运行中"
          />
          <button
            type="button"
            class="lg:hidden p-1 rounded text-slate-400 hover:text-slate-100 cursor-pointer"
            @click="mobileMenuOpen = false"
          >
            <X class="h-4 w-4" />
          </button>
        </div>
      </div>

      <!-- ⌘K 快捷检索 -->
      <div class="px-3 pt-3 pb-1">
        <button
          type="button"
          class="w-full flex items-center justify-between gap-2 px-2.5 py-1.5 rounded-lg bg-slate-900/80 border border-slate-800 hover:border-slate-700 text-xs text-slate-400 transition cursor-pointer"
          @click="goSearchTorrents"
        >
          <span class="flex items-center gap-2 truncate">
            <Search class="w-3.5 h-3.5 text-slate-500 shrink-0" />
            <span class="truncate">检索磁力 / InfoHash</span>
          </span>
          <kbd class="font-mono-num text-[10px] px-1.5 py-0.5 rounded bg-slate-800 text-slate-400 border border-slate-700/80 shrink-0">
            ⌘K
          </kbd>
        </button>
      </div>

      <!-- 导航分组 -->
      <nav class="flex-1 px-2.5 py-3 space-y-4 overflow-y-auto obs-scroll">
        <!-- RUNTIME -->
        <div class="space-y-1">
          <div class="px-2.5 pb-1 text-[10px] font-mono-num uppercase tracking-widest text-slate-500">
            运行观测 · RUNTIME
          </div>

          <RouterLink
            to="/"
            class="w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-[13px] border border-transparent text-slate-400 hover:text-slate-100 hover:bg-slate-800/40 transition"
            :class="{ 'obs-nav-active': isRouteActive('/') }"
            @click="mobileMenuOpen = false"
          >
            <span class="flex items-center gap-2.5">
              <svg class="w-4 h-4 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                <path d="M22 12h-4l-3 9L9 3l-3 9H2" />
              </svg>
              总览态势
            </span>
            <span class="font-mono-num text-[10px] text-emerald-400">LIVE</span>
          </RouterLink>

          <RouterLink
            to="/console"
            class="w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-[13px] border border-transparent text-slate-400 hover:text-slate-100 hover:bg-slate-800/40 transition"
            :class="{ 'obs-nav-active': isRouteActive('/console') }"
            @click="mobileMenuOpen = false"
          >
            <span class="flex items-center gap-2.5">
              <svg class="w-4 h-4 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                <rect x="3" y="4" width="18" height="16" rx="2" />
                <polyline points="7 10 10 13 7 16" />
                <line x1="13" y1="16" x2="17" y2="16" />
              </svg>
              实时 Console
            </span>
            <span
              class="font-mono-num text-[10px] px-1.5 py-0.5 rounded border"
              :class="consoleBadgeClass"
            >
              {{ telemetry.consoleBadge }}
            </span>
          </RouterLink>

          <RouterLink
            to="/globe"
            class="w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-[13px] border border-transparent text-slate-400 hover:text-slate-100 hover:bg-slate-800/40 transition"
            :class="{ 'obs-nav-active': isRouteActive('/globe') }"
            @click="mobileMenuOpen = false"
          >
            <span class="flex items-center gap-2.5">
              <svg class="w-4 h-4 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                <circle cx="12" cy="12" r="9" />
                <path d="M3.6 9h16.8M3.6 15h16.8" />
                <path d="M12 3a14.5 14.5 0 0 0 0 18M12 3a14.5 14.5 0 0 1 0 18" />
              </svg>
              全球态势
            </span>
            <span class="font-mono-num text-[10px] px-1.5 py-0.5 rounded bg-cyan-500/15 text-cyan-300 border border-cyan-500/25">
              3D
            </span>
          </RouterLink>
        </div>

        <!-- ENGINE -->
        <div class="space-y-1">
          <div class="px-2.5 pb-1 text-[10px] font-mono-num uppercase tracking-widest text-slate-500">
            运行引擎 · ENGINE
          </div>

          <RouterLink
            to="/workers"
            class="w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-[13px] border border-transparent text-slate-400 hover:text-slate-100 hover:bg-slate-800/40 transition"
            :class="{ 'obs-nav-active': route.path === '/workers' }"
            @click="mobileMenuOpen = false"
          >
            <span class="flex items-center gap-2.5">
              <svg class="w-4 h-4 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                <rect x="4" y="4" width="16" height="16" rx="2" />
                <rect x="9" y="9" width="6" height="6" />
                <path d="M9 1v3M15 1v3M9 20v3M15 20v3M20 9h3M20 14h3M1 9h3M1 14h3" />
              </svg>
              进程管理
            </span>
            <span class="font-mono-num text-[10px] px-1.5 py-0.5 rounded bg-slate-800/90 text-slate-300">
              {{ telemetry.runningWorkerCount }}/{{ telemetry.totalWorkerCount }}
            </span>
          </RouterLink>

          <RouterLink
            to="/diagnostics"
            class="w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-[13px] border border-transparent text-slate-400 hover:text-slate-100 hover:bg-slate-800/40 transition"
            :class="{ 'obs-nav-active': isDiagnosticsActive() }"
            @click="mobileMenuOpen = false"
          >
            <span class="flex items-center gap-2.5">
              <svg class="w-4 h-4 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                <path d="M3 3v18h18" />
                <path d="m19 9-5 5-4-4-3 3" />
              </svg>
              进程诊断
            </span>
            <span class="font-mono-num text-[10px] text-cyan-400">3s</span>
          </RouterLink>

          <RouterLink
            to="/redis"
            class="w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-[13px] border border-transparent text-slate-400 hover:text-slate-100 hover:bg-slate-800/40 transition"
            :class="{ 'obs-nav-active': isRouteActive('/redis') }"
            @click="mobileMenuOpen = false"
          >
            <span class="flex items-center gap-2.5">
              <svg class="w-4 h-4 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                <ellipse cx="12" cy="5" rx="9" ry="3" />
                <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                <path d="M3 12c0 1.66 4 3 9 3s9-1.34 9-3" />
              </svg>
              Redis 队列
            </span>
            <span
              class="font-mono-num text-[10px] text-cyan-400"
              :title="snap ? fmtExact(snap.kpi.stream_new_lag, 'Lag') : undefined"
            >
              {{ snap ? fmtCount(snap.kpi.stream_new_lag) : "-" }}
            </span>
          </RouterLink>
        </div>

        <!-- PIPELINE -->
        <div class="space-y-1">
          <div class="px-2.5 pb-1 text-[10px] font-mono-num uppercase tracking-widest text-slate-500">
            数据管线 · PIPELINE
          </div>

          <RouterLink
            to="/torrents"
            class="w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-[13px] border border-transparent text-slate-400 hover:text-slate-100 hover:bg-slate-800/40 transition"
            :class="{ 'obs-nav-active': isRouteActive('/torrents') }"
            @click="mobileMenuOpen = false"
          >
            <span class="flex items-center gap-2.5">
              <svg class="w-4 h-4 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                <circle cx="11" cy="11" r="8" />
                <path d="m21 21-4.3-4.3" />
              </svg>
              磁力库
            </span>
            <span
              class="font-mono-num text-[10px] text-slate-500"
              :title="snap?.kpi.torrents_total_docs != null ? fmtExact(snap.kpi.torrents_total_docs, 'docs') : undefined"
            >
              {{ snap?.kpi.torrents_total_docs != null ? fmtCount(snap.kpi.torrents_total_docs) : "-" }}
            </span>
          </RouterLink>

          <RouterLink
            to="/filter"
            class="w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-[13px] border border-transparent text-slate-400 hover:text-slate-100 hover:bg-slate-800/40 transition"
            :class="{ 'obs-nav-active': isRouteActive('/filter') }"
            @click="mobileMenuOpen = false"
          >
            <span class="flex items-center gap-2.5">
              <svg class="w-4 h-4 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                <polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3" />
              </svg>
              过滤清洗
            </span>
            <span
              v-if="telemetry.filterHasDraft"
              class="h-1.5 w-1.5 rounded-full bg-amber-400"
              title="有未发布草稿"
            />
          </RouterLink>

          <RouterLink
            to="/sql"
            class="w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-[13px] border border-transparent text-slate-400 hover:text-slate-100 hover:bg-slate-800/40 transition"
            :class="{ 'obs-nav-active': isRouteActive('/sql') }"
            @click="mobileMenuOpen = false"
          >
            <span class="flex items-center gap-2.5">
              <svg class="w-4 h-4 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                <polyline points="4 17 10 11 4 5" />
                <line x1="12" y1="19" x2="20" y2="19" />
              </svg>
              SQL 控制台
            </span>
          </RouterLink>
        </div>

        <!-- SYSTEM -->
        <div class="space-y-1">
          <div class="px-2.5 pb-1 text-[10px] font-mono-num uppercase tracking-widest text-slate-500">
            系统设置 · SYSTEM
          </div>

          <RouterLink
            to="/settings/services"
            class="w-full flex items-center justify-between px-2.5 py-2 rounded-lg text-[13px] border border-transparent text-slate-400 hover:text-slate-100 hover:bg-slate-800/40 transition"
            :class="{ 'obs-nav-active': isRouteActive('/settings/services') }"
            @click="mobileMenuOpen = false"
          >
            <span class="flex items-center gap-2.5">
              <svg class="w-4 h-4 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                <circle cx="12" cy="12" r="3" />
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9c.26.6.86 1 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z" />
              </svg>
              服务配置
            </span>
          </RouterLink>
        </div>
      </nav>

      <!-- 本机单机多进程微遥测 -->
      <div class="p-3 border-t border-slate-800/75 space-y-2.5 bg-[#090c12]">
        <div class="flex items-center justify-between text-[11px]">
          <span class="text-slate-500 font-mono-num">NOFILE LIMIT</span>
          <span
            class="font-mono-num text-emerald-400"
            :title="snap?.kpi.system_soft_nofile != null ? fmtExact(snap.kpi.system_soft_nofile) : undefined"
          >
            {{ snap?.kpi.system_soft_nofile != null ? fmtCount(snap.kpi.system_soft_nofile) : "N/A" }}
          </span>
        </div>
        <div class="space-y-1">
          <div class="flex items-center justify-between text-[11px]">
            <span class="text-slate-500">子进程总 RSS</span>
            <span class="font-mono-num text-slate-300">
              {{ fmtBytes(totalRssBytes) }} · {{ activePids }} PIDs
            </span>
          </div>
          <div class="h-1.5 w-full rounded-full bg-slate-800 overflow-hidden">
            <div
              class="h-full rounded-full bg-gradient-to-r from-emerald-500 to-cyan-400 transition-all duration-300"
              :style="{ width: `${rssPercent}%` }"
            />
          </div>
        </div>
      </div>
    </aside>

    <!-- 主内容区域 -->
    <div class="flex-1 flex flex-col min-w-0">
      <header class="sticky top-0 z-20 h-14 px-4 sm:px-6 bg-[#0a0d14]/85 backdrop-blur-xl border-b border-slate-800/80 flex items-center justify-between gap-3">
        <div class="flex items-center gap-3 min-w-0">
          <button
            type="button"
            class="lg:hidden p-1.5 rounded-lg text-slate-400 hover:text-white hover:bg-slate-800 cursor-pointer shrink-0"
            @click="mobileMenuOpen = true"
          >
            <Menu class="w-4 h-4" />
          </button>
          <div class="flex items-center gap-2 min-w-0">
            <h1 class="text-sm font-semibold text-slate-100 shrink-0">{{ headerTitle }}</h1>
            <span class="text-slate-700 hidden sm:inline">/</span>
            <span class="text-xs text-slate-400 font-mono-num truncate hidden sm:inline">{{ headerSub }}</span>
          </div>
        </div>

        <div class="flex items-center gap-2.5 shrink-0">
          <div class="hidden xl:flex items-center gap-3 px-3 py-1 rounded-full bg-slate-900/90 border border-slate-800 text-[11px] font-mono-num">
            <span class="flex items-center gap-1.5">
              <span
                class="h-1.5 w-1.5 rounded-full"
                :class="snap?.health.redis_ok ? 'bg-emerald-400' : snap ? 'bg-rose-500' : 'bg-slate-500'"
              />
              <span class="text-slate-400">Redis</span>
              <span :class="snap?.health.redis_ok ? 'text-emerald-300' : snap ? 'text-rose-400' : 'text-slate-400'">
                {{ snap?.health.redis_ok ? "OK" : snap ? "ERR" : "-" }}
              </span>
            </span>
            <span class="h-3 w-px bg-slate-800" />
            <span class="flex items-center gap-1.5">
              <span
                class="h-1.5 w-1.5 rounded-full"
                :class="snap?.health.manticore_ok ? 'bg-emerald-400' : snap ? 'bg-rose-500' : 'bg-slate-500'"
              />
              <span class="text-slate-400">Manticore</span>
              <span :class="snap?.health.manticore_ok ? 'text-emerald-300' : snap ? 'text-rose-400' : 'text-slate-400'">
                {{ snap?.health.manticore_ok ? "OK" : snap ? "ERR" : "-" }}
              </span>
            </span>
            <span class="h-3 w-px bg-slate-800" />
            <RouterLink
              to="/console"
              class="flex items-center gap-1.5 hover:text-cyan-300 transition"
              title="前往实时入库 Console (SSE)"
            >
              <span class="text-slate-400">SSE</span>
              <span
                class="h-1.5 w-1.5 rounded-full"
                :class="
                  telemetry.sseStatus === 'connected'
                    ? 'bg-cyan-400 animate-pulse'
                    : telemetry.sseStatus === 'reconnecting' || telemetry.sseStatus === 'connecting'
                      ? 'bg-amber-400'
                      : 'bg-rose-500'
                "
              />
              <span class="text-slate-300">{{ liveClock }}</span>
            </RouterLink>
          </div>

          <button
            type="button"
            class="obs-btn-ghost p-1.5 rounded-lg cursor-pointer"
            :class="telemetry.loading ? 'opacity-60' : ''"
            title="立即刷新"
            @click="handleManualRefresh"
          >
            <RefreshCw class="w-3.5 h-3.5" :class="{ 'animate-spin': telemetry.loading }" />
          </button>

          <el-dropdown trigger="click" @command="handleAccountCommand">
            <button
              type="button"
              class="flex items-center gap-2 pl-2 border-l border-slate-800 cursor-pointer hover:text-slate-100 transition"
            >
              <div class="h-7 w-7 rounded-lg bg-emerald-500/15 border border-emerald-500/30 text-emerald-300 flex items-center justify-center text-xs font-mono-num font-bold uppercase">
                {{ (authStore.user?.username ?? "A").slice(0, 1) }}
              </div>
              <span class="hidden sm:inline text-xs font-medium text-slate-300">
                {{ authStore.user?.username ?? "admin" }}
              </span>
              <ChevronDown class="w-3.5 h-3.5 text-slate-500" />
            </button>
            <template #dropdown>
              <el-dropdown-menu>
                <el-dropdown-item command="password">
                  <span class="inline-flex items-center gap-2">
                    <KeyRound class="h-3.5 w-3.5 text-slate-400" />
                    修改密码
                  </span>
                </el-dropdown-item>
                <el-dropdown-item divided command="logout">
                  <span class="inline-flex items-center gap-2 text-rose-400">
                    <LogOut class="h-3.5 w-3.5" />
                    退出登录
                  </span>
                </el-dropdown-item>
              </el-dropdown-menu>
            </template>
          </el-dropdown>
        </div>
      </header>

      <!-- 全局严重告警横幅 -->
      <div
        v-if="telemetry.hasCriticalAlert && snap"
        class="mx-4 sm:mx-6 mt-4 rounded-xl border border-rose-500/40 bg-rose-950/45 px-4 py-3 flex flex-wrap items-center justify-between gap-3"
      >
        <div class="flex items-center gap-2.5 text-xs text-rose-200 flex-wrap">
          <span class="h-2 w-2 rounded-full bg-rose-500 text-rose-500 pulse-ring shrink-0" />
          <span class="font-semibold text-rose-300">严重告警</span>
          <span v-if="!snap.health.redis_ok">
            Redis: {{ snap.health.redis_message || "连接异常" }}
          </span>
          <span v-if="!snap.health.manticore_ok">
            Manticore: {{ snap.health.manticore_message || "连接异常" }}
          </span>
          <span v-if="snap.health.crashed_workers > 0">
            {{ snap.health.crashed_workers }} 个受管 Worker 子进程异常退出
          </span>
        </div>
        <div class="flex items-center gap-2">
          <button
            type="button"
            class="obs-btn-ghost px-2.5 py-1 rounded-md text-xs cursor-pointer"
            @click="router.push(!snap.health.manticore_ok && snap.health.redis_ok ? '/settings/services?tab=manticore' : '/settings/services')"
          >
            服务配置
          </button>
          <button
            type="button"
            class="px-2.5 py-1 rounded-md text-xs bg-rose-500/20 text-rose-200 border border-rose-500/40 hover:bg-rose-500/30 cursor-pointer"
            @click="router.push('/workers')"
          >
            排查进程
          </button>
        </div>
      </div>

      <!-- 页面视图容器 -->
      <main class="flex-1 p-4 sm:p-6 max-w-[1480px] w-full mx-auto">
        <RouterView />
      </main>
    </div>

    <!-- 修改管理员密码弹窗 -->
    <ObsModal v-model="pwdModalVisible" title="修改管理员密码" max-width="400px">
      <form class="space-y-3.5" @submit.prevent="submitPasswordChange">
        <div
          v-if="pwdError"
          class="rounded-lg border border-rose-500/40 bg-rose-950/40 px-3 py-2 text-xs text-rose-200"
        >
          {{ pwdError }}
        </div>
        <div>
          <label class="text-slate-400 block mb-1.5">当前密码</label>
          <input
            v-model="pwdForm.current_password"
            type="password"
            autocomplete="current-password"
            class="obs-input w-full rounded-lg px-3 py-2 font-mono-num text-xs"
          />
        </div>
        <div>
          <label class="text-slate-400 block mb-1.5">新密码</label>
          <input
            v-model="pwdForm.new_password"
            type="password"
            placeholder="至少 6 位"
            autocomplete="new-password"
            class="obs-input w-full rounded-lg px-3 py-2 font-mono-num text-xs"
          />
        </div>
        <div>
          <label class="text-slate-400 block mb-1.5">确认新密码</label>
          <input
            v-model="pwdForm.confirm_password"
            type="password"
            autocomplete="new-password"
            class="obs-input w-full rounded-lg px-3 py-2 font-mono-num text-xs"
          />
        </div>
      </form>
      <template #footer>
        <button
          type="button"
          class="obs-btn-ghost px-3.5 py-1.5 rounded-lg text-xs cursor-pointer"
          @click="pwdModalVisible = false"
        >
          取消
        </button>
        <button
          type="button"
          :disabled="pwdSubmitting"
          class="obs-btn-primary px-4 py-1.5 rounded-lg text-xs cursor-pointer"
          @click="submitPasswordChange"
        >
          {{ pwdSubmitting ? "保存中..." : "确认修改" }}
        </button>
      </template>
    </ObsModal>
  </div>
</template>
