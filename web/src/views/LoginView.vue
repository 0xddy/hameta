<script setup lang="ts">
import { reactive, ref } from "vue";
import { useRouter } from "vue-router";
import { ElMessage } from "element-plus";
import { Lock, Terminal } from "lucide-vue-next";
import { ApiError } from "@/api/client";
import { useAuthStore } from "@/stores/auth";

const router = useRouter();
const authStore = useAuthStore();

const loading = ref(false);
const errorMessage = ref<string | null>(null);
const form = reactive({
  username: "",
  password: "",
});

async function handleLogin() {
  if (!form.username.trim() || !form.password) {
    errorMessage.value = "请输入管理员用户名和密码";
    return;
  }

  loading.value = true;
  errorMessage.value = null;
  try {
    await authStore.login(form.username.trim(), form.password);
    ElMessage.success("登录成功");
    await router.replace("/");
  } catch (err) {
    if (err instanceof ApiError) {
      errorMessage.value = err.message;
    } else {
      errorMessage.value = "登录请求失败，请检查网络连接";
    }
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <main class="min-h-screen flex items-center justify-center p-6 bg-slate-900">
    <div class="w-full max-w-md rounded-2xl border border-slate-200 bg-white text-slate-900 p-8 shadow-2xl">
      <div class="flex items-center gap-3 mb-6">
        <div class="h-10 w-10 rounded-xl bg-indigo-50 text-indigo-600 flex items-center justify-center">
          <Lock class="h-5 w-5" />
        </div>
        <div>
          <h1 class="text-xl font-semibold text-slate-900">hameta 运维面板登录</h1>
          <p class="text-xs text-slate-500">纯管理后台，所有操作均经会话鉴权保护</p>
        </div>
      </div>

      <el-alert
        v-if="errorMessage"
        type="error"
        :title="errorMessage"
        show-icon
        :closable="false"
        class="mb-4!"
      />

      <el-form label-position="top" @submit.prevent="handleLogin">
        <el-form-item label="管理员用户名">
          <el-input
            v-model="form.username"
            size="large"
            placeholder="请输入用户名"
            autocomplete="username"
          />
        </el-form-item>

        <el-form-item label="登录密码">
          <el-input
            v-model="form.password"
            size="large"
            type="password"
            show-password
            placeholder="请输入密码"
            autocomplete="current-password"
            @keyup.enter="handleLogin"
          />
        </el-form-item>

        <el-button
          type="primary"
          size="large"
          class="w-full mt-2"
          :loading="loading"
          native-type="submit"
        >
          登录运维控制台
        </el-button>
      </el-form>

      <div class="mt-6 rounded-lg border border-slate-200 bg-slate-50 p-3 text-xs text-slate-600 flex gap-2">
        <Terminal class="h-4 w-4 shrink-0 text-slate-500 mt-0.5" />
        <div>
          忘记账密时，可在宿主机直接执行重置命令：
          <code class="block mt-1 font-mono text-slate-800">
            hameta admin reset --username admin --password &lt;新密码&gt;
          </code>
        </div>
      </div>
    </div>
  </main>
</template>
