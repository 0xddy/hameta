<script setup lang="ts">
import { type Component, reactive, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { ElMessage, type FormInstance } from "element-plus";
import {
  Activity,
  Cpu,
  Database,
  Filter,
  KeyRound,
  LogOut,
  Search,
  ServerCog,
  TerminalSquare,
  UserCheck,
} from "lucide-vue-next";
import { changePassword } from "@/api/auth";
import { ApiError } from "@/api/client";
import { useAuthStore } from "@/stores/auth";

const route = useRoute();
const router = useRouter();
const authStore = useAuthStore();

const pwdDialogVisible = ref(false);
const pwdSubmitting = ref(false);
const pwdFormRef = ref<FormInstance>();
const pwdForm = reactive({
  current_password: "",
  new_password: "",
  confirm_password: "",
});

const navItems: Array<{
  path: string;
  label: string;
  icon: Component;
  ready: boolean;
  tag?: string;
}> = [
  { path: "/", label: "系统总览", icon: Activity, ready: true },
  { path: "/settings/services", label: "服务配置中心", icon: ServerCog, ready: true },
  { path: "/workers", label: "进程与采集调优", icon: Cpu, ready: true },
  { path: "/torrents", label: "磁力检索浏览", icon: Search, ready: true },
  { path: "/filter", label: "过滤清洗工作台", icon: Filter, ready: true },
  { path: "/redis", label: "Redis Streams", icon: Database, ready: true },
  { path: "/sql", label: "只读 SQL 控制台", icon: TerminalSquare, ready: true },
];

async function handleLogout() {
  await authStore.logout();
  ElMessage.success("已安全退出登录");
  await router.replace("/login");
}

function openPwdDialog() {
  pwdForm.current_password = "";
  pwdForm.new_password = "";
  pwdForm.confirm_password = "";
  pwdDialogVisible.value = true;
}

async function submitPasswordChange() {
  if (!pwdForm.current_password) {
    ElMessage.warning("请输入当前原密码");
    return;
  }
  if (pwdForm.new_password.length < 6) {
    ElMessage.warning("新密码至少需要 6 位");
    return;
  }
  if (pwdForm.new_password !== pwdForm.confirm_password) {
    ElMessage.warning("两次输入的新密码不一致");
    return;
  }

  pwdSubmitting.value = true;
  try {
    const res = await changePassword(pwdForm.current_password, pwdForm.new_password);
    ElMessage.success(res.message);
    pwdDialogVisible.value = false;
  } catch (err) {
    const msg = err instanceof ApiError ? err.message : "修改密码失败";
    ElMessage.error(msg);
  } finally {
    pwdSubmitting.value = false;
  }
}
</script>

<template>
  <div class="min-h-screen flex bg-slate-50 text-slate-900">
    <!-- 左侧导航栏 -->
    <aside class="w-60 shrink-0 bg-slate-900 text-slate-200 flex flex-col border-r border-slate-800">
      <div class="h-14 px-4 flex items-center gap-2.5 border-b border-slate-800">
        <div class="h-8 w-8 rounded-lg bg-indigo-500 text-white flex items-center justify-center font-bold text-sm">
          H
        </div>
        <div>
          <div class="font-semibold text-sm text-white tracking-wide">hameta</div>
          <div class="text-[11px] text-slate-400">DHT-Hash 运维面板</div>
        </div>
      </div>

      <nav class="flex-1 p-3 space-y-1">
        <template v-for="item in navItems" :key="item.path">
          <RouterLink
            v-if="item.ready"
            :to="item.path"
            class="flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm transition"
            :class="
              route.path === item.path
                ? 'bg-indigo-600 text-white font-medium'
                : 'text-slate-300 hover:bg-slate-800 hover:text-white'
            "
          >
            <component :is="item.icon" class="h-4 w-4 shrink-0" />
            <span>{{ item.label }}</span>
          </RouterLink>

          <div
            v-else
            class="flex items-center justify-between px-3 py-2 rounded-lg text-sm text-slate-500 cursor-not-allowed select-none"
            title="将在后续里程碑开放"
          >
            <span class="flex items-center gap-2.5">
              <component :is="item.icon" class="h-4 w-4 shrink-0" />
              <span>{{ item.label }}</span>
            </span>
            <span class="text-[10px] px-1.5 py-0.5 rounded bg-slate-800 text-slate-400">
              {{ item.tag }}
            </span>
          </div>
        </template>
      </nav>

      <div class="p-3 border-t border-slate-800 text-xs text-slate-400">
        版本 v{{ authStore.version }}
      </div>
    </aside>

    <!-- 主内容区 -->
    <div class="flex-1 flex flex-col min-w-0">
      <header class="h-14 bg-white border-b border-slate-200 px-6 flex items-center justify-between">
        <div class="text-sm font-medium text-slate-600">
          {{ route.meta.title ?? "控制台" }}
        </div>

        <div class="flex items-center gap-3">
          <div class="flex items-center gap-1.5 text-sm text-slate-700">
            <UserCheck class="h-4 w-4 text-emerald-600" />
            <span class="font-medium">{{ authStore.user?.username ?? "管理员" }}</span>
          </div>
          <el-button size="small" @click="openPwdDialog">
            <KeyRound class="h-3.5 w-3.5 mr-1" />
            修改密码
          </el-button>
          <el-button size="small" type="danger" plain @click="handleLogout">
            <LogOut class="h-3.5 w-3.5 mr-1" />
            退出
          </el-button>
        </div>
      </header>

      <main class="flex-1 p-6 overflow-y-auto">
        <RouterView />
      </main>
    </div>

    <el-dialog v-model="pwdDialogVisible" title="修改管理员密码" width="420px">
      <el-form ref="pwdFormRef" :model="pwdForm" label-position="top">
        <el-form-item label="当前密码">
          <el-input v-model="pwdForm.current_password" type="password" show-password />
        </el-form-item>
        <el-form-item label="新密码 (至少 6 位)">
          <el-input v-model="pwdForm.new_password" type="password" show-password />
        </el-form-item>
        <el-form-item label="确认新密码">
          <el-input v-model="pwdForm.confirm_password" type="password" show-password />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="pwdDialogVisible = false">取消</el-button>
        <el-button type="primary" :loading="pwdSubmitting" @click="submitPasswordChange">
          确认更新
        </el-button>
      </template>
    </el-dialog>
  </div>
</template>
