<script setup lang="ts">
import { onMounted, reactive, ref } from "vue";
import { useRouter } from "vue-router";
import { ElMessage } from "element-plus";
import { ApiError, fetchHealth } from "@/api/client";
import { useAuthStore } from "@/stores/auth";
import { copyToClipboard } from "@/utils/format";

const router = useRouter();
const authStore = useAuthStore();

const loading = ref(false);
const serviceReady = ref(true);
const errorMessage = ref<string | null>(null);
const showPassword = ref(false);

const form = reactive({
  username: "",
  password: "",
});

async function handleLogin() {
  if (!form.username.trim() || !form.password) {
    errorMessage.value = "请输入用户名和密码";
    return;
  }

  loading.value = true;
  errorMessage.value = null;
  try {
    await authStore.login(form.username.trim(), form.password);
    ElMessage.success(`欢迎回来，${form.username.trim()}`);
    await router.replace("/");
  } catch (err) {
    if (err instanceof ApiError) {
      errorMessage.value = err.message;
    } else {
      errorMessage.value = "网络异常，请重试";
    }
  } finally {
    loading.value = false;
  }
}

async function copyResetCmd() {
  const ok = await copyToClipboard("hameta admin reset --username admin");
  if (ok) ElMessage.success("已复制密码重置命令");
}

onMounted(async () => {
  try {
    await fetchHealth();
    serviceReady.value = true;
  } catch {
    serviceReady.value = false;
  }
});
</script>

<template>
  <main class="min-h-screen obs-grid-bg flex items-center justify-center p-6">
    <div class="w-full max-w-[400px] space-y-6">
      <div class="flex flex-col items-center gap-3">
        <div class="relative h-12 w-12 rounded-2xl bg-gradient-to-br from-emerald-400 to-cyan-500 p-[1px] shadow-lg shadow-emerald-500/20">
          <div class="h-full w-full rounded-[15px] bg-[#0b0f18] flex items-center justify-center">
            <span class="font-mono-num font-bold text-lg text-emerald-400">H</span>
          </div>
        </div>
        <div class="text-center space-y-1">
          <div class="flex items-center justify-center gap-2">
            <h1 class="text-lg font-semibold tracking-wider text-slate-50">HAMETA</h1>
            <span class="text-[11px] font-medium px-1.5 py-0.5 rounded bg-emerald-500/10 text-emerald-300 border border-emerald-500/25">
              管理控制台
            </span>
          </div>
          <p class="text-xs text-slate-400">磁力采集 · 智能清洗 · 全文检索</p>
        </div>
      </div>

      <form class="obs-card rounded-2xl p-6 space-y-4" @submit.prevent="handleLogin">
        <div class="flex items-center justify-between pb-3 border-b border-slate-800/80 text-xs">
          <span class="text-slate-300 font-medium">管理员登录</span>
          <span
            class="inline-flex items-center gap-1.5 text-[11px] font-medium"
            :class="serviceReady ? 'text-emerald-400' : 'text-amber-400'"
          >
            <span
              class="h-1.5 w-1.5 rounded-full"
              :class="serviceReady ? 'bg-emerald-400' : 'bg-amber-400'"
            />
            {{ serviceReady ? "服务就绪" : "检测中" }}
          </span>
        </div>

        <div
          v-if="errorMessage"
          class="rounded-xl border border-rose-500/40 bg-rose-950/45 px-3 py-2 text-xs text-rose-200"
        >
          {{ errorMessage }}
        </div>

        <div class="space-y-3.5">
          <div class="space-y-1.5">
            <label class="block text-xs text-slate-400">用户名</label>
            <input
              v-model="form.username"
              type="text"
              placeholder="admin"
              autocomplete="username"
              class="obs-input w-full rounded-lg px-3 py-2.5 text-sm font-mono-num"
            />
          </div>

          <div class="space-y-1.5">
            <div class="flex items-center justify-between">
              <label class="block text-xs text-slate-400">密码</label>
              <button
                type="button"
                class="text-[11px] text-slate-500 hover:text-slate-300 cursor-pointer"
                @click="showPassword = !showPassword"
              >
                {{ showPassword ? "隐藏" : "显示" }}
              </button>
            </div>
            <input
              v-model="form.password"
              :type="showPassword ? 'text' : 'password'"
              placeholder="请输入密码"
              autocomplete="current-password"
              class="obs-input w-full rounded-lg px-3 py-2.5 text-sm font-mono-num"
            />
          </div>

          <button
            type="submit"
            :disabled="loading"
            class="obs-btn-primary w-full py-2.5 rounded-lg text-sm cursor-pointer mt-1"
          >
            {{ loading ? "正在登录..." : "登录控制台" }}
          </button>
        </div>
      </form>

      <div class="obs-card rounded-xl px-3.5 py-2.5 flex flex-wrap items-center justify-between gap-2 text-[11px] text-slate-400">
        <span>忘记密码？</span>
        <code
          class="font-mono-num text-cyan-300/90 hover:text-cyan-200 cursor-pointer transition"
          title="点击复制重置命令"
          @click="copyResetCmd"
        >
          hameta admin reset --username admin
        </code>
      </div>
    </div>
  </main>
</template>
