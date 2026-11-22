<script setup lang="ts">
import { reactive, ref } from "vue";
import { useRouter } from "vue-router";
import { ElMessage, ElMessageBox } from "element-plus";
import { CheckCircle2, Rocket, Upload } from "lucide-vue-next";
import { ApiError } from "@/api/client";
import {
  defaultManticoreConfig,
  defaultRedisConfig,
  initSetupManticoreTable,
  submitSetupInit,
  testSetupManticore,
  testSetupRedis,
  type ManticoreServiceConfig,
  type ManticoreTestReport,
  type RedisServiceConfig,
  type RedisTestReport,
} from "@/api/setup";
import ManticoreConfigFields from "@/components/ManticoreConfigFields.vue";
import RedisConfigFields from "@/components/RedisConfigFields.vue";
import { useAuthStore } from "@/stores/auth";

const router = useRouter();
const authStore = useAuthStore();

const step = ref(0);
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
  crawler_profile: "P1",
});

function nextStep() {
  if (step.value === 0) {
    if (form.admin_username.trim().length < 2) {
      ElMessage.warning("管理员用户名至少需要 2 个字符");
      return;
    }
    if (form.admin_password.length < 6) {
      ElMessage.warning("管理员密码至少需要 6 位");
      return;
    }
    if (form.admin_password !== form.confirm_password) {
      ElMessage.warning("两次输入的密码不一致");
      return;
    }
  }
  if (step.value < 3) {
    step.value += 1;
  }
}

async function handleTestRedis() {
  testingRedis.value = true;
  try {
    redisReport.value = await testSetupRedis(form.redis);
    if (redisReport.value.ok) {
      ElMessage.success(redisReport.value.message);
    } else {
      ElMessage.error(redisReport.value.message);
    }
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "测试 Redis 连接失败");
  } finally {
    testingRedis.value = false;
  }
}

async function handleTestManticore() {
  testingManticore.value = true;
  try {
    manticoreReport.value = await testSetupManticore(form.manticore);
    if (manticoreReport.value.ok) {
      ElMessage.success(manticoreReport.value.message);
    } else {
      ElMessage.warning(manticoreReport.value.message);
    }
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "测试 Manticore 失败");
  } finally {
    testingManticore.value = false;
  }
}

async function handleCreateManticoreTable() {
  try {
    await ElMessageBox.confirm(
      "确认在当前空 Manticore 实例创建 torrents 表？已存在的表绝对不会被修改。",
      "显式初始化建表确认",
      { type: "warning", confirmButtonText: "确认建表", cancelButtonText: "取消" },
    );
  } catch {
    return;
  }

  creatingTable.value = true;
  try {
    const res = await initSetupManticoreTable(form.manticore);
    ElMessage.success(res.message);
    await handleTestManticore();
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
  if (form.filterMode === "legacy" && !form.legacy_filter_json.trim()) {
    ElMessage.warning("请粘贴或上传旧版 torrent-indexer config.json 内容，或切回内置默认线上预设");
    return;
  }

  submitting.value = true;
  try {
    await submitSetupInit({
      admin_username: form.admin_username.trim(),
      admin_password: form.admin_password,
      redis: form.redis,
      manticore: form.manticore,
      legacy_filter_json: form.filterMode === "legacy" ? form.legacy_filter_json : null,
      create_default_workers: form.create_default_workers,
      crawler_port: form.crawler_port,
      crawler_profile: form.crawler_profile,
    });
    await authStore.refreshBootstrap();
    ElMessage.success("系统初始化完成，已自动登录控制台");
    await router.replace("/");
  } catch (err) {
    ElMessage.error(err instanceof ApiError ? err.message : "初始化失败");
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <main class="min-h-screen bg-slate-900 py-10 px-4 flex justify-center items-start">
    <div class="w-full max-w-4xl rounded-2xl bg-white shadow-2xl border border-slate-200 p-8">
      <div class="flex items-center justify-between gap-4 mb-6 pb-4 border-b border-slate-100">
        <div class="flex items-center gap-3">
          <div class="h-10 w-10 rounded-xl bg-indigo-50 text-indigo-600 flex items-center justify-center">
            <Rocket class="h-5 w-5" />
          </div>
          <div>
            <h1 class="text-xl font-semibold text-slate-900">hameta 首次启动引导向导</h1>
            <p class="text-xs text-slate-500">
              配置管理员账密、Redis、Manticore 存量表兼容校验与线上过滤规则预设
            </p>
          </div>
        </div>
        <span class="text-xs px-2.5 py-1 rounded-full bg-slate-100 text-slate-600 font-mono">
          步骤 {{ step + 1 }} / 4
        </span>
      </div>

      <el-steps :active="step" finish-status="success" align-center class="mb-8">
        <el-step title="管理员账号" />
        <el-step title="Redis 连接" />
        <el-step title="Manticore 校验" />
        <el-step title="规则与节点" />
      </el-steps>

      <!-- Step 1: 管理员 -->
      <el-form v-show="step === 0" label-position="top" class="max-w-lg mx-auto">
        <el-form-item label="管理员用户名">
          <el-input v-model="form.admin_username" placeholder="至少 2 个字符" />
        </el-form-item>
        <el-form-item label="登录密码 (至少 6 位)">
          <el-input v-model="form.admin_password" type="password" show-password />
        </el-form-item>
        <el-form-item label="确认登录密码">
          <el-input v-model="form.confirm_password" type="password" show-password />
        </el-form-item>
        <el-alert
          type="info"
          :closable="false"
          title="提示：若未来忘记账密，可随时在服务器执行 hameta admin reset --username <user> --password <pass> 重置"
        />
      </el-form>

      <!-- Step 2: Redis -->
      <el-form v-show="step === 1" label-position="top">
        <RedisConfigFields v-model="form.redis" :test-report="redisReport" />
        <div class="mt-4 flex justify-end">
          <el-button type="primary" plain :loading="testingRedis" @click="handleTestRedis">
            一键测试 Redis 连接与读写
          </el-button>
        </div>
      </el-form>

      <!-- Step 3: Manticore -->
      <el-form v-show="step === 2" label-position="top">
        <ManticoreConfigFields v-model="form.manticore" :test-report="manticoreReport" />
        <div class="mt-4 flex justify-end gap-3">
          <el-button
            v-if="manticoreReport?.schema && !manticoreReport.schema.table_exists"
            type="warning"
            :loading="creatingTable"
            @click="handleCreateManticoreTable"
          >
            初始化新建 torrents 表（仅空库）
          </el-button>
          <el-button type="primary" plain :loading="testingManticore" @click="handleTestManticore">
            测试连接并核验 DESCRIBE torrents
          </el-button>
        </div>
      </el-form>

      <!-- Step 4: 过滤规则预设与默认节点 -->
      <el-form v-show="step === 3" label-position="top" class="space-y-4">
        <el-form-item label="过滤清洗规则基线">
          <el-radio-group v-model="form.filterMode">
            <el-radio value="preset">
              使用内置线上生产推荐预设（67 关键词 / 2 文件正则 / 15 标题正则 / 1 自定义规则 / 3 整种规则 / 7 上限 + 动态清洗词库）
            </el-radio>
            <el-radio value="legacy">导入旧版 torrent-indexer config.json</el-radio>
          </el-radio-group>
        </el-form-item>

        <div v-if="form.filterMode === 'legacy'" class="space-y-2">
          <div class="flex items-center gap-3">
            <label class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded border border-slate-300 text-xs text-slate-700 cursor-pointer hover:bg-slate-50">
              <Upload class="h-3.5 w-3.5" />
              选择本地 JSON 文件
              <input type="file" accept=".json,application/json" class="hidden" @change="handleLegacyFileUpload" />
            </label>
            <span class="text-xs text-slate-500">支持完整 config.json 或单独 filter 对象</span>
          </div>
          <el-input
            v-model="form.legacy_filter_json"
            type="textarea"
            :rows="6"
            placeholder="请粘贴旧版 config.json 内容..."
          />
        </div>

        <el-divider content-position="left">默认 Worker 实例初始化（创建后保持停止状态，待您检查后启动）</el-divider>

        <el-form-item>
          <el-checkbox v-model="form.create_default_workers">
            自动创建默认采集实例 (crawler-1) 与索引入库实例 (indexer-1)
          </el-checkbox>
        </el-form-item>

        <div v-if="form.create_default_workers" class="grid grid-cols-1 gap-4 md:grid-cols-2">
          <el-form-item label="默认 crawler-1 UDP 端口">
            <el-input-number v-model="form.crawler_port" :min="1" :max="65535" class="w-full!" />
          </el-form-item>
          <el-form-item label="默认 DHT 爬取档位">
            <el-select v-model="form.crawler_profile" class="w-full!">
              <el-option v-for="lv in 17" :key="lv" :label="`P${lv}`" :value="`P${lv}`" />
            </el-select>
          </el-form-item>
        </div>
      </el-form>

      <div class="mt-8 pt-4 border-t border-slate-100 flex items-center justify-between">
        <el-button :disabled="step === 0" @click="step -= 1">上一步</el-button>
        <div class="flex gap-3">
          <el-button v-if="step < 3" type="primary" @click="nextStep">下一步</el-button>
          <el-button
            v-else
            type="success"
            :loading="submitting"
            @click="handleSubmitInit"
          >
            <CheckCircle2 class="h-4 w-4 mr-1.5" />
            完成初始化并进入面板
          </el-button>
        </div>
      </div>
    </div>
  </main>
</template>
