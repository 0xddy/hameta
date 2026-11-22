import { createRouter, createWebHistory } from "vue-router";
import AppLayout from "@/layouts/AppLayout.vue";
import { useAuthStore } from "@/stores/auth";
import FilterWorkbenchView from "@/views/FilterWorkbenchView.vue";
import LoginView from "@/views/LoginView.vue";
import OverviewView from "@/views/OverviewView.vue";
import RedisMonitorView from "@/views/RedisMonitorView.vue";
import ServiceSettingsView from "@/views/ServiceSettingsView.vue";
import SetupWizardView from "@/views/SetupWizardView.vue";
import SqlConsoleView from "@/views/SqlConsoleView.vue";
import TorrentsView from "@/views/TorrentsView.vue";
import WorkerDiagnosticsView from "@/views/WorkerDiagnosticsView.vue";
import WorkersView from "@/views/WorkersView.vue";

const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: "/setup",
      name: "setup",
      component: SetupWizardView,
      meta: { public: true, title: "系统初始化引导" },
    },
    {
      path: "/login",
      name: "login",
      component: LoginView,
      meta: { public: true, title: "管理员登录" },
    },
    {
      path: "/",
      component: AppLayout,
      children: [
        {
          path: "",
          name: "overview",
          component: OverviewView,
          meta: { title: "系统总览" },
        },
        {
          path: "settings/services",
          name: "settings-services",
          component: ServiceSettingsView,
          meta: { title: "服务配置中心 (Redis / Manticore)" },
        },
        {
          path: "workers",
          name: "workers",
          component: WorkersView,
          meta: { title: "进程与采集调优" },
        },
        {
          path: "workers/:id/diagnostics",
          name: "worker-diagnostics",
          component: WorkerDiagnosticsView,
          meta: { title: "节点深度诊断" },
        },
        {
          path: "torrents",
          name: "torrents",
          component: TorrentsView,
          meta: { title: "磁力检索浏览" },
        },
        {
          path: "filter",
          name: "filter",
          component: FilterWorkbenchView,
          meta: { title: "过滤清洗工作台" },
        },
        {
          path: "redis",
          name: "redis",
          component: RedisMonitorView,
          meta: { title: "Redis Streams 监控与排查" },
        },
        {
          path: "sql",
          name: "sql",
          component: SqlConsoleView,
          meta: { title: "只读 SQL 诊断控制台" },
        },
      ],
    },
    {
      path: "/:pathMatch(.*)*",
      redirect: "/",
    },
  ],
});

router.beforeEach(async (to) => {
  const authStore = useAuthStore();
  if (!authStore.bootstrapped) {
    try {
      await authStore.refreshBootstrap();
    } catch {
      // ignore network error until target renders
    }
  }

  if (!authStore.initialized) {
    if (to.path !== "/setup") {
      return { path: "/setup", replace: true };
    }
    return true;
  }

  if (to.path === "/setup") {
    return { path: authStore.authenticated ? "/" : "/login", replace: true };
  }

  if (!authStore.authenticated && to.path !== "/login") {
    return { path: "/login", replace: true };
  }

  if (authStore.authenticated && to.path === "/login") {
    return { path: "/", replace: true };
  }

  return true;
});

export default router;
