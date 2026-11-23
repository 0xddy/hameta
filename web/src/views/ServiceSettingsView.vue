<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { ElMessage } from "element-plus";
import {
  CheckCircle2,
  Database,
  Download,
  ExternalLink,
  Globe2,
  History,
  RefreshCw,
  RotateCcw,
  Server,
  ShieldCheck,
  XCircle,
} from "lucide-vue-next";
import {
  DEFAULT_GEOIP_DOWNLOAD_URL,
  DEFAULT_GEOIP_RELEASES_URL,
  defaultGeoIpConfig,
  detectSettingsServices,
  downloadGeoIpMmdb,
  getGeoIpSettings,
  getServicesSettings,
  initManticoreTable,
  listSettingsHistory,
  reloadGeoIpSettings,
  rollbackSettings,
  testManticoreSettings,
  testRedisSettings,
  updateGeoIpSettings,
  updateManticoreSettings,
  updateRedisSettings,
  type AffectedWorkerItem,
  type CrawlerGeoIpReloadResult,
  type DownloadGeoIpResponse,
  type GeoIpConfig,
  type GeoIpSettingsResponse,
  type ServiceHistoryItem,
  type WorkerRestartOutcome,
} from "@/api/settings";
import {
  defaultManticoreConfig,
  defaultRedisConfig,
  normalizeManticoreConfig,
  type DetectedManticoreService,
  type DetectedRedisService,
  type ManticoreServiceConfig,
  type ManticoreTestReport,
  type RedisServiceConfig,
  type RedisTestReport,
  type ServiceDiscoveryReport,
} from "@/api/setup";
import ManticoreConfigFields from "@/components/ManticoreConfigFields.vue";
import RedisConfigFields from "@/components/RedisConfigFields.vue";
import ServiceDiscoveryBar from "@/components/ServiceDiscoveryBar.vue";
import ObsNumberInput from "@/components/ui/ObsNumberInput.vue";
import ObsSelect from "@/components/ui/ObsSelect.vue";
import { GLOBE_HOME_HUBS } from "@/data/globeGeo";
import { useTelemetryStore } from "@/stores/telemetry";
import { fmtBytes, fmtCount, fmtDurMs, formatDateTime } from "@/utils/format";

const route = useRoute();
const router = useRouter();
const telemetry = useTelemetryStore();

type SettingsTab = "redis" | "manticore" | "geoip" | "history";
const activeTab = ref<SettingsTab>("redis");

const loading = ref(false);
const discovery = ref<ServiceDiscoveryReport>({
  docker_available: false,
  redis: [],
  manticore: [],
});
const detecting = ref(false);

// Redis state
const redisCfg = ref<RedisServiceConfig>(defaultRedisConfig());
const redisVersion = ref(0);
const redisNote = ref("");
const redisTest = ref<RedisTestReport | null>(null);
const redisTesting = ref(false);
const redisSaving = ref(false);
const redisRestartAffected = ref(true);
const redisAffectedWorkers = ref<AffectedWorkerItem[]>([]);
const redisOutcomes = ref<WorkerRestartOutcome[]>([]);

// Manticore state
const manticoreCfg = ref<ManticoreServiceConfig>(defaultManticoreConfig());
const manticoreVersion = ref(0);
const manticoreNote = ref("");
const manticoreTest = ref<ManticoreTestReport | null>(null);
const manticoreTesting = ref(false);
const manticoreInitializing = ref(false);
const manticoreSaving = ref(false);
const manticoreRestartAffected = ref(true);
const manticoreAffectedWorkers = ref<AffectedWorkerItem[]>([]);
const manticoreOutcomes = ref<WorkerRestartOutcome[]>([]);

// GeoIP state
const geoCfg = ref<GeoIpConfig>(defaultGeoIpConfig());
const geoInfo = ref<GeoIpSettingsResponse | null>(null);
const geoSaving = ref(false);
const geoReloading = ref(false);
const geoDownloading = ref(false);
const geoDownloadInfo = ref<DownloadGeoIpResponse | null>(null);
const geoReloadResults = ref<CrawlerGeoIpReloadResult[]>([]);

// History state
const historyKind = ref<"redis" | "manticore">("redis");
const historyItems = ref<ServiceHistoryItem[]>([]);
const historyLoading = ref(false);
const rollbackBusyVer = ref<number | null>(null);
const rollbackRestart = ref(true);

const HOME_COUNTRY_OPTIONS = computed(() =>
  Object.values(GLOBE_HOME_HUBS).map((h) => ({
    value: h.code,
    label: h.name,
  })),
);

async function runDiscover() {
  detecting.value = true;
  try {
    discovery.value = await detectSettingsServices();
  } catch {
    // ignore scan error
  } finally {
    detecting.value = false;
  }
}

async function fetchServices() {
  loading.value = true;
  try {
    const res = await getServicesSettings();
    redisCfg.value = { ...defaultRedisConfig(), ...res.redis.config };
    redisVersion.value = res.redis.version;
    redisAffectedWorkers.value = res.redis.affected_running_workers;

    manticoreCfg.value = normalizeManticoreConfig(res.manticore.config);
    manticoreVersion.value = res.manticore.version;
    manticoreAffectedWorkers.value = res.manticore.affected_running_workers;
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "加载服务设置失败");
  } finally {
    loading.value = false;
  }
}

async function fetchGeoIp() {
  try {
    const res = await getGeoIpSettings();
    geoInfo.value = res;
    geoCfg.value = {
      ...defaultGeoIpConfig(),
      ...res.config,
      download_url: res.config.download_url?.trim() || DEFAULT_GEOIP_DOWNLOAD_URL,
    };
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "读取 GeoIP 配置失败");
  }
}

async function fetchHistory() {
  historyLoading.value = true;
  try {
    historyItems.value = await listSettingsHistory(historyKind.value, 25);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "加载历史版本失败");
  } finally {
    historyLoading.value = false;
  }
}

function selectDetectedRedis(item: DetectedRedisService) {
  redisCfg.value = {
    ...redisCfg.value,
    url: "",
    host: item.host,
    port: item.port,
  };
  ElMessage.info(`已填充候选 Redis ${item.host}:${item.port}`);
}

function selectDetectedManticore(item: DetectedManticoreService) {
  manticoreCfg.value = {
    ...manticoreCfg.value,
    url: item.url,
  };
  ElMessage.info(`已填充候选 Manticore ${item.url}`);
}

async function handleTestRedis() {
  redisTesting.value = true;
  try {
    redisTest.value = await testRedisSettings(redisCfg.value);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "测试 Redis 失败");
  } finally {
    redisTesting.value = false;
  }
}

async function handleSaveRedis() {
  redisSaving.value = true;
  try {
    const res = await updateRedisSettings(
      redisCfg.value,
      redisNote.value.trim() || "通过系统配置页面更新 Redis",
      redisRestartAffected.value,
    );
    redisVersion.value = res.version;
    redisOutcomes.value = res.restarted_workers;
    redisNote.value = "";
    ElMessage.success(`Redis 配置已保存为 v${res.version}`);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "保存 Redis 配置失败");
  } finally {
    redisSaving.value = false;
  }
}

async function handleTestManticore() {
  manticoreTesting.value = true;
  try {
    manticoreTest.value = await testManticoreSettings(manticoreCfg.value);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "测试 Manticore 失败");
  } finally {
    manticoreTesting.value = false;
  }
}

async function handleInitManticoreTable() {
  manticoreInitializing.value = true;
  try {
    const res = await initManticoreTable(manticoreCfg.value);
    if (res.ok) ElMessage.success(res.message);
    else ElMessage.warning(res.message);
    await handleTestManticore();
    await telemetry.refreshSnapshot().catch(() => {});
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "建表失败");
  } finally {
    manticoreInitializing.value = false;
  }
}

async function handleSaveManticore() {
  manticoreSaving.value = true;
  try {
    const res = await updateManticoreSettings(
      manticoreCfg.value,
      manticoreNote.value.trim() || "通过系统配置页面更新 Manticore",
      manticoreRestartAffected.value,
    );
    manticoreVersion.value = res.version;
    manticoreOutcomes.value = res.restarted_workers;
    manticoreNote.value = "";
    ElMessage.success(`Manticore 配置已保存为 v${res.version}`);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "保存 Manticore 配置失败");
  } finally {
    manticoreSaving.value = false;
  }
}

async function handleSaveGeoIp() {
  geoSaving.value = true;
  try {
    const res = await updateGeoIpSettings(geoCfg.value, true);
    geoCfg.value = { ...res.config };
    geoReloadResults.value = res.reload_results;
    if (geoInfo.value) {
      geoInfo.value = {
        ...geoInfo.value,
        ...res.inspect,
        config: res.config,
      };
    }
    const okCnt = res.reload_results.filter((r) => r.ok).length;
    ElMessage.success(
      `GeoIP 配置已保存` + (res.reload_results.length > 0 ? `，${okCnt}/${res.reload_results.length} 个爬虫热重载完成` : ""),
    );
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "保存 GeoIP 配置失败");
  } finally {
    geoSaving.value = false;
  }
}

async function handleReloadGeoIpOnly() {
  geoReloading.value = true;
  try {
    const res = await reloadGeoIpSettings();
    geoReloadResults.value = res.reload_results;
    if (geoInfo.value) {
      geoInfo.value = {
        ...geoInfo.value,
        ...res.inspect,
      };
    }
    await telemetry.refreshSnapshot().catch(() => {});
    ElMessage.success("已触发所有运行中爬虫热重载 MMDB");
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "热重载失败");
  } finally {
    geoReloading.value = false;
  }
}

async function handleDownloadGeoIp() {
  geoDownloading.value = true;
  try {
    const targetPath = geoCfg.value.mmdb_path.trim() || "GeoLite2-Country.mmdb";
    const downloadUrl = geoCfg.value.download_url.trim() || DEFAULT_GEOIP_DOWNLOAD_URL;
    const res = await downloadGeoIpMmdb({
      mmdb_path: targetPath,
      download_url: downloadUrl,
      reload_crawlers: true,
    });
    geoCfg.value = {
      ...defaultGeoIpConfig(),
      ...res.config,
      download_url: res.config.download_url?.trim() || DEFAULT_GEOIP_DOWNLOAD_URL,
    };
    geoReloadResults.value = res.reload_results;
    geoDownloadInfo.value = res;
    if (geoInfo.value) {
      geoInfo.value = {
        ...geoInfo.value,
        ...res.inspect,
        config: geoCfg.value,
      };
    } else {
      await fetchGeoIp();
    }
    await telemetry.refreshSnapshot().catch(() => {});
    ElMessage.success(res.message);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "下载或更新 GeoIP 数据库失败");
  } finally {
    geoDownloading.value = false;
  }
}

async function handleRollback(ver: number) {
  rollbackBusyVer.value = ver;
  try {
    await rollbackSettings(historyKind.value, ver, rollbackRestart.value);
    ElMessage.success(`已将 ${historyKind.value} 回滚并生成新激活版本`);
    await Promise.all([fetchServices(), fetchHistory()]);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "回滚失败");
  } finally {
    rollbackBusyVer.value = null;
  }
}

function applyTabQuery() {
  const t = route.query.tab;
  if (t === "redis" || t === "manticore" || t === "geoip" || t === "history") {
    activeTab.value = t;
    void router.replace({ path: "/settings/services", query: {} });
  }
}

watch(activeTab, (tab) => {
  if (tab === "manticore" && !loading.value && manticoreTest.value === null) {
    void handleTestManticore();
  }
  if (tab === "geoip") void fetchGeoIp();
  if (tab === "history") void fetchHistory();
});

watch(historyKind, () => {
  if (activeTab.value === "history") void fetchHistory();
});

watch(
  () => route.query.tab,
  () => {
    applyTabQuery();
  },
);

onMounted(async () => {
  applyTabQuery();
  await Promise.all([fetchServices(), fetchGeoIp(), runDiscover()]);
  if (activeTab.value === "manticore" && manticoreTest.value === null) {
    void handleTestManticore();
  }
  if (activeTab.value === "history") void fetchHistory();
});
</script>

<template>
  <section class="obs-card rounded-xl overflow-hidden">
    <!-- Unified Rounded Pill Tab Bar + Auto-Detect Trigger -->
    <div class="px-4 py-2.5 border-b border-slate-800/80 bg-[#0b0f17] flex flex-wrap items-center justify-between gap-2 text-xs">
      <div class="flex flex-wrap items-center gap-1.5">
        <button
          type="button"
          class="px-3 py-1.5 rounded-lg font-medium inline-flex items-center gap-1.5 border transition cursor-pointer"
          :class="activeTab === 'redis' ? 'obs-tab-active' : 'border-transparent text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'"
          @click="activeTab = 'redis'"
        >
          <Server class="w-3.5 h-3.5" />
          Redis 连接配置 (v{{ redisVersion }})
        </button>
        <button
          type="button"
          class="px-3 py-1.5 rounded-lg font-medium inline-flex items-center gap-1.5 border transition cursor-pointer"
          :class="activeTab === 'manticore' ? 'obs-tab-active' : 'border-transparent text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'"
          @click="activeTab = 'manticore'"
        >
          <Database class="w-3.5 h-3.5" />
          Manticore Search (v{{ manticoreVersion }})
        </button>
        <button
          type="button"
          class="px-3 py-1.5 rounded-lg font-medium inline-flex items-center gap-1.5 border transition cursor-pointer"
          :class="activeTab === 'geoip' ? 'obs-tab-active' : 'border-transparent text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'"
          @click="activeTab = 'geoip'"
        >
          <Globe2 class="w-3.5 h-3.5" />
          GeoIP 离线库 (MMDB)
        </button>
        <button
          type="button"
          class="px-3 py-1.5 rounded-lg font-medium inline-flex items-center gap-1.5 border transition cursor-pointer"
          :class="activeTab === 'history' ? 'obs-tab-active' : 'border-transparent text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'"
          @click="activeTab = 'history'"
        >
          <History class="w-3.5 h-3.5" />
          历史版本审计
        </button>
      </div>

      <div class="flex items-center gap-2">
        <button
          type="button"
          class="obs-btn obs-btn-ghost px-2.5! py-1! text-xs! inline-flex items-center gap-1"
          :disabled="detecting"
          @click="runDiscover"
        >
          <span>📡</span>
          {{ detecting ? "探测中…" : "自动检测候选服务" }}
        </button>
        <button
          type="button"
          class="obs-btn obs-btn-ghost px-2.5! py-1! text-xs! inline-flex items-center gap-1"
          @click="fetchServices(); fetchGeoIp()"
        >
          <RefreshCw class="w-3.5 h-3.5" :class="loading ? 'animate-spin text-emerald-400' : ''" />
          刷新配置
        </button>
      </div>
    </div>

    <div class="p-4 md:p-5 space-y-4">
      <!-- TAB 1: REDIS -->
      <section v-if="activeTab === 'redis'" class="space-y-4">
        <ServiceDiscoveryBar
          kind="redis"
          :loading="detecting"
          :docker-available="discovery.docker_available"
          :redis-items="discovery.redis"
          :active-key="`${redisCfg.host}:${redisCfg.port}`"
          @rescan="runDiscover"
          @select-redis="selectDetectedRedis"
        />

        <div class="obs-inset rounded-xl p-5 space-y-4">
          <RedisConfigFields v-model="redisCfg" :test-report="redisTest" :lock-indexed-prefix="true" />

          <div class="pt-3 border-t border-slate-800 flex flex-wrap items-center justify-between gap-3 text-xs">
            <div class="flex flex-wrap items-center gap-3 flex-1 min-w-[240px]">
              <input v-model="redisNote" type="text" placeholder="配置变更备注（可选）" class="obs-input rounded-lg px-3 py-1.5 text-xs max-w-xs" />
              <label v-if="redisAffectedWorkers.length > 0" class="inline-flex items-center gap-1.5 text-slate-300 cursor-pointer">
                <input v-model="redisRestartAffected" type="checkbox" class="obs-checkbox" />
                保存后自动重启 {{ redisAffectedWorkers.length }} 个受影响运行中子进程
              </label>
            </div>

            <div class="flex items-center gap-2">
              <button type="button" class="obs-btn obs-btn-ghost px-3.5! py-1.5! text-xs!" :disabled="redisTesting" @click="handleTestRedis">
                {{ redisTesting ? "测试中…" : "测试 Redis 连通性" }}
              </button>
              <button type="button" class="obs-btn obs-btn-primary px-4! py-1.5! text-xs!" :disabled="redisSaving" @click="handleSaveRedis">
                {{ redisSaving ? "保存中…" : "保存 Redis 配置" }}
              </button>
            </div>
          </div>

          <div v-if="redisOutcomes.length > 0" class="bg-[#0b0f17] border border-slate-800 p-3 rounded-lg space-y-1 text-[11px] font-mono">
            <div v-for="o in redisOutcomes" :key="o.id" :class="o.ok ? 'text-emerald-300' : 'text-rose-300'">
              [{{ o.id }}] {{ o.message }}
            </div>
          </div>
        </div>
      </section>

      <!-- TAB 2: MANTICORE -->
      <section v-else-if="activeTab === 'manticore'" class="space-y-4">
        <ServiceDiscoveryBar
          kind="manticore"
          :loading="detecting"
          :docker-available="discovery.docker_available"
          :manticore-items="discovery.manticore"
          :active-key="manticoreCfg.url"
          @rescan="runDiscover"
          @select-manticore="selectDetectedManticore"
        />

        <div class="obs-inset rounded-xl p-5 space-y-4">
          <ManticoreConfigFields v-model="manticoreCfg" :test-report="manticoreTest" />

          <div class="pt-3 border-t border-slate-800 flex flex-wrap items-center justify-between gap-3 text-xs">
            <div class="flex flex-wrap items-center gap-3 flex-1 min-w-[240px]">
              <input v-model="manticoreNote" type="text" placeholder="配置变更备注（可选）" class="obs-input rounded-lg px-3 py-1.5 text-xs max-w-xs" />
              <label v-if="manticoreAffectedWorkers.length > 0" class="inline-flex items-center gap-1.5 text-slate-300 cursor-pointer">
                <input v-model="manticoreRestartAffected" type="checkbox" class="obs-checkbox" />
                保存后自动重启 {{ manticoreAffectedWorkers.length }} 个受影响运行中子进程
              </label>
            </div>

            <div class="flex flex-wrap items-center gap-2">
              <button type="button" class="obs-btn obs-btn-ghost px-3.5! py-1.5! text-xs!" :disabled="manticoreTesting" @click="handleTestManticore">
                {{ manticoreTesting ? "检测中…" : "测试连接 & 校验表结构" }}
              </button>
              <button
                v-if="manticoreTest?.schema && !manticoreTest.schema.table_exists"
                type="button"
                class="obs-btn px-3.5! py-1.5! text-xs! rounded-lg border-amber-500/45! bg-amber-500/15 text-amber-200 hover:bg-amber-500/25"
                :disabled="manticoreInitializing"
                @click="handleInitManticoreTable"
              >
                {{ manticoreInitializing ? "建表中…" : "自动创建 torrents 表" }}
              </button>
              <button type="button" class="obs-btn obs-btn-primary px-4! py-1.5! text-xs!" :disabled="manticoreSaving" @click="handleSaveManticore">
                {{ manticoreSaving ? "保存中…" : "保存 Manticore 配置" }}
              </button>
            </div>
          </div>

          <div v-if="manticoreOutcomes.length > 0" class="bg-[#0b0f17] border border-slate-800 p-3 rounded-lg space-y-1 text-[11px] font-mono">
            <div v-for="o in manticoreOutcomes" :key="o.id" :class="o.ok ? 'text-emerald-300' : 'text-rose-300'">
              [{{ o.id }}] {{ o.message }}
            </div>
          </div>
        </div>
      </section>

      <!-- TAB 3: GEOIP (MMDB) -->
      <section v-else-if="activeTab === 'geoip'" class="space-y-4">
        <div v-if="geoInfo" class="obs-inset rounded-xl p-4 space-y-3 text-xs">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <div class="flex items-center gap-2">
              <ShieldCheck v-if="geoInfo.mmdb_valid" class="w-4 h-4 text-emerald-400" />
              <XCircle v-else class="w-4 h-4 text-amber-400" />
              <span class="font-semibold text-white">
                {{ geoInfo.mmdb_valid ? "GeoLite2 MMDB 离线库校验有效" : "MMDB 离线库未就绪或校验失败" }}
              </span>
            </div>

            <div class="flex flex-wrap items-center gap-2.5">
              <span class="font-mono text-[11px] text-slate-400">
                在线爬虫 {{ geoInfo.running_crawlers.length }} 个
              </span>
              <button
                v-if="!geoInfo.mmdb_exists || !geoInfo.mmdb_valid"
                type="button"
                class="obs-btn obs-btn-primary px-3! py-1.5! text-xs! inline-flex items-center gap-1.5"
                :disabled="geoDownloading || geoSaving || geoReloading"
                @click="handleDownloadGeoIp"
              >
                <Download class="w-3.5 h-3.5" :class="geoDownloading ? 'animate-bounce' : ''" />
                {{ geoDownloading ? "下载中…" : "下载数据库" }}
              </button>
              <button
                v-else
                type="button"
                class="obs-btn obs-btn-ghost px-3! py-1.5! text-xs! border-emerald-500/35! text-emerald-300! inline-flex items-center gap-1.5"
                :disabled="geoDownloading || geoSaving || geoReloading"
                @click="handleDownloadGeoIp"
              >
                <RefreshCw class="w-3.5 h-3.5 text-emerald-400" :class="geoDownloading ? 'animate-spin' : ''" />
                {{ geoDownloading ? "更新中…" : "更新数据库" }}
              </button>
            </div>
          </div>

          <div class="bg-[#0b0f17] border border-slate-800 p-3 rounded-lg grid grid-cols-1 md:grid-cols-3 gap-3 font-mono text-[11px]">
            <div class="md:col-span-3">
              <span class="text-slate-500">解析绝对路径：</span>
              <span class="text-cyan-200 break-all">{{ geoInfo.resolved_path }}</span>
            </div>
            <div>
              <span class="text-slate-500 block">库类型 / 前缀树条目数</span>
              <span class="text-emerald-300 font-mono-num">{{ geoInfo.database_type ?? "—" }} · {{ fmtCount(geoInfo.node_count) }}</span>
            </div>
            <div>
              <span class="text-slate-500 block">文件体积</span>
              <span class="text-sky-300 font-mono-num">{{ geoInfo.mmdb_exists ? fmtBytes(geoInfo.mmdb_size_bytes) : "文件不存在" }}</span>
            </div>
            <div>
              <span class="text-slate-500 block">构建时间</span>
              <span class="text-slate-200 font-mono-num">{{ geoInfo.mmdb_build_epoch ? formatDateTime(geoInfo.mmdb_build_epoch) : "—" }}</span>
            </div>
          </div>

          <div class="flex flex-wrap items-center justify-between gap-2 text-[11px] font-mono">
            <span class="text-slate-400">{{ geoInfo.status_message }}</span>
            <a
              :href="DEFAULT_GEOIP_RELEASES_URL"
              target="_blank"
              rel="noopener noreferrer"
              class="text-cyan-400 hover:text-cyan-300 inline-flex items-center gap-1 transition-colors"
            >
              <span>P3TERX/GeoLite.mmdb Releases</span>
              <ExternalLink class="w-3 h-3" />
            </a>
          </div>

          <div
            v-if="geoDownloadInfo"
            class="px-3 py-2 rounded-lg bg-emerald-500/10 border border-emerald-500/30 text-[11px] font-mono text-emerald-200 flex flex-wrap items-center justify-between gap-2"
          >
            <span>{{ geoDownloadInfo.message }}</span>
            <span class="text-slate-300 font-mono-num">
              {{ fmtBytes(geoDownloadInfo.downloaded_bytes) }} · {{ fmtDurMs(geoDownloadInfo.duration_ms) }}<template v-if="geoDownloadInfo.release_tag"> · {{ geoDownloadInfo.release_tag }}</template>
            </span>
          </div>
        </div>

        <div class="obs-inset rounded-xl p-5 space-y-4 text-xs">
          <div class="flex flex-wrap items-center justify-between gap-3">
            <label class="inline-flex items-center gap-2 text-slate-200 cursor-pointer font-medium">
              <input v-model="geoCfg.enabled" type="checkbox" class="obs-checkbox" />
              启用 3D 全球雷达 GeoIP UDP 来源国采样
            </label>

            <button
              type="button"
              class="obs-btn obs-btn-ghost px-3! py-1.5! text-xs!"
              :disabled="geoReloading || geoDownloading || geoSaving"
              @click="handleReloadGeoIpOnly"
            >
              {{ geoReloading ? "重载中…" : "仅立即热重载 MMDB" }}
            </button>
          </div>

          <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div class="md:col-span-2">
              <div class="flex items-center justify-between gap-2 mb-1">
                <label class="text-[11px] text-slate-400">IP 数据库下载地址 (GitHub Release 直链)</label>
                <button
                  v-if="geoCfg.download_url.trim() !== DEFAULT_GEOIP_DOWNLOAD_URL"
                  type="button"
                  class="text-[11px] text-cyan-400 hover:underline cursor-pointer"
                  @click="geoCfg.download_url = DEFAULT_GEOIP_DOWNLOAD_URL"
                >
                  恢复默认源
                </button>
              </div>
              <input
                v-model="geoCfg.download_url"
                type="url"
                :placeholder="DEFAULT_GEOIP_DOWNLOAD_URL"
                class="obs-input w-full rounded-lg px-3 py-1.5 text-xs font-mono"
              />
              <p class="mt-1 text-[10px] text-slate-500">
                点击上方「下载数据库 / 更新数据库」将流式拉取最新 Release、校验 MMDB 格式、原子替换到下方目录并热重载所有运行中爬虫。
              </p>
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">MaxMind GeoLite2-Country.mmdb 文件路径</label>
              <input
                v-model="geoCfg.mmdb_path"
                type="text"
                placeholder="GeoLite2-Country.mmdb 或 /opt/geoip/GeoLite2-Country.mmdb"
                class="obs-input w-full rounded-lg px-3 py-1.5 text-xs font-mono"
              />
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">本机部署归属国家/地区 (HOME HUB)</label>
              <ObsSelect v-model="geoCfg.home_country" :options="HOME_COUNTRY_OPTIONS" class="w-full" />
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">IP /24 前缀 LRU 缓存槽位数</label>
              <ObsNumberInput v-model="geoCfg.prefix_cache_slots" :min="1024" :max="1048576" />
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">自适应降采样触发阈值 (PPS)</label>
              <ObsNumberInput v-model="geoCfg.adaptive_sample_threshold_pps" :min="1000" :step="1000" />
            </div>
            <div>
              <label class="block text-[11px] text-slate-400 mb-1">无锁环形采样队列容量</label>
              <ObsNumberInput v-model="geoCfg.sample_queue_capacity" :min="512" :step="512" />
            </div>
          </div>

          <div class="pt-3 border-t border-slate-800 flex justify-end">
            <button
              type="button"
              class="obs-btn obs-btn-primary px-4! py-1.5! text-xs!"
              :disabled="geoSaving || geoDownloading || geoReloading"
              @click="handleSaveGeoIp"
            >
              {{ geoSaving ? "保存并通知爬虫重载中…" : "保存 GeoIP 配置并热重载所有运行中爬虫" }}
            </button>
          </div>

          <div v-if="geoReloadResults.length > 0" class="bg-[#0b0f17] border border-slate-800 p-3 rounded-lg space-y-1.5 text-[11px] font-mono">
            <div
              v-for="r in geoReloadResults"
              :key="r.instance_id"
              class="flex items-center gap-2"
              :class="r.ok ? 'text-emerald-300' : 'text-rose-300'"
            >
              <CheckCircle2 v-if="r.ok" class="w-3.5 h-3.5 shrink-0" />
              <XCircle v-else class="w-3.5 h-3.5 shrink-0" />
              <span>[{{ r.instance_id }}] {{ r.message }}</span>
            </div>
          </div>
        </div>
      </section>

      <!-- TAB 4: HISTORY -->
      <section v-else class="obs-inset rounded-xl overflow-hidden">
        <div class="p-4 border-b border-slate-800 bg-[#0b0f17] flex flex-wrap items-center justify-between gap-3 text-xs">
          <div class="bg-[#06090f] border border-slate-800 p-0.5 rounded-lg inline-flex">
            <button
              type="button"
              class="px-3 py-1 rounded-md transition cursor-pointer"
              :class="historyKind === 'redis' ? 'bg-emerald-500/15 text-emerald-300 font-medium' : 'text-slate-400 hover:text-slate-200'"
              @click="historyKind = 'redis'"
            >
              Redis 配置历史
            </button>
            <button
              type="button"
              class="px-3 py-1 rounded-md transition cursor-pointer"
              :class="historyKind === 'manticore' ? 'bg-emerald-500/15 text-emerald-300 font-medium' : 'text-slate-400 hover:text-slate-200'"
              @click="historyKind = 'manticore'"
            >
              Manticore 配置历史
            </button>
          </div>

          <label class="inline-flex items-center gap-2 text-slate-300 cursor-pointer">
            <input v-model="rollbackRestart" type="checkbox" class="obs-checkbox" />
            回滚时自动重启受影响子进程
          </label>
        </div>

        <div v-if="historyLoading" class="py-12 text-center text-xs font-mono text-slate-400">
          正在加载配置版本审计历史…
        </div>

        <div v-else class="divide-y divide-slate-800/60">
          <div v-for="item in historyItems" :key="item.id" class="p-4 flex flex-wrap items-center justify-between gap-3 text-xs">
            <div class="space-y-1 min-w-0">
              <div class="flex flex-wrap items-center gap-2">
                <span class="font-mono font-bold text-emerald-300">v{{ item.version }}</span>
                <span v-if="item.is_active" class="px-2 py-0.5 rounded-md bg-emerald-500/15 text-emerald-300 border border-emerald-500/30 text-[10px] font-mono">
                  ACTIVE
                </span>
                <span
                  v-if="item.last_test_ok !== null"
                  class="px-2 py-0.5 rounded-md text-[10px] font-mono"
                  :class="item.last_test_ok ? 'bg-sky-500/10 text-sky-300' : 'bg-rose-500/10 text-rose-300'"
                >
                  {{ item.last_test_ok ? "TEST PASS" : "TEST FAIL" }}
                </span>
                <span class="text-white">{{ item.note || "无变更说明" }}</span>
              </div>
              <div class="text-[11px] font-mono text-slate-500">
                {{ item.created_by }} · {{ formatDateTime(item.created_at) }}
                <template v-if="item.last_test_message"> · {{ item.last_test_message }}</template>
              </div>
            </div>

            <button
              v-if="!item.is_active"
              type="button"
              class="obs-btn obs-btn-ghost px-3! py-1.5! text-xs! inline-flex items-center gap-1"
              :disabled="rollbackBusyVer === item.version"
              @click="handleRollback(item.version)"
            >
              <RotateCcw class="w-3.5 h-3.5" />
              {{ rollbackBusyVer === item.version ? "回滚中…" : "回滚至此版本" }}
            </button>
          </div>

          <div v-if="historyItems.length === 0" class="py-12 text-center text-xs text-slate-500">暂无配置历史</div>
        </div>
      </section>
    </div>
  </section>
</template>
