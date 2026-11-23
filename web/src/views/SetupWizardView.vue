<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { useRouter } from "vue-router";
import { ElMessage, ElMessageBox } from "element-plus";
import { Upload } from "lucide-vue-next";
import { ApiError } from "@/api/client";
import {
  defaultManticoreConfig,
  defaultRedisConfig,
  detectSetupServices,
  initSetupManticoreTable,
  normalizeManticoreConfig,
  submitSetupInit,
  testSetupManticore,
  testSetupRedis,
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
import { DHT_PROFILE_SELECT_OPTIONS } from "@/data/dhtProfiles";
import { useAuthStore } from "@/stores/auth";

const router = useRouter();
const authStore = useAuthStore();

const steps = ["1. 管理员", "2. Redis", "3. Manticore", "4. 初始配置"];
const step = ref(0);

const detecting = ref(false);
const discovery = ref<ServiceDiscoveryReport | null>(null);
const redisTouched = ref(false);
const manticoreTouched = ref(false);

const testingRedis = ref(false);
const redisReport = ref<RedisTestReport | null>(null);

const testingManticore = ref(false);
const manticoreReport = ref<ManticoreTestReport | null>(null);
const creatingTable = ref(false);
const submitting = ref(false);

const form = reactive({
  admin_username: "admin",
  admin_password: "",
  confirm_password: "",
  redis: defaultRedisConfig() as RedisServiceConfig,
  manticore: defaultManticoreConfig() as ManticoreServiceConfig,
  filterMode: "preset" as "preset" | "legacy",
  legacy_filter_json: "",
  create_default_workers: true,
  crawler_port: 12313,
  crawler_profile: "P5",
});

const activeRedisKey = computed(() =>
  form.redis.url.trim() ? "" : `${form.redis.host}:${form.redis.port}`,
);
const activeManticoreKey = computed(() => form.manticore.url.trim().replace(/\/+$/, ""));

async function runDetection(autoFill: boolean) {
  detecting.value = true;
  try {
    const res = await detectSetupServices();
    discovery.value = res;
    if (autoFill) {
      if (!redisTouched.value) {
        const bestRedis = res.redis.find((r) => r.reachable);
        if (bestRedis) {
          form.redis = { ...form.redis, url: "", host: bestRedis.host, port: bestRedis.port };
        }
      }
      if (!manticoreTouched.value) {
        const bestManticore = res.manticore.find((m) => m.reachable);
        if (bestManticore) {
          form.manticore = { ...form.manticore, url: bestManticore.url };
        }
      }
    }
  } catch {
    // 忽略检测异常，允许手动填写
  } finally {
    detecting.value = false;
  }
}

function applyDetectedRedis(item: DetectedRedisService) {
  redisTouched.value = true;
  form.redis = {
    ...form.redis,
    url: "",
    host: item.host,
    port: item.port,
  };
  redisReport.value = null;
}

function applyDetectedManticore(item: DetectedManticoreService) {
  manticoreTouched.value = true;
  form.manticore = {
    ...form.manticore,
    url: item.url,
  };
  manticoreReport.value = null;
}

function handleRedisChange(next: RedisServiceConfig) {
  redisTouched.value = true;
  form.redis = next;
}

function handleManticoreChange(next: ManticoreServiceConfig) {
  manticoreTouched.value = true;
  form.manticore = next;
}

function validateStep0(): boolean {
  if (form.admin_username.trim().length < 2) {
    ElMessage.warning("用户名至少 2 个字符");
    return false;
  }
  if (form.admin_password.length < 6) {
    ElMessage.warning("密码至少 6 位");
    return false;
  }
  if (form.admin_password !== form.confirm_password) {
    ElMessage.warning("两次密码不一致");
    return false;
  }
  return true;
}

function jumpStep(target: number) {
  if (target === step.value) return;
  if (target > 0 && step.value === 0 && !validateStep0()) return;
  step.value = target;
}

function nextStep() {
  if (step.value === 0 && !validateStep0()) return;
  if (step.value < 3) step.value += 1;
}

async function handleTestRedis() {
  testingRedis.value = true;
  try {
    redisReport.value = await testSetupRedis(form.redis);
    if (redisReport.value.ok) {
      ElMessage.success("Redis 连接正常");
    } else {
      ElMessage.error(redisReport.value.message);
    }
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "测试失败");
  } finally {
    testingRedis.value = false;
  }
}

async function handleTestManticore() {
  testingManticore.value = true;
  try {
    manticoreReport.value = await testSetupManticore(normalizeManticoreConfig(form.manticore));
    if (manticoreReport.value.ok) {
      ElMessage.success("Manticore 连接与 Schema 校验正常");
    } else {
      ElMessage.warning(manticoreReport.value.message);
    }
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "测试失败");
  } finally {
    testingManticore.value = false;
  }
}

async function handleCreateManticoreTable() {
  try {
    await ElMessageBox.confirm("确认在当前空实例创建 torrents 表？", "初始化建表", {
      type: "warning",
      confirmButtonText: "确认创建",
      cancelButtonText: "取消",
    });
  } catch {
    return;
  }

  creatingTable.value = true;
  try {
    await initSetupManticoreTable(normalizeManticoreConfig(form.manticore));
    ElMessage.success("已创建 torrents 表");
    await Promise.all([handleTestManticore(), runDetection(false)]);
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "建表失败");
  } finally {
    creatingTable.value = false;
  }
}

function handleLegacyFileUpload(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;
  const reader = new FileReader();
  reader.onload = () => {
    form.legacy_filter_json = String(reader.result ?? "");
    ElMessage.success(`已读取 ${file.name}`);
  };
  reader.readAsText(file);
}

async function handleSubmitInit() {
  if (!validateStep0()) {
    step.value = 0;
    return;
  }
  if (form.filterMode === "legacy" && !form.legacy_filter_json.trim()) {
    ElMessage.warning("请上传或粘贴规则 JSON");
    return;
  }

  submitting.value = true;
  try {
    await submitSetupInit({
      admin_username: form.admin_username.trim(),
      admin_password: form.admin_password,
      redis: form.redis,
      manticore: normalizeManticoreConfig(form.manticore),
      legacy_filter_json: form.filterMode === "legacy" ? form.legacy_filter_json : null,
      create_default_workers: form.create_default_workers,
      crawler_port: form.crawler_port,
      crawler_profile: form.crawler_profile,
    });
    await authStore.refreshBootstrap();
    ElMessage.success("初始化完成！已进入控制台");
    await router.replace("/");
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "初始化失败");
  } finally {
    submitting.value = false;
  }
}

onMounted(() => {
  void runDetection(true);
});
</script>

<template>
  <main class="min-h-screen obs-grid-bg py-8 px-4 flex flex-col items-center justify-center">
    <div class="w-full max-w-3xl space-y-5">
      <!-- Brand + 4 Steps -->
      <div class="flex flex-wrap items-center justify-between gap-4 px-1">
        <div class="flex items-center gap-2.5">
          <div class="h-8 w-8 rounded-lg bg-gradient-to-br from-emerald-400 to-cyan-500 p-[1px]">
            <div class="h-full w-full rounded-[7px] bg-[#0b0e15] flex items-center justify-center font-mono-num font-bold text-sm text-emerald-400">
              H
            </div>
          </div>
          <div>
            <div class="text-sm font-semibold text-slate-100 leading-none tracking-wider">
              HAMETA 初始化向导
            </div>
            <div class="text-[11px] font-mono-num text-slate-500 mt-1">FIRST-BOOT PROVISIONING</div>
          </div>
        </div>

        <nav class="flex flex-wrap items-center gap-1.5 text-xs">
          <button
            v-for="(label, idx) in steps"
            :key="label"
            type="button"
            class="px-3 py-1.5 rounded-full font-medium cursor-pointer transition"
            :class="
              idx === step
                ? 'bg-emerald-500 text-[#04130d] font-semibold'
                : idx < step
                  ? 'bg-emerald-500/15 text-emerald-300 border border-emerald-500/30'
                  : 'text-slate-400 hover:text-slate-200'
            "
            @click="jumpStep(idx)"
          >
            {{ label }}
          </button>
        </nav>
      </div>

      <div class="obs-card rounded-2xl p-6 sm:p-7 space-y-6">
        <!-- Step 0: Admin -->
        <div v-show="step === 0" class="max-w-sm mx-auto py-2 space-y-4">
          <div class="text-center space-y-1">
            <h2 class="text-base font-semibold text-slate-100">创建管理员账号</h2>
            <p class="text-xs text-slate-400">凭证将使用 Argon2id 哈希写入本地 SQLite 状态库</p>
          </div>
          <div class="space-y-3 text-xs">
            <div>
              <label class="text-slate-400 block mb-1.5">用户名</label>
              <input
                v-model="form.admin_username"
                type="text"
                placeholder="admin"
                class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
              />
            </div>
            <div>
              <label class="text-slate-400 block mb-1.5">密码</label>
              <input
                v-model="form.admin_password"
                type="password"
                placeholder="至少 6 位"
                class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
              />
            </div>
            <div>
              <label class="text-slate-400 block mb-1.5">确认密码</label>
              <input
                v-model="form.confirm_password"
                type="password"
                placeholder="再次输入密码"
                class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
                @keyup.enter="nextStep"
              />
            </div>
          </div>
        </div>

        <!-- Step 1: Redis -->
        <div v-show="step === 1" class="space-y-4">
          <div class="flex items-center justify-between">
            <h2 class="text-sm font-semibold text-slate-100">连接 Redis (Streams 与去重状态)</h2>
            <button
              type="button"
              :disabled="testingRedis"
              class="obs-btn-ghost px-3 py-1.5 rounded-lg text-xs cursor-pointer"
              @click="handleTestRedis"
            >
              ⚡ {{ testingRedis ? "测试中..." : "测试连接" }}
            </button>
          </div>

          <ServiceDiscoveryBar
            kind="redis"
            :loading="detecting"
            :docker-available="discovery?.docker_available ?? false"
            :redis-items="discovery?.redis ?? []"
            :active-key="activeRedisKey"
            @rescan="runDetection(false)"
            @select-redis="applyDetectedRedis"
          />

          <RedisConfigFields
            :model-value="form.redis"
            :test-report="redisReport"
            compact
            @update:model-value="handleRedisChange"
          />
        </div>

        <!-- Step 2: Manticore -->
        <div v-show="step === 2" class="space-y-4">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="text-sm font-semibold text-slate-100">
              连接 ManticoreSearch (torrents 存量兼容性校验)
            </h2>
            <div class="flex gap-2">
              <button
                v-if="manticoreReport?.schema && !manticoreReport.schema.table_exists"
                type="button"
                :disabled="creatingTable"
                class="px-3 py-1.5 rounded-lg text-xs border border-amber-500/40 bg-amber-500/15 text-amber-200 hover:bg-amber-500/25 cursor-pointer"
                @click="handleCreateManticoreTable"
              >
                {{ creatingTable ? "建表中..." : "初始化 torrents 表" }}
              </button>
              <button
                type="button"
                :disabled="testingManticore"
                class="obs-btn-ghost px-3 py-1.5 rounded-lg text-xs cursor-pointer"
                @click="handleTestManticore"
              >
                ⚡ {{ testingManticore ? "校验中..." : "测试并校验 Schema" }}
              </button>
            </div>
          </div>

          <ServiceDiscoveryBar
            kind="manticore"
            :loading="detecting"
            :docker-available="discovery?.docker_available ?? false"
            :manticore-items="discovery?.manticore ?? []"
            :active-key="activeManticoreKey"
            @rescan="runDetection(false)"
            @select-manticore="applyDetectedManticore"
          />

          <ManticoreConfigFields
            :model-value="form.manticore"
            :test-report="manticoreReport"
            compact
            @update:model-value="handleManticoreChange"
          />
        </div>

        <!-- Step 3: 初始配置与默认子进程 -->
        <div v-show="step === 3" class="space-y-4">
          <h2 class="text-sm font-semibold text-slate-100">过滤规则初始化与默认 Worker 子进程</h2>

          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3 text-xs">
            <button
              type="button"
              class="text-left rounded-xl border p-3.5 transition cursor-pointer"
              :class="
                form.filterMode === 'preset'
                  ? 'border-emerald-500/45 bg-emerald-500/10'
                  : 'border-slate-800 bg-[#0c1019] hover:border-slate-700'
              "
              @click="form.filterMode = 'preset'"
            >
              <div
                class="font-semibold"
                :class="form.filterMode === 'preset' ? 'text-emerald-200' : 'text-slate-200'"
              >
                ✦ 内置默认预设 (HAMETA)
              </div>
              <div class="text-slate-400 mt-1">包含 8 阶段清洗器与常见垃圾拦截规则基线</div>
            </button>

            <button
              type="button"
              class="text-left rounded-xl border p-3.5 transition cursor-pointer"
              :class="
                form.filterMode === 'legacy'
                  ? 'border-emerald-500/45 bg-emerald-500/10'
                  : 'border-slate-800 bg-[#0c1019] hover:border-slate-700'
              "
              @click="form.filterMode = 'legacy'"
            >
              <div
                class="font-semibold"
                :class="form.filterMode === 'legacy' ? 'text-emerald-200' : 'text-slate-200'"
              >
                ↑ 导入旧版规则 JSON
              </div>
              <div class="text-slate-400 mt-1">自动转译旧版 config.json 为策略链</div>
            </button>
          </div>

          <div v-if="form.filterMode === 'legacy'" class="space-y-2 text-xs">
            <label class="obs-btn-ghost inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg cursor-pointer">
              <Upload class="h-3.5 w-3.5" />
              选择 JSON 文件
              <input
                type="file"
                accept=".json,application/json"
                class="hidden"
                @change="handleLegacyFileUpload"
              />
            </label>
            <textarea
              v-model="form.legacy_filter_json"
              rows="5"
              placeholder="粘贴旧版过滤规则 JSON 内容..."
              class="obs-input w-full rounded-lg p-3 font-mono-num text-xs text-cyan-200"
            />
          </div>

          <div class="obs-inset rounded-xl p-4 space-y-3 text-xs">
            <div class="flex items-center justify-between">
              <div>
                <div class="font-medium text-slate-100">创建默认 Worker 子进程</div>
                <div class="text-slate-400 mt-0.5">
                  crawler-1 与 indexer-1（由本机进程守护托管，初始保持停止）
                </div>
              </div>
              <button
                type="button"
                class="px-2.5 py-0.5 rounded font-mono-num cursor-pointer border transition"
                :class="
                  form.create_default_workers
                    ? 'bg-emerald-500/20 text-emerald-300 border-emerald-500/35'
                    : 'bg-slate-800 text-slate-400 border-slate-700'
                "
                @click="form.create_default_workers = !form.create_default_workers"
              >
                {{ form.create_default_workers ? "已启用" : "已禁用" }}
              </button>
            </div>

            <div v-if="form.create_default_workers" class="grid grid-cols-1 sm:grid-cols-2 gap-3 pt-1">
              <div>
                <label class="text-slate-400 block mb-1.5">Crawler UDP 端口</label>
                <ObsNumberInput v-model="form.crawler_port" :min="1" :max="65535" />
              </div>
              <div>
                <label class="text-slate-400 block mb-1.5">初始采集档位</label>
                <ObsSelect
                  v-model="form.crawler_profile"
                  :options="DHT_PROFILE_SELECT_OPTIONS"
                  mono
                  class="w-full"
                />
              </div>
            </div>
          </div>
        </div>

        <div class="pt-4 border-t border-slate-800/80 flex items-center justify-between">
          <button
            type="button"
            class="obs-btn-ghost px-3.5 py-2 rounded-lg text-xs cursor-pointer"
            :style="{ visibility: step === 0 ? 'hidden' : 'visible' }"
            @click="step = Math.max(0, step - 1)"
          >
            ← 上一步
          </button>

          <button
            v-if="step < 3"
            type="button"
            class="obs-btn-primary px-4 py-2 rounded-lg text-xs cursor-pointer"
            @click="nextStep"
          >
            下一步 →
          </button>
          <button
            v-else
            type="button"
            :disabled="submitting"
            class="obs-btn-primary px-4 py-2 rounded-lg text-xs cursor-pointer"
            @click="handleSubmitInit"
          >
            {{ submitting ? "初始化中..." : "✓ 完成初始化" }}
          </button>
        </div>
      </div>
    </div>
  </main>
</template>
