import { createRouter, createWebHistory } from "vue-router";
import AppLayout from "@/layouts/AppLayout.vue";
import { useAuthStore } from "@/stores/auth";
import ConsoleView from "@/views/ConsoleView.vue";
import FilterWorkbenchView from "@/views/FilterWorkbenchView.vue";
import GlobeView from "@/views/GlobeView.vue";
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
      meta: { public: true, title: "初始化向导", sub: "FIRST-BOOT PROVISIONING" },
    },
    {
      path: "/login",
      name: "login",
      component: LoginView,
      meta: { public: true, title: "管理员登录", sub: "ADMIN AUTHENTICATION" },
    },
    {
      path: "/",
      component: AppLayout,
      children: [
        {
          path: "",
          name: "overview",
          component: OverviewView,
          meta: { title: "总览态势", sub: "SINGLE-HOST MULTI-PROCESS TELEMETRY" },
        },
        {
          path: "console",
          name: "console",
          component: ConsoleView,
          meta: {
            title: "实时 Console",
            sub: "SSE REALTIME INGEST STREAM · /api/v1/console/ingest-stream",
          },
        },
        {
          path: "globe",
          name: "globe",
          component: GlobeView,
          meta: { title: "全球 DHT 流量态势", sub: "3D GLOBAL INBOUND DHT UDP TRAFFIC RADAR" },
        },
        {
          path: "workers",
          name: "workers",
          component: WorkersView,
          meta: { title: "进程管理", sub: "LOCAL PROCESS GUARD · WORKER LIFECYCLE" },
        },
        {
          path: "diagnostics",
          name: "diagnostics",
          component: WorkerDiagnosticsView,
          meta: { title: "进程诊断", sub: "REALTIME WORKER DIAGNOSTICS" },
        },
        {
          path: "workers/:id/diagnostics",
          name: "worker-diagnostics",
          component: WorkerDiagnosticsView,
          meta: { title: "进程诊断", sub: "REALTIME WORKER DIAGNOSTICS" },
        },
        {
          path: "torrents",
          name: "torrents",
          component: TorrentsView,
          meta: { title: "磁力库", sub: "MANTICORE FULL-TEXT INDEX" },
        },
        {
          path: "filter",
          name: "filter",
          component: FilterWorkbenchView,
          meta: { title: "过滤清洗", sub: "3-STAGE ARCSWAP WORKBENCH" },
        },
        {
          path: "redis",
          name: "redis",
          component: RedisMonitorView,
          meta: { title: "Redis 队列", sub: "HAMETA STREAMS & LOCK INSPECTOR" },
        },
        {
          path: "sql",
          name: "sql",
          component: SqlConsoleView,
          meta: { title: "SQL 控制台", sub: "READ-ONLY MANTICORE CONSOLE" },
        },
        {
          path: "settings/services",
          name: "settings-services",
          component: ServiceSettingsView,
          meta: { title: "服务配置", sub: "REDIS & MANTICORE ENDPOINTS" },
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
