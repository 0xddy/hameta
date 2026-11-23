<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { ElMessage } from "element-plus";
import {
  ArrowDown,
  ArrowUp,
  BarChart3,
  BookOpen,
  CheckCircle2,
  Copy,
  Download,
  FlaskConical,
  History,
  Layers,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  Search,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  Trash2,
  Upload,
} from "lucide-vue-next";
import {
  discardFilterDraft,
  discoverAdCandidates,
  getFilterPresets,
  getFilterState,
  getFilterStats,
  importLegacyFilter,
  listFilterVersions,
  publishFilterDraft,
  resetFilterPreset,
  runPlaygroundTest,
  samplePlaygroundTorrents,
  saveFilterDraft,
  validateFilterBundle,
  rollbackFilterVersion,
  type AbDiffStatus,
  type AdPhraseRule,
  type AdRegexRule,
  type DiscoveredAdCandidate,
  type FilterBundle,
  type FilterPresetsResponse,
  type FilterStatsResponse,
  type FilterVersionDto,
  type IndexerRuleStatusDto,
  type PlaygroundInput,
  type PlaygroundTestResponse,
  type PolicyRule,
  type RuleMatcher,
} from "@/api/filter";
import ByteSizeInput from "@/components/filter/ByteSizeInput.vue";
import RuleEditorDrawer from "@/components/filter/RuleEditorDrawer.vue";
import ObsHorizontalBars from "@/components/ui/ObsHorizontalBars.vue";
import ObsModal from "@/components/ui/ObsModal.vue";
import ObsNumberInput from "@/components/ui/ObsNumberInput.vue";
import ObsSelect from "@/components/ui/ObsSelect.vue";
import { useTelemetryStore } from "@/stores/telemetry";
import { fmtBytes, fmtCount, formatDateTime } from "@/utils/format";

const route = useRoute();
const router = useRouter();
const telemetry = useTelemetryStore();

type WorkbenchTab = "rules" | "cleaner" | "playground" | "stats" | "versions";
const activeTab = ref<WorkbenchTab>("rules");

const loading = ref(false);
const saving = ref(false);
const validating = ref(false);
const publishing = ref(false);

const publishedVer = ref<FilterVersionDto | null>(null);
const draftVer = ref<FilterVersionDto | null>(null);
const onlineIndexers = ref<IndexerRuleStatusDto[]>([]);
const presets = ref<FilterPresetsResponse | null>(null);
const dirty = ref(false);

function makeFallbackBundle(): FilterBundle {
  return {
    version: 1,
    policy: {
      schema_version: 1,
      preset_id: null,
      gates: {
        reject_empty_name: true,
        reject_garbled_name: true,
        min_name_length: { enabled: false, value: 2 },
        max_name_length: { enabled: false, value: 512 },
        reject_empty_files: true,
        reject_all_empty_files: true,
        max_empty_files: { enabled: false, value: 50 },
        max_file_count: { enabled: false, value: 2000 },
        max_file_name_length: { enabled: false, value: 255 },
        min_total_size: { enabled: false, value: 1024 * 1024 },
        max_total_size: { enabled: false, value: 2 * 1024 * 1024 * 1024 * 1024 },
        density_check: { enabled: false, min_file_count: 50, min_avg_file_size: 32768 },
      },
      cleaner: {
        enable_nfkc: true,
        enable_confusable_norm: true,
        enable_deobfuscate_brackets: true,
        enable_collapse_spaced_chars: true,
        enable_strip_leading_site_prefix: true,
        enable_strip_urls_and_domains: true,
        enable_ad_regexes: true,
        enable_ad_phrases: true,
        enable_span_ad_scoring: true,
        span_ad_threshold: 65,
        enable_filename_cross_clean: true,
        enable_cleanup: true,
        reject_if_empty_after_clean: true,
        min_refined_name_length: { enabled: true, value: 2 },
      },
      rules: [],
    },
    ad_phrases: [],
    ad_regexes: [],
  };
}

const bundle = ref<FilterBundle>(makeFallbackBundle());

// Rules tab state
const ruleActionFilter = ref<"all" | "allow" | "reject">("all");
const ruleKeyword = ref("");
const drawerVisible = ref(false);
const drawerMode = ref<"create" | "edit">("edit");
const editingRuleIdx = ref<number | null>(null);
const editingRuleObj = ref<PolicyRule | null>(null);
const templatesModalVisible = ref(false);

// Cleaner tab state
const phraseKeyword = ref("");
const regexKeyword = ref("");
const addPhraseVisible = ref(false);
const phraseModalTab = ref<"single" | "batch">("single");
const singlePhraseText = ref("");
const singlePhraseNote = ref("");
const bulkPhraseText = ref("");
const bulkPhraseNote = ref("");

const addRegexVisible = ref(false);
const regexModalTab = ref<"single" | "batch">("single");
const singleRegexPattern = ref("");
const singleRegexReplacement = ref(" ");
const singleRegexNote = ref("");
const bulkRegexText = ref("");
const bulkRegexReplacement = ref(" ");
const bulkRegexNote = ref("");

// Smart ad template discovery state
const discoveryLoading = ref(false);
const discoveryScanning = ref(false);
const discoveryMeta = ref("");
const discoveredCandidates = ref<DiscoveredAdCandidate[]>([]);
const dismissedDiscoveryKeys = ref<Set<string>>(new Set());

// Publish / Discard / Import / Reset Modals
const publishVisible = ref(false);
const publishNote = ref("");
const importVisible = ref(false);
const importRawJson = ref("");
const importNote = ref("");
const importBusy = ref(false);
const resetVisible = ref(false);
const resetPresetId = ref("online_standard");
const resetIncludeDict = ref(true);
const resetBusy = ref(false);

// Playground state
const pgMode = ref<"published" | "draft" | "ab_compare">("ab_compare");
const pgAutoPlaceholder = ref(true);
const pgQuickTitle = ref("");
const pgSamplesText = ref("");
const pgSamples = ref<PlaygroundInput[]>([
  { name: "[ExampleSite.com] Example.Torrent.2025.1080p.WEB-DL.x264", files: [{ path: "Example.Torrent.2025.1080p.mkv", size: 2_400_000_000 }] },
]);
const pgRunning = ref(false);
const pgSampling = ref(false);
const pgResult = ref<PlaygroundTestResponse | null>(null);

// Stats & Versions state
const statsLoading = ref(false);
const stats = ref<FilterStatsResponse | null>(null);
const versionsLoading = ref(false);
const versions = ref<FilterVersionDto[]>([]);
const rollbackBusyVer = ref<number | null>(null);

const hasDraft = computed(() => Boolean(draftVer.value) || dirty.value);

const filteredRules = computed(() => {
  const kw = ruleKeyword.value.trim().toLowerCase();
  return bundle.value.policy.rules
    .map((rule, index) => ({ rule, index }))
    .filter(({ rule }) => {
      if (ruleActionFilter.value !== "all" && rule.action !== ruleActionFilter.value) return false;
      if (!kw) return true;
      return (
        rule.name.toLowerCase().includes(kw) ||
        rule.id.toLowerCase().includes(kw) ||
        rule.tags.some((t) => t.toLowerCase().includes(kw))
      );
    });
});

const filteredPhrases = computed(() => {
  const kw = phraseKeyword.value.trim().toLowerCase();
  return bundle.value.ad_phrases
    .map((item, index) => ({ item, index }))
    .filter(({ item }) => !kw || item.phrase.toLowerCase().includes(kw) || item.note.toLowerCase().includes(kw));
});

const filteredRegexes = computed(() => {
  const kw = regexKeyword.value.trim().toLowerCase();
  return bundle.value.ad_regexes
    .map((item, index) => ({ item, index }))
    .filter(
      ({ item }) =>
        !kw ||
        item.pattern.toLowerCase().includes(kw) ||
        item.replacement.toLowerCase().includes(kw) ||
        item.note.toLowerCase().includes(kw),
    );
});

const enabledPhraseCount = computed(() => bundle.value.ad_phrases.filter((x) => x.enabled).length);
const enabledRegexCount = computed(() => bundle.value.ad_regexes.filter((x) => x.enabled).length);

const presetSelectOptions = computed(() =>
  (presets.value?.policy_presets ?? []).map((p) => ({
    value: p.id,
    label: `${p.name} (${p.allow_count} 白 / ${p.reject_count} 黑)`,
  })),
);

const PG_MODE_OPTIONS = [
  { label: "线上 vs 草稿 A/B 对比", value: "ab_compare" },
  { label: "仅用当前编辑器草稿测试", value: "draft" },
  { label: "仅用线上已发布规则测试", value: "published" },
];

const statsBarItems = computed(() => {
  if (!stats.value) return [];
  return stats.value.rule_hits.slice(0, 12).map((h) => ({
    label: `${h.action === "allow" ? "[ALLOW]" : "[REJECT]"} ${h.rule_label}`,
    value: h.hits,
  }));
});

const allowRuleCount = computed(() => bundle.value.policy.rules.filter((r) => r.action === "allow").length);
const rejectRuleCount = computed(() => bundle.value.policy.rules.filter((r) => r.action === "reject").length);

type CleanerStageKey =
  | "enable_nfkc"
  | "enable_confusable_norm"
  | "enable_deobfuscate_brackets"
  | "enable_collapse_spaced_chars"
  | "enable_strip_leading_site_prefix"
  | "enable_strip_urls_and_domains"
  | "enable_ad_regexes"
  | "enable_ad_phrases"
  | "enable_span_ad_scoring"
  | "enable_filename_cross_clean"
  | "enable_cleanup";

const CLEANER_STAGE_META: Array<{
  key: CleanerStageKey;
  num: string;
  title: string;
  desc: string;
  smart?: boolean;
}> = [
  { key: "enable_nfkc", num: "STAGE 01", title: "NFKC 规范化", desc: "全角字母数字与兼容字符统一规范化" },
  { key: "enable_confusable_norm", num: "STAGE 02", title: "同形字与噪声归一", desc: "还原西里尔混写、带圈字符并降噪营销符号", smart: true },
  { key: "enable_deobfuscate_brackets", num: "STAGE 03", title: "方括号混淆拆解", desc: "拆解 [abc . com] 等域名空格伪装" },
  { key: "enable_collapse_spaced_chars", num: "STAGE 04", title: "拉宽字符折叠", desc: "恢复 w w w . x x x 等插空推广词" },
  { key: "enable_strip_leading_site_prefix", num: "STAGE 05", title: "发布站前缀剥离", desc: "移除片头 [站点名] / 【发布组】 前缀" },
  { key: "enable_strip_urls_and_domains", num: "STAGE 06", title: "URL / 域名剥离", desc: "擦除正文中挟带的推广网址与域名" },
  { key: "enable_ad_regexes", num: "STAGE 07", title: "正则擦除引擎", desc: "按优先级顺序执行自定义正则模式替换" },
  { key: "enable_ad_phrases", num: "STAGE 08", title: "广告短语词典", desc: "大小写不敏感字面量短语批量擦除" },
  { key: "enable_span_ad_scoring", num: "STAGE 09", title: "语块智能打分剥离", desc: "char n-gram + 语块边界打分擦除变种引流", smart: true },
  { key: "enable_filename_cross_clean", num: "STAGE 10", title: "主文件名交叉对齐", desc: "结合最大主文件名锚定并剥离外侧未知水印", smart: true },
  { key: "enable_cleanup", num: "STAGE 11", title: "残留标点裁剪", desc: "合并连续空白并清理首尾冗余符号" },
];

function toggleCleanerStage(key: CleanerStageKey) {
  bundle.value.policy.cleaner[key] = !bundle.value.policy.cleaner[key];
  markDirty();
}

function exportBundleJson() {
  const payload = JSON.stringify(bundle.value, null, 2);
  const blob = new Blob([payload], { type: "application/json;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `hameta-filter-v${publishedVer.value?.version ?? "draft"}.json`;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
  ElMessage.success("已导出当前规则配置 JSON");
}

function moveRegex(idx: number, delta: -1 | 1) {
  const arr = bundle.value.ad_regexes;
  const target = idx + delta;
  if (target < 0 || target >= arr.length) return;
  [arr[idx], arr[target]] = [arr[target], arr[idx]];
  markDirty();
}

function matcherBadge(m: RuleMatcher): string {
  switch (m.type) {
    case "keyword_group":
      return `关键词 (${m.words.length})`;
    case "extension_rule":
      return `后缀集 (${m.extensions.length})`;
    case "title_pattern_preset":
      return `标题特征 (${m.patterns.length})`;
    case "file_constraint":
      return "文件约束";
    case "substantive_content":
      return "实质内容";
    case "size_ratio":
      return `占比 ≥${Math.round(m.max_ratio * 100)}%`;
    case "custom_regex":
      return `自定义正则 (${m.patterns.length})`;
    case "composite":
      return `复合 ${m.logic.toUpperCase()} (${m.conditions.length})`;
  }
}

function markDirty() {
  dirty.value = true;
  telemetry.filterHasDraft = true;
}

function deepClone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function applyLoadedState(st: Awaited<ReturnType<typeof getFilterState>>) {
  publishedVer.value = st.published;
  draftVer.value = st.draft;
  onlineIndexers.value = st.online_indexers;
  const activeBundle = st.draft?.bundle ?? st.published?.bundle ?? makeFallbackBundle();
  const cloned = deepClone(activeBundle);
  cloned.policy.gates = {
    ...makeFallbackBundle().policy.gates,
    ...(cloned.policy.gates ?? {}),
  };
  cloned.policy.cleaner = {
    ...makeFallbackBundle().policy.cleaner,
    ...(cloned.policy.cleaner ?? {}),
  };
  bundle.value = cloned;
  dirty.value = false;
  telemetry.filterHasDraft = Boolean(st.has_draft || st.draft);
}

async function fetchAllInitial() {
  loading.value = true;
  try {
    const [st, pr] = await Promise.all([getFilterState(), getFilterPresets()]);
    presets.value = pr;
    applyLoadedState(st);
    if (pr.policy_presets.length > 0) {
      resetPresetId.value = pr.policy_presets[0].id;
    }
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "加载过滤工作台状态失败");
  } finally {
    loading.value = false;
  }
}

async function handleValidate() {
  validating.value = true;
  try {
    const res = await validateFilterBundle(bundle.value);
    ElMessage.success(
      `校验通过：${res.counts.enabled_rules}/${res.counts.total_rules} 条规则启用 · ${res.counts.ad_phrases} 短语 · ${res.counts.ad_regexes} 正则`,
    );
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "配置校验未通过");
  } finally {
    validating.value = false;
  }
}

async function handleSaveDraft(silent = false): Promise<boolean> {
  saving.value = true;
  try {
    const saved = await saveFilterDraft(bundle.value, "通过可视化工作台保存草稿");
    draftVer.value = saved;
    dirty.value = false;
    telemetry.filterHasDraft = true;
    if (!silent) ElMessage.success(`草稿已保存为 v${saved.version}`);
    return true;
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "保存草稿失败");
    return false;
  } finally {
    saving.value = false;
  }
}

async function handleDiscardDraft() {
  try {
    await discardFilterDraft();
    const st = await getFilterState();
    applyLoadedState(st);
    ElMessage.success("已放弃草稿并恢复线上已发布版本");
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "放弃草稿失败");
  }
}

function openPublishModal() {
  publishVisible.value = true;
}

async function confirmPublish() {
  publishing.value = true;
  try {
    if (dirty.value || !draftVer.value) {
      const ok = await handleSaveDraft(true);
      if (!ok) {
        publishing.value = false;
        return;
      }
    }
    const res = await publishFilterDraft(publishNote.value.trim() || undefined);
    const okCount = res.reload_results.filter((r) => r.ok).length;
    const failCount = res.reload_results.length - okCount;
    ElMessage.success(
      `已发布 v${res.published.version}` +
        (res.reload_results.length > 0
          ? `，热推送 ${okCount} 个 Indexer 成功` + (failCount > 0 ? ` (${failCount} 个失败)` : "")
          : ""),
    );
    publishVisible.value = false;
    const st = await getFilterState();
    applyLoadedState(st);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "发布失败");
  } finally {
    publishing.value = false;
  }
}

async function submitImportLegacy() {
  if (!importRawJson.value.trim()) {
    ElMessage.warning("请粘贴旧版 rules / cleaner JSON 内容");
    return;
  }
  importBusy.value = true;
  try {
    const res = await importLegacyFilter(importRawJson.value, true, importNote.value.trim() || "导入旧版规则");
    ElMessage.success(`已转换 (${res.source_format}) 并存入草稿`);
    importVisible.value = false;
    const st = await getFilterState();
    applyLoadedState(st);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "导入解析失败");
  } finally {
    importBusy.value = false;
  }
}

async function submitResetPreset() {
  resetBusy.value = true;
  try {
    await resetFilterPreset(resetPresetId.value, resetIncludeDict.value, `重置为预设模板 ${resetPresetId.value}`);
    ElMessage.success("已生成预设模板草稿");
    resetVisible.value = false;
    const st = await getFilterState();
    applyLoadedState(st);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "重置失败");
  } finally {
    resetBusy.value = false;
  }
}

// Rule list editing helpers
function openCreateRule() {
  drawerMode.value = "create";
  editingRuleIdx.value = null;
  editingRuleObj.value = null;
  drawerVisible.value = true;
}

function openEditRule(idx: number) {
  drawerMode.value = "edit";
  editingRuleIdx.value = idx;
  editingRuleObj.value = bundle.value.policy.rules[idx];
  drawerVisible.value = true;
}

function saveRuleFromDrawer(nextRule: PolicyRule) {
  if (drawerMode.value === "create" || editingRuleIdx.value === null) {
    bundle.value.policy.rules.push(nextRule);
  } else {
    bundle.value.policy.rules.splice(editingRuleIdx.value, 1, nextRule);
  }
  markDirty();
}

function moveRule(idx: number, delta: -1 | 1) {
  const arr = bundle.value.policy.rules;
  const target = idx + delta;
  if (target < 0 || target >= arr.length) return;
  [arr[idx], arr[target]] = [arr[target], arr[idx]];
  markDirty();
}

function duplicateRule(idx: number) {
  const src = bundle.value.policy.rules[idx];
  const copy = deepClone(src);
  copy.id = `${src.id}_copy_${Date.now().toString(36).slice(-4)}`;
  copy.name = `${src.name} (副本)`;
  bundle.value.policy.rules.splice(idx + 1, 0, copy);
  markDirty();
}

function deleteRule(idx: number) {
  bundle.value.policy.rules.splice(idx, 1);
  markDirty();
}

function addFromTemplate(tplRule: PolicyRule) {
  const copy = deepClone(tplRule);
  if (bundle.value.policy.rules.some((r) => r.id === copy.id)) {
    copy.id = `${copy.id}_${Date.now().toString(36).slice(-4)}`;
  }
  bundle.value.policy.rules.push(copy);
  markDirty();
  templatesModalVisible.value = false;
  ElMessage.success(`已追加规则模板「${copy.name}」`);
}

// Cleaner inline + modal helpers
function openAddPhraseModal(tab: "single" | "batch" = "single") {
  phraseModalTab.value = tab;
  singlePhraseText.value = "";
  singlePhraseNote.value = "";
  bulkPhraseText.value = "";
  bulkPhraseNote.value = "";
  addPhraseVisible.value = true;
}

function submitAddPhraseModal() {
  if (phraseModalTab.value === "single") {
    const p = singlePhraseText.value.trim();
    if (!p) {
      ElMessage.warning("请输入广告短语");
      return;
    }
    const rule: AdPhraseRule = {
      id: `phr_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 5)}`,
      phrase: p,
      enabled: true,
      note: singlePhraseNote.value.trim(),
    };
    bundle.value.ad_phrases.unshift(rule);
    markDirty();
    addPhraseVisible.value = false;
    ElMessage.success(`已添加广告短语「${p}」`);
    return;
  }

  const lines = bulkPhraseText.value
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  if (lines.length === 0) {
    ElMessage.warning("请至少输入一条广告短语");
    return;
  }
  const existing = new Set(bundle.value.ad_phrases.map((x) => x.phrase));
  const note = bulkPhraseNote.value.trim() || "批量导入";
  let added = 0;
  for (const line of lines) {
    if (existing.has(line)) continue;
    existing.add(line);
    bundle.value.ad_phrases.unshift({
      id: `phr_${Date.now().toString(36)}_${added}`,
      phrase: line,
      enabled: true,
      note,
    });
    added += 1;
  }
  markDirty();
  addPhraseVisible.value = false;
  ElMessage.success(`已追加 ${added} 条广告短语`);
}

function openAddRegexModal(tab: "single" | "batch" = "single") {
  regexModalTab.value = tab;
  singleRegexPattern.value = "";
  singleRegexReplacement.value = " ";
  singleRegexNote.value = "";
  bulkRegexText.value = "";
  bulkRegexReplacement.value = " ";
  bulkRegexNote.value = "";
  addRegexVisible.value = true;
}

function fillRegexPreset(pattern: string, replacement: string, note: string) {
  singleRegexPattern.value = pattern;
  singleRegexReplacement.value = replacement;
  singleRegexNote.value = note;
}

function submitAddRegexModal() {
  if (regexModalTab.value === "single") {
    const pat = singleRegexPattern.value.trim();
    if (!pat) {
      ElMessage.warning("请输入正则表达式");
      return;
    }
    const rule: AdRegexRule = {
      id: `rgx_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 5)}`,
      pattern: pat,
      replacement: singleRegexReplacement.value,
      enabled: true,
      note: singleRegexNote.value.trim(),
    };
    bundle.value.ad_regexes.unshift(rule);
    markDirty();
    addRegexVisible.value = false;
    ElMessage.success("已添加动态广告正则");
    return;
  }

  const lines = bulkRegexText.value
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  if (lines.length === 0) {
    ElMessage.warning("请至少输入一条正则表达式");
    return;
  }
  const existing = new Set(bundle.value.ad_regexes.map((x) => x.pattern));
  const note = bulkRegexNote.value.trim() || "批量正则导入";
  let added = 0;
  for (const pat of lines) {
    if (existing.has(pat)) continue;
    existing.add(pat);
    bundle.value.ad_regexes.unshift({
      id: `rgx_${Date.now().toString(36)}_${added}`,
      pattern: pat,
      replacement: bulkRegexReplacement.value,
      enabled: true,
      note,
    });
    added += 1;
  }
  markDirty();
  addRegexVisible.value = false;
  ElMessage.success(`已追加 ${added} 条动态广告正则`);
}

// Playground helpers
function syncSamplesTextFromList() {
  pgSamplesText.value = pgSamples.value.map((s) => s.name).join("\n");
}

function applyTitleFromQuickOrLines() {
  const t = pgQuickTitle.value.trim();
  if (t) {
    pgSamples.value.unshift({ name: t, files: [{ path: `${t}.mkv`, size: 1_500_000_000 }] });
    pgQuickTitle.value = "";
    syncSamplesTextFromList();
  }
}

async function loadSamplesFromBackend(source: "manticore_latest" | "manticore_random" | "rejected_recent") {
  pgSampling.value = true;
  try {
    const res = await samplePlaygroundTorrents({ source, limit: 15 });
    pgSamples.value = res.samples;
    syncSamplesTextFromList();
    ElMessage.success(`已载入 ${res.samples.length} 条真实样本 (${source})`);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "拉取样本失败");
  } finally {
    pgSampling.value = false;
  }
}

async function runPlayground() {
  // Merge lines from textarea if edited manually
  const lines = pgSamplesText.value
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);

  let inputs: PlaygroundInput[];
  if (lines.length > 0 && lines.length !== pgSamples.value.length) {
    inputs = lines.map((name) => ({ name }));
  } else if (lines.length > 0) {
    inputs = lines.map((name, i) => ({
      ...(pgSamples.value[i] ?? {}),
      name,
    }));
  } else {
    ElMessage.warning("请至少输入或拉取 1 条测试标题");
    return;
  }

  pgRunning.value = true;
  try {
    pgResult.value = await runPlaygroundTest({
      mode: pgMode.value,
      draft_bundle: pgMode.value === "published" ? undefined : bundle.value,
      samples: inputs,
      auto_placeholder_file: pgAutoPlaceholder.value,
    });
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "演练测试失败");
  } finally {
    pgRunning.value = false;
  }
}

function sendTitleToPlayground(title: string) {
  pgSamples.value = [{ name: title, files: [] }, ...pgSamples.value.slice(0, 14)];
  syncSamplesTextFromList();
  activeTab.value = "playground";
}

function abDiffLabel(s: AbDiffStatus): { text: string; cls: string } {
  switch (s) {
    case "newly_rejected":
      return { text: "新增拦截", cls: "bg-rose-500/15 text-rose-300 border-rose-500/35" };
    case "newly_accepted":
      return { text: "新增放行", cls: "bg-emerald-500/15 text-emerald-300 border-emerald-500/35" };
    case "refined_name_changed":
      return { text: "清洗标题变化", cls: "bg-amber-500/15 text-amber-300 border-amber-500/35" };
    case "unchanged_rejected":
      return { text: "一致拦截", cls: "bg-slate-800/80 text-slate-400 border-slate-700" };
    default:
      return { text: "一致放行", cls: "bg-slate-800/80 text-slate-300 border-slate-700" };
  }
}

// Smart ad template discovery helpers
const visibleDiscoveryCandidates = computed(() => {
  const existing = new Set(bundle.value.ad_phrases.map((x) => x.phrase.trim().toLowerCase()));
  return discoveredCandidates.value.filter((c) => {
    const k = c.phrase.trim().toLowerCase();
    return k && !existing.has(k) && !dismissedDiscoveryKeys.value.has(k);
  });
});

async function loadLiveDiscoveryCandidates(silent = false) {
  discoveryLoading.value = true;
  try {
    const res = await getFilterStats();
    stats.value = res;
    discoveredCandidates.value = res.discovered_ad_candidates ?? [];
    discoveryMeta.value =
      discoveredCandidates.value.length > 0
        ? `已同步在线 Indexer 旁路统计，发现 ${discoveredCandidates.value.length} 个高频模板`
        : "当前在线 Indexer 暂未观察到跨标题重复的新水印候选";
    if (!silent) {
      ElMessage.success(discoveryMeta.value);
    }
  } catch (e) {
    if (!silent) {
      ElMessage.error(e instanceof Error ? e.message : "拉取在线发现候选失败");
    }
  } finally {
    discoveryLoading.value = false;
  }
}

async function scanManticoreDiscoveryCandidates() {
  discoveryScanning.value = true;
  try {
    const res = await discoverAdCandidates(300);
    discoveredCandidates.value = res.candidates ?? [];
    discoveryMeta.value =
      res.scanned_titles > 0
        ? `已扫描库内近期 ${res.scanned_titles} 条种子并合并在线发现，识别 ${visibleDiscoveryCandidates.value.length} 个候选模板`
        : `来源 ${res.source}，识别 ${visibleDiscoveryCandidates.value.length} 个候选模板`;
    ElMessage.success(discoveryMeta.value);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "扫描库内疑似广告模板失败");
  } finally {
    discoveryScanning.value = false;
  }
}

function adoptDiscoveredCandidate(cand: DiscoveredAdCandidate) {
  const phrase = cand.phrase.trim();
  if (!phrase) return;
  const existing = new Set(bundle.value.ad_phrases.map((x) => x.phrase.trim().toLowerCase()));
  if (!existing.has(phrase.toLowerCase())) {
    bundle.value.ad_phrases.unshift({
      id: `phr_auto_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 5)}`,
      phrase,
      enabled: true,
      note: `智能统计自发现（覆盖 ${cand.distinct_titles} 个不同标题）`,
    });
    markDirty();
    ElMessage.success(`已将「${phrase}」加入动态广告短语草稿`);
  }
  dismissedDiscoveryKeys.value.add(phrase.toLowerCase());
}

function dismissDiscoveredCandidate(cand: DiscoveredAdCandidate) {
  dismissedDiscoveryKeys.value.add(cand.phrase.trim().toLowerCase());
}

// Stats & Versions loaders
async function loadStats() {
  statsLoading.value = true;
  try {
    stats.value = await getFilterStats();
    if ((stats.value.discovered_ad_candidates?.length ?? 0) > 0 && discoveredCandidates.value.length === 0) {
      discoveredCandidates.value = stats.value.discovered_ad_candidates ?? [];
    }
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "读取命中统计失败");
  } finally {
    statsLoading.value = false;
  }
}

async function loadVersions() {
  versionsLoading.value = true;
  try {
    versions.value = await listFilterVersions(30);
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "读取规则版本历史失败");
  } finally {
    versionsLoading.value = false;
  }
}

async function handleRollback(ver: number) {
  rollbackBusyVer.value = ver;
  try {
    const res = await rollbackFilterVersion(ver);
    ElMessage.success(`已回滚并发布新版本 v${res.published.version}`);
    const st = await getFilterState();
    applyLoadedState(st);
    await loadVersions();
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "版本回滚失败");
  } finally {
    rollbackBusyVer.value = null;
  }
}

function applyRouteDeepLink() {
  const tabQ = route.query.tab;
  const titleQ = route.query.title;
  let changed = false;
  if (tabQ === "playground" || tabQ === "rules" || tabQ === "cleaner" || tabQ === "stats" || tabQ === "versions") {
    activeTab.value = tabQ;
    changed = true;
  }
  if (typeof titleQ === "string" && titleQ.trim()) {
    const t = titleQ.trim();
    pgSamples.value = [{ name: t, files: [{ path: `${t}.mkv`, size: 1_500_000_000 }] }];
    syncSamplesTextFromList();
    activeTab.value = "playground";
    changed = true;
  }
  if (changed) {
    void router.replace({ path: "/filter", query: {} });
  }
}

watch(activeTab, (tab) => {
  if (tab === "cleaner" && discoveredCandidates.value.length === 0 && !discoveryMeta.value) {
    void loadLiveDiscoveryCandidates(true);
  }
  if (tab === "stats") void loadStats();
  if (tab === "versions") void loadVersions();
});

watch(
  () => [route.query.tab, route.query.title],
  () => {
    applyRouteDeepLink();
  },
);

onMounted(async () => {
  syncSamplesTextFromList();
  applyRouteDeepLink();
  await fetchAllInitial();
  if (activeTab.value === "stats") void loadStats();
  if (activeTab.value === "versions") void loadVersions();
});
</script>

<template>
  <div class="space-y-4">
    <!-- 2-Row Governance Header -->
    <section class="obs-card rounded-xl p-4 space-y-3">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div class="flex flex-wrap items-center gap-2">
          <span class="px-2.5 py-0.5 rounded-full text-[11px] font-mono bg-emerald-500/12 border border-emerald-500/30 text-emerald-300 inline-flex items-center gap-1.5">
            <span class="w-1.5 h-1.5 rounded-full bg-emerald-400" />
            线网生效 {{ publishedVer ? `v${publishedVer.version}` : "未发布" }}
          </span>
          <span
            v-if="hasDraft"
            class="px-2.5 py-0.5 rounded-full text-[11px] font-mono bg-amber-500/15 border border-amber-500/35 text-amber-300"
          >
            {{ dirty ? "存在未保存编辑" : `草稿待发布 v${draftVer?.version}` }}
          </span>
          <span
            v-for="ix in onlineIndexers"
            :key="ix.id"
            class="px-2 py-0.5 rounded-md text-[10px] font-mono bg-[#0b0f17] border border-slate-800 text-slate-300"
          >
            {{ ix.id }} · v{{ ix.active_rule_version ?? "—" }}
          </span>
        </div>

        <div class="flex flex-wrap items-center gap-2">
          <button type="button" class="obs-btn obs-btn-ghost px-2.5! py-1.5! text-xs! inline-flex items-center gap-1" @click="exportBundleJson">
            <Download class="w-3.5 h-3.5" />
            导出 JSON
          </button>
          <button type="button" class="obs-btn obs-btn-ghost px-2.5! py-1.5! text-xs! inline-flex items-center gap-1" @click="importVisible = true">
            <Upload class="w-3.5 h-3.5" />
            导入 JSON
          </button>
          <button type="button" class="obs-btn obs-btn-ghost px-2.5! py-1.5! text-xs! inline-flex items-center gap-1" @click="resetVisible = true">
            <RotateCcw class="w-3.5 h-3.5" />
            重置预设
          </button>
          <button type="button" class="obs-btn obs-btn-ghost px-2.5! py-1.5! text-xs! inline-flex items-center gap-1" :disabled="validating" @click="handleValidate">
            <CheckCircle2 class="w-3.5 h-3.5 text-sky-400" />
            {{ validating ? "校验中…" : "校验配置" }}
          </button>
          <button
            v-if="hasDraft"
            type="button"
            class="obs-btn obs-btn-ghost px-2.5! py-1.5! text-xs! text-rose-300!"
            @click="handleDiscardDraft"
          >
            放弃草稿
          </button>
          <button type="button" class="obs-btn obs-btn-ghost px-3! py-1.5! text-xs! inline-flex items-center gap-1" :disabled="saving" @click="handleSaveDraft(false)">
            <Save class="w-3.5 h-3.5 text-amber-300" />
            {{ saving ? "保存中…" : "保存草稿" }}
          </button>
        </div>
      </div>

      <div class="flex flex-wrap items-center gap-2.5 pt-3 border-t border-slate-800/80">
        <input
          v-model="publishNote"
          type="text"
          placeholder="发布说明（可选），例如：收紧碎片体积门限并增补推广词库 · 热推送至在线 Indexer 写入进程"
          class="obs-input flex-1 min-w-[240px] rounded-lg px-3 py-1.5 text-xs"
        />
        <button type="button" class="obs-btn obs-btn-primary px-4! py-1.5! text-xs! shrink-0" :disabled="publishing" @click="openPublishModal">
          {{ publishing ? "发布并推送中…" : "发布并热生效" }}
        </button>
      </div>
    </section>

    <!-- Single Unified Workbench Card -->
    <section class="obs-card rounded-xl overflow-hidden">
      <!-- Pill Tab Navigation Bar -->
      <div class="px-4 py-2.5 border-b border-slate-800/80 bg-[#0b0f17] flex flex-wrap items-center justify-between gap-2 text-xs">
        <div class="flex flex-wrap items-center gap-1.5">
          <button
            type="button"
            class="px-3 py-1.5 rounded-lg font-medium inline-flex items-center gap-1.5 border transition cursor-pointer"
            :class="activeTab === 'rules' ? 'obs-tab-active' : 'border-transparent text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'"
            @click="activeTab = 'rules'"
          >
            <Layers class="w-3.5 h-3.5" />
            <span>规则策略 &amp; 闸门</span>
            <span class="px-1.5 py-0.2 rounded bg-slate-800/90 text-[10px] font-mono-num text-slate-300">{{ bundle.policy.rules.length }}</span>
          </button>
          <button
            type="button"
            class="px-3 py-1.5 rounded-lg font-medium inline-flex items-center gap-1.5 border transition cursor-pointer"
            :class="activeTab === 'cleaner' ? 'obs-tab-active' : 'border-transparent text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'"
            @click="activeTab = 'cleaner'"
          >
            <Sparkles class="w-3.5 h-3.5" />
            <span>标题清洗与词库</span>
            <span class="px-1.5 py-0.2 rounded bg-slate-800/90 text-[10px] font-mono-num text-slate-300">
              {{ bundle.ad_phrases.length + bundle.ad_regexes.length }}
            </span>
          </button>
          <button
            type="button"
            class="px-3 py-1.5 rounded-lg font-medium inline-flex items-center gap-1.5 border transition cursor-pointer"
            :class="activeTab === 'playground' ? 'obs-tab-active' : 'border-transparent text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'"
            @click="activeTab = 'playground'"
          >
            <FlaskConical class="w-3.5 h-3.5" />
            <span>A/B 演练场</span>
          </button>
          <button
            type="button"
            class="px-3 py-1.5 rounded-lg font-medium inline-flex items-center gap-1.5 border transition cursor-pointer"
            :class="activeTab === 'stats' ? 'obs-tab-active' : 'border-transparent text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'"
            @click="activeTab = 'stats'"
          >
            <BarChart3 class="w-3.5 h-3.5" />
            <span>命中统计</span>
          </button>
          <button
            type="button"
            class="px-3 py-1.5 rounded-lg font-medium inline-flex items-center gap-1.5 border transition cursor-pointer"
            :class="activeTab === 'versions' ? 'obs-tab-active' : 'border-transparent text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'"
            @click="activeTab = 'versions'"
          >
            <History class="w-3.5 h-3.5" />
            <span>版本历史</span>
          </button>
        </div>

        <span class="text-[11px] font-mono text-slate-500 hidden md:inline">
          草稿校验与演练通过后可一键热更新生效
        </span>
      </div>

      <!-- TAB 1: RULES & GATES -->
      <div v-if="activeTab === 'rules'" class="p-4 space-y-5">
        <!-- 11 Base Structural Gates in 3 Grouped Cards -->
        <div class="space-y-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <div>
              <h3 class="text-xs font-semibold text-white">前置基础结构闸门 (Structural Gates)</h3>
              <p class="text-[11px] text-slate-400">硬性完整性保护先于一切放行/拦截策略执行</p>
            </div>
            <span class="text-[10px] font-mono text-slate-500 uppercase tracking-wider">12 Built-in Guards</span>
          </div>

          <div class="grid grid-cols-1 md:grid-cols-3 gap-3 text-xs">
            <!-- Group 1: 标题合规检查 -->
            <div class="obs-inset rounded-xl p-3.5 space-y-2.5">
              <div class="text-[11px] font-semibold text-emerald-300 uppercase tracking-wider font-mono border-b border-slate-800/80 pb-1.5">
                标题合规检查
              </div>
              <label class="flex items-center gap-2 text-slate-200 cursor-pointer">
                <input v-model="bundle.policy.gates.reject_empty_name" type="checkbox" class="obs-checkbox" @change="markDirty" />
                <span>拒绝空标题种子</span>
              </label>
              <label
                class="flex items-center gap-2 text-slate-200 cursor-pointer"
                title="拦截含 U+FFFD 替换符、控制符或典型 GBK/UTF-8 错解 Mojibake 的不可读乱码标题（先于白名单执行）"
              >
                <input v-model="bundle.policy.gates.reject_garbled_name" type="checkbox" class="obs-checkbox" @change="markDirty" />
                <span>拒绝编码乱码标题 (U+FFFD / Mojibake)</span>
              </label>
              <div class="flex items-center justify-between gap-2">
                <label class="inline-flex items-center gap-2 text-slate-300 cursor-pointer">
                  <input v-model="bundle.policy.gates.min_name_length.enabled" type="checkbox" class="obs-checkbox" @change="markDirty" />
                  <span>标题最短字数</span>
                </label>
                <div class="w-24">
                  <ObsNumberInput v-model="bundle.policy.gates.min_name_length.value" :min="1" :disabled="!bundle.policy.gates.min_name_length.enabled" @change="markDirty" />
                </div>
              </div>
              <div class="flex items-center justify-between gap-2">
                <label class="inline-flex items-center gap-2 text-slate-300 cursor-pointer">
                  <input v-model="bundle.policy.gates.max_name_length.enabled" type="checkbox" class="obs-checkbox" @change="markDirty" />
                  <span>标题最长字数</span>
                </label>
                <div class="w-24">
                  <ObsNumberInput v-model="bundle.policy.gates.max_name_length.value" :min="1" :disabled="!bundle.policy.gates.max_name_length.enabled" @change="markDirty" />
                </div>
              </div>
              <div class="flex items-center justify-between gap-2">
                <label class="inline-flex items-center gap-2 text-slate-300 cursor-pointer">
                  <input v-model="bundle.policy.gates.max_file_name_length.enabled" type="checkbox" class="obs-checkbox" @change="markDirty" />
                  <span>单文件名最长</span>
                </label>
                <div class="w-24">
                  <ObsNumberInput v-model="bundle.policy.gates.max_file_name_length.value" :min="1" :disabled="!bundle.policy.gates.max_file_name_length.enabled" @change="markDirty" />
                </div>
              </div>
            </div>

            <!-- Group 2: 文件清单与空文件 -->
            <div class="obs-inset rounded-xl p-3.5 space-y-2.5">
              <div class="text-[11px] font-semibold text-sky-300 uppercase tracking-wider font-mono border-b border-slate-800/80 pb-1.5">
                文件清单与空文件
              </div>
              <label class="flex items-center gap-2 text-slate-200 cursor-pointer">
                <input v-model="bundle.policy.gates.reject_empty_files" type="checkbox" class="obs-checkbox" @change="markDirty" />
                <span>拒绝无文件清单种子</span>
              </label>
              <label class="flex items-center gap-2 text-slate-200 cursor-pointer">
                <input v-model="bundle.policy.gates.reject_all_empty_files" type="checkbox" class="obs-checkbox" @change="markDirty" />
                <span>拒绝文件大小全为 0 B</span>
              </label>
              <div class="flex items-center justify-between gap-2">
                <label class="inline-flex items-center gap-2 text-slate-300 cursor-pointer">
                  <input v-model="bundle.policy.gates.max_empty_files.enabled" type="checkbox" class="obs-checkbox" @change="markDirty" />
                  <span>最多 0B 文件</span>
                </label>
                <div class="w-24">
                  <ObsNumberInput v-model="bundle.policy.gates.max_empty_files.value" :min="0" :disabled="!bundle.policy.gates.max_empty_files.enabled" @change="markDirty" />
                </div>
              </div>
              <div class="flex items-center justify-between gap-2">
                <label class="inline-flex items-center gap-2 text-slate-300 cursor-pointer">
                  <input v-model="bundle.policy.gates.max_file_count.enabled" type="checkbox" class="obs-checkbox" @change="markDirty" />
                  <span>文件数量上限</span>
                </label>
                <div class="w-24">
                  <ObsNumberInput v-model="bundle.policy.gates.max_file_count.value" :min="1" :disabled="!bundle.policy.gates.max_file_count.enabled" @change="markDirty" />
                </div>
              </div>
            </div>

            <!-- Group 3: 体积与碎文件密度 -->
            <div class="obs-inset rounded-xl p-3.5 space-y-2.5">
              <div class="text-[11px] font-semibold text-amber-300 uppercase tracking-wider font-mono border-b border-slate-800/80 pb-1.5">
                体积与碎文件密度
              </div>
              <div class="space-y-1.5">
                <label class="inline-flex items-center gap-2 text-slate-300 cursor-pointer">
                  <input v-model="bundle.policy.gates.min_total_size.enabled" type="checkbox" class="obs-checkbox" @change="markDirty" />
                  <span>总体积下限 (Min Total)</span>
                </label>
                <ByteSizeInput
                  v-model="bundle.policy.gates.min_total_size.value"
                  :min="0"
                  @update:model-value="(v) => { bundle.policy.gates.min_total_size.value = v ?? 0; markDirty(); }"
                />
              </div>
              <div class="space-y-1.5">
                <label class="inline-flex items-center gap-2 text-slate-300 cursor-pointer">
                  <input v-model="bundle.policy.gates.max_total_size.enabled" type="checkbox" class="obs-checkbox" @change="markDirty" />
                  <span>总体积上限 (Max Total)</span>
                </label>
                <ByteSizeInput
                  v-model="bundle.policy.gates.max_total_size.value"
                  :min="0"
                  @update:model-value="(v) => { bundle.policy.gates.max_total_size.value = v ?? 0; markDirty(); }"
                />
              </div>
              <div class="space-y-1.5 pt-0.5">
                <label class="inline-flex items-center gap-2 text-slate-300 cursor-pointer">
                  <input v-model="bundle.policy.gates.density_check.enabled" type="checkbox" class="obs-checkbox" @change="markDirty" />
                  <span>碎文件密度保护 (最少文件数 / 平均最小 B)</span>
                </label>
                <div class="grid grid-cols-2 gap-2">
                  <ObsNumberInput v-model="bundle.policy.gates.density_check.min_file_count" :min="1" :disabled="!bundle.policy.gates.density_check.enabled" @change="markDirty" />
                  <ObsNumberInput v-model="bundle.policy.gates.density_check.min_avg_file_size" :min="1" :disabled="!bundle.policy.gates.density_check.enabled" @change="markDirty" />
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Rule Priority Chain -->
        <div class="pt-3 border-t border-slate-800/80 space-y-3">
          <div class="flex flex-wrap items-center justify-between gap-2.5">
            <div class="flex flex-wrap items-center gap-2">
              <div class="bg-[#0b0f17] border border-slate-800 p-0.5 rounded-lg inline-flex text-xs">
                <button
                  type="button"
                  class="px-2.5 py-1 rounded-md transition cursor-pointer"
                  :class="ruleActionFilter === 'all' ? 'bg-slate-800 text-white font-medium' : 'text-slate-400 hover:text-slate-200'"
                  @click="ruleActionFilter = 'all'"
                >
                  全部 ({{ bundle.policy.rules.length }})
                </button>
                <button
                  type="button"
                  class="px-2.5 py-1 rounded-md transition cursor-pointer"
                  :class="ruleActionFilter === 'allow' ? 'bg-emerald-500/15 text-emerald-300 font-medium' : 'text-slate-400 hover:text-slate-200'"
                  @click="ruleActionFilter = 'allow'"
                >
                  白名单 ALLOW ({{ allowRuleCount }})
                </button>
                <button
                  type="button"
                  class="px-2.5 py-1 rounded-md transition cursor-pointer"
                  :class="ruleActionFilter === 'reject' ? 'bg-rose-500/15 text-rose-300 font-medium' : 'text-slate-400 hover:text-slate-200'"
                  @click="ruleActionFilter = 'reject'"
                >
                  黑名单 REJECT ({{ rejectRuleCount }})
                </button>
              </div>

              <div class="relative w-56">
                <Search class="w-3.5 h-3.5 text-slate-500 absolute left-2.5 top-1/2 -translate-y-1/2 pointer-events-none" />
                <input v-model="ruleKeyword" type="text" placeholder="搜索规则名 / ID / 标签" class="obs-input rounded-lg pl-8 pr-3 py-1 text-xs" />
              </div>
            </div>

            <div class="flex items-center gap-2">
              <button type="button" class="obs-btn obs-btn-ghost px-3! py-1.5! text-xs! inline-flex items-center gap-1" @click="templatesModalVisible = true">
                <BookOpen class="w-3.5 h-3.5 text-sky-400" />
                从模板库添加
              </button>
              <button type="button" class="obs-btn obs-btn-primary px-3! py-1.5! text-xs! inline-flex items-center gap-1" @click="openCreateRule">
                <Plus class="w-3.5 h-3.5" />
                新建规则
              </button>
            </div>
          </div>

          <div class="space-y-2.5">
            <div
              v-for="{ rule, index } in filteredRules"
              :key="rule.id"
              class="rounded-xl border p-3.5 flex flex-wrap items-center justify-between gap-3 transition-colors"
              :class="
                !rule.enabled
                  ? 'border-slate-800/70 bg-[#090d16]/50 opacity-55'
                  : rule.action === 'allow'
                    ? 'border-emerald-500/30 bg-emerald-500/[0.03] hover:border-emerald-500/45'
                    : 'border-slate-800 bg-[#0b0f17]/90 hover:border-slate-700'
              "
            >
              <div class="flex items-start gap-3 min-w-0 flex-1">
                <input v-model="rule.enabled" type="checkbox" class="obs-checkbox mt-1" @change="markDirty" />
                <span class="font-mono-num text-[11px] text-slate-500 w-7 pt-0.5">#{{ index + 1 }}</span>

                <div class="min-w-0 space-y-1">
                  <div class="flex flex-wrap items-center gap-2">
                    <span
                      class="px-2 py-0.5 rounded-md text-[10px] font-mono border inline-flex items-center gap-1"
                      :class="
                        rule.action === 'allow'
                          ? 'bg-emerald-500/12 text-emerald-300 border-emerald-500/30'
                          : 'bg-rose-500/12 text-rose-300 border-rose-500/30'
                      "
                    >
                      <ShieldCheck v-if="rule.action === 'allow'" class="w-3 h-3" />
                      <ShieldAlert v-else class="w-3 h-3" />
                      {{ rule.action.toUpperCase() }}
                    </span>
                    <span class="font-semibold text-xs text-white">{{ rule.name }}</span>
                    <span class="px-2 py-0.5 rounded-md bg-sky-500/10 border border-sky-500/25 text-[10px] font-mono text-sky-300">
                      {{ matcherBadge(rule.matcher) }}
                    </span>
                    <span class="text-[10px] font-mono text-slate-500">{{ rule.id }}</span>
                  </div>

                  <div v-if="rule.note || rule.tags.length > 0" class="flex flex-wrap items-center gap-2 text-[11px] text-slate-400">
                    <span v-for="tag in rule.tags" :key="tag" class="px-1.5 py-0.5 rounded bg-slate-800/80 text-slate-300 text-[10px]">
                      {{ tag }}
                    </span>
                    <span v-if="rule.note" class="truncate">{{ rule.note }}</span>
                  </div>
                </div>
              </div>

              <div class="flex items-center gap-1 shrink-0">
                <button type="button" class="obs-btn obs-btn-ghost p-1.5!" title="上移优先级" :disabled="index === 0" @click="moveRule(index, -1)">
                  <ArrowUp class="w-3.5 h-3.5" />
                </button>
                <button type="button" class="obs-btn obs-btn-ghost p-1.5!" title="下移优先级" :disabled="index === bundle.policy.rules.length - 1" @click="moveRule(index, 1)">
                  <ArrowDown class="w-3.5 h-3.5" />
                </button>
                <button type="button" class="obs-btn obs-btn-ghost p-1.5!" title="编辑规则" @click="openEditRule(index)">
                  <Pencil class="w-3.5 h-3.5 text-emerald-400" />
                </button>
                <button type="button" class="obs-btn obs-btn-ghost p-1.5!" title="复制" @click="duplicateRule(index)">
                  <Copy class="w-3.5 h-3.5 text-slate-400" />
                </button>
                <button type="button" class="obs-btn obs-btn-danger p-1.5!" title="删除" @click="deleteRule(index)">
                  <Trash2 class="w-3.5 h-3.5" />
                </button>
              </div>
            </div>

            <div v-if="filteredRules.length === 0" class="py-12 text-center text-xs text-slate-500">
              没有符合条件的策略规则，可点击右上角新增规则或载入预设模板
            </div>
          </div>
        </div>
      </div>

      <!-- TAB 2: TITLE CLEANER -->
      <div v-else-if="activeTab === 'cleaner'" class="p-4 space-y-5">
        <!-- 11 Clickable Pipeline Stages + Threshold + Post-Clean Gates -->
        <div class="space-y-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <div>
              <h3 class="text-xs font-semibold text-white">标题清洗流水线 (Cleaner Pipeline)</h3>
              <p class="text-[11px] text-slate-400">按序执行字符规范化、同形字降噪、混淆拆解、规则擦除与类 NLP 智能补盲</p>
            </div>
            <span class="text-[10px] font-mono text-slate-500">点击卡片即可启停对应清洗阶段</span>
          </div>

          <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-2.5">
            <button
              v-for="st in CLEANER_STAGE_META"
              :key="st.key"
              type="button"
              class="text-left rounded-xl border p-3 transition-all cursor-pointer space-y-1.5"
              :class="
                bundle.policy.cleaner[st.key]
                  ? st.smart
                    ? 'border-cyan-500/40 bg-cyan-500/[0.06] hover:border-cyan-400/60'
                    : 'border-emerald-500/35 bg-emerald-500/[0.06] hover:border-emerald-500/50'
                  : 'border-slate-800 bg-[#0b0f17]/70 opacity-65 hover:opacity-95'
              "
              @click="toggleCleanerStage(st.key)"
            >
              <div class="flex items-center justify-between gap-2 text-[10px] font-mono">
                <div class="flex items-center gap-1.5">
                  <span :class="bundle.policy.cleaner[st.key] ? (st.smart ? 'text-cyan-300' : 'text-emerald-400') : 'text-slate-500'">{{ st.num }}</span>
                  <span
                    v-if="st.smart"
                    class="px-1.5 py-0.2 rounded bg-cyan-500/15 border border-cyan-500/30 text-cyan-300 text-[9px]"
                  >
                    智能
                  </span>
                </div>
                <span
                  class="px-1.5 py-0.5 rounded border text-[9px]"
                  :class="
                    bundle.policy.cleaner[st.key]
                      ? st.smart
                        ? 'bg-cyan-500/15 text-cyan-200 border-cyan-500/35'
                        : 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30'
                      : 'bg-slate-800 text-slate-500 border-slate-700'
                  "
                >
                  {{ bundle.policy.cleaner[st.key] ? "ON" : "OFF" }}
                </span>
              </div>
              <div class="text-xs font-semibold text-white">{{ st.title }}</div>
              <div class="text-[11px] text-slate-400 leading-snug">{{ st.desc }}</div>
            </button>
          </div>

          <div class="grid grid-cols-1 lg:grid-cols-3 gap-2.5 text-xs">
            <div class="obs-inset rounded-xl p-3 flex flex-wrap items-center justify-between gap-3">
              <div class="space-y-0.5">
                <div class="text-cyan-200 font-medium">阶段 9 · 语块广告判定阈值</div>
                <div class="text-[10px] text-slate-400">建议 60–75；越低越激进 (范围 20–95)</div>
              </div>
              <div class="w-24">
                <ObsNumberInput
                  v-model="bundle.policy.cleaner.span_ad_threshold"
                  :min="20"
                  :max="95"
                  :disabled="!bundle.policy.cleaner.enable_span_ad_scoring"
                  @change="markDirty"
                />
              </div>
            </div>
            <label class="obs-inset rounded-xl p-3 flex items-center gap-2.5 cursor-pointer text-amber-200">
              <input v-model="bundle.policy.cleaner.reject_if_empty_after_clean" type="checkbox" class="obs-checkbox" @change="markDirty" />
              <span>12 · 清洗后标题为空则直接拒绝入库</span>
            </label>
            <div class="obs-inset rounded-xl p-3 flex flex-wrap items-center justify-between gap-3">
              <label class="inline-flex items-center gap-2 text-slate-200 cursor-pointer">
                <input v-model="bundle.policy.cleaner.min_refined_name_length.enabled" type="checkbox" class="obs-checkbox" @change="markDirty" />
                <span>13 · 清洗后最短有效字符限制</span>
              </label>
              <div class="w-24">
                <ObsNumberInput
                  v-model="bundle.policy.cleaner.min_refined_name_length.value"
                  :min="1"
                  :disabled="!bundle.policy.cleaner.min_refined_name_length.enabled"
                  @change="markDirty"
                />
              </div>
            </div>
          </div>
        </div>

        <!-- Dual Phrase & Regex Tables with Frosted Overlay -->
        <div class="grid grid-cols-1 xl:grid-cols-2 gap-4 text-xs">
          <!-- 左列：动态广告短语 -->
          <section
            class="relative overflow-hidden obs-inset rounded-xl p-4 space-y-3 flex flex-col min-w-0"
            :class="bundle.policy.cleaner.enable_ad_phrases ? '' : 'border-amber-500/35'"
          >
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="flex flex-wrap items-center gap-2">
                <span class="font-semibold text-slate-100 text-sm">动态广告短语</span>
                <span class="px-2 py-0.5 rounded bg-slate-800 text-cyan-300 font-mono-num text-[11px]">
                  启用 {{ enabledPhraseCount }} / 共 {{ bundle.ad_phrases.length }}
                </span>
                <span class="px-1.5 py-0.5 rounded bg-slate-900 border border-slate-800 text-slate-400 font-mono-num text-[10px]">
                  阶段 8 控制
                </span>
              </div>
              <div class="flex items-center gap-1.5">
                <input
                  v-model="phraseKeyword"
                  type="text"
                  placeholder="搜索短语 / 备注..."
                  class="obs-input rounded-lg px-2.5 py-1 text-xs w-36"
                />
                <button
                  type="button"
                  class="obs-btn obs-btn-primary px-3! py-1! rounded-lg text-xs! shrink-0"
                  @click="openAddPhraseModal('single')"
                >
                  + 添加
                </button>
              </div>
            </div>

            <div class="max-h-[420px] overflow-y-auto obs-scroll rounded-lg border border-slate-800/80 bg-[#080b11]">
              <table class="w-full text-left text-xs">
                <thead class="sticky top-0 z-10 bg-[#0c111b] border-b border-slate-800 text-[11px] text-slate-500">
                  <tr>
                    <th class="py-2 px-2.5 font-medium">启用</th>
                    <th class="py-2 px-2 font-medium">广告短语（点击直接编辑）</th>
                    <th class="py-2 px-2 font-medium">备注</th>
                    <th class="py-2 px-2.5 text-right font-medium">操作</th>
                  </tr>
                </thead>
                <tbody class="divide-y divide-slate-800/60">
                  <tr
                    v-for="{ item, index } in filteredPhrases"
                    :key="item.id"
                    class="hover:bg-slate-800/30 transition"
                    :class="item.enabled ? '' : 'opacity-60'"
                  >
                    <td class="py-2 px-2.5 align-middle w-14">
                      <button
                        type="button"
                        class="px-2 py-0.5 rounded-full text-[10px] font-mono-num border cursor-pointer transition"
                        :class="
                          item.enabled
                            ? 'bg-emerald-500/20 text-emerald-300 border-emerald-500/40'
                            : 'bg-slate-800 text-slate-400 border-slate-700'
                        "
                        @click="item.enabled = !item.enabled; markDirty()"
                      >
                        {{ item.enabled ? "ON" : "OFF" }}
                      </button>
                    </td>
                    <td class="py-2 px-2 align-middle min-w-[160px]">
                      <input
                        v-model="item.phrase"
                        type="text"
                        placeholder="广告短语..."
                        class="obs-input w-full rounded-md px-2.5 py-1 text-xs font-mono-num"
                        @input="markDirty"
                      />
                    </td>
                    <td class="py-2 px-2 align-middle min-w-[110px]">
                      <input
                        v-model="item.note"
                        type="text"
                        placeholder="备注..."
                        class="obs-input w-full rounded-md px-2.5 py-1 text-xs text-slate-300"
                        @input="markDirty"
                      />
                    </td>
                    <td class="py-2 px-2.5 align-middle text-right w-14">
                      <button
                        type="button"
                        class="obs-btn-danger px-2 py-1 rounded text-[11px] cursor-pointer"
                        @click="bundle.ad_phrases.splice(index, 1); markDirty()"
                      >
                        删
                      </button>
                    </td>
                  </tr>

                  <tr v-if="filteredPhrases.length === 0">
                    <td colspan="4" class="py-8 px-4 text-center text-xs text-slate-500">
                      {{ phraseKeyword.trim() ? "未找到匹配的广告短语" : "暂无广告短语规则，请点击右上角「+ 添加」" }}
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>

            <div
              v-if="!bundle.policy.cleaner.enable_ad_phrases"
              class="absolute inset-0 z-20 bg-[#080a0f]/80 backdrop-blur-[2px] flex flex-col items-center justify-center gap-3 p-6 text-center"
            >
              <span class="px-3 py-1 rounded-full bg-amber-500/15 border border-amber-500/35 text-amber-300 font-mono-num text-[11px] font-semibold">
                阶段 8 · 动态广告短语剥离已关闭
              </span>
              <p class="text-xs text-slate-300 max-w-xs leading-relaxed">
                当前阶段开关处于关闭状态，列表中的 <span class="font-mono-num text-amber-200">{{ bundle.ad_phrases.length }}</span> 条广告短语暂不参与标题清洗。
              </p>
              <button
                type="button"
                class="obs-btn obs-btn-primary px-4! py-1.5! rounded-lg text-xs!"
                @click="bundle.policy.cleaner.enable_ad_phrases = true; markDirty()"
              >
                ⚡ 立即开启阶段 8
              </button>
            </div>
          </section>

          <!-- 右列：动态广告正则 -->
          <section
            class="relative overflow-hidden obs-inset rounded-xl p-4 space-y-3 flex flex-col min-w-0"
            :class="bundle.policy.cleaner.enable_ad_regexes ? '' : 'border-amber-500/35'"
          >
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="flex flex-wrap items-center gap-2">
                <span class="font-semibold text-slate-100 text-sm">动态广告正则</span>
                <span class="px-2 py-0.5 rounded bg-slate-800 text-cyan-300 font-mono-num text-[11px]">
                  启用 {{ enabledRegexCount }} / 共 {{ bundle.ad_regexes.length }}
                </span>
                <span class="px-1.5 py-0.5 rounded bg-slate-900 border border-slate-800 text-slate-400 font-mono-num text-[10px]">
                  阶段 7 控制 · 按顺序
                </span>
              </div>
              <div class="flex items-center gap-1.5">
                <input
                  v-model="regexKeyword"
                  type="text"
                  placeholder="搜索正则 / 备注..."
                  class="obs-input rounded-lg px-2.5 py-1 text-xs w-36"
                />
                <button
                  type="button"
                  class="obs-btn obs-btn-primary px-3! py-1! rounded-lg text-xs! shrink-0"
                  @click="openAddRegexModal('single')"
                >
                  + 添加
                </button>
              </div>
            </div>

            <div class="max-h-[420px] overflow-y-auto obs-scroll rounded-lg border border-slate-800/80 bg-[#080b11]">
              <table class="w-full text-left text-xs">
                <thead class="sticky top-0 z-10 bg-[#0c111b] border-b border-slate-800 text-[11px] text-slate-500">
                  <tr>
                    <th class="py-2 px-2.5 font-medium">启用 / 顺序</th>
                    <th class="py-2 px-2 font-medium">正则表达式 &amp; 替换为（点击直接编辑）</th>
                    <th class="py-2 px-2 font-medium">备注</th>
                    <th class="py-2 px-2.5 text-right font-medium">操作</th>
                  </tr>
                </thead>
                <tbody class="divide-y divide-slate-800/60">
                  <tr
                    v-for="{ item, index } in filteredRegexes"
                    :key="item.id"
                    class="hover:bg-slate-800/30 transition"
                    :class="item.enabled ? '' : 'opacity-60'"
                  >
                    <td class="py-2 px-2.5 align-top w-24">
                      <div class="flex items-center gap-1">
                        <button
                          type="button"
                          class="px-2 py-0.5 rounded-full text-[10px] font-mono-num border cursor-pointer transition"
                          :class="
                            item.enabled
                              ? 'bg-emerald-500/20 text-emerald-300 border-emerald-500/40'
                              : 'bg-slate-800 text-slate-400 border-slate-700'
                          "
                          @click="item.enabled = !item.enabled; markDirty()"
                        >
                          {{ item.enabled ? "ON" : "OFF" }}
                        </button>
                        <button
                          type="button"
                          title="上移执行优先级"
                          :disabled="index === 0"
                          class="px-1 py-0.5 rounded text-[10px] font-mono-num obs-btn-ghost"
                          :class="index === 0 ? 'opacity-30 cursor-not-allowed' : 'cursor-pointer'"
                          @click="moveRegex(index, -1)"
                        >
                          ↑
                        </button>
                        <button
                          type="button"
                          title="下移执行优先级"
                          :disabled="index === bundle.ad_regexes.length - 1"
                          class="px-1 py-0.5 rounded text-[10px] font-mono-num obs-btn-ghost"
                          :class="index === bundle.ad_regexes.length - 1 ? 'opacity-30 cursor-not-allowed' : 'cursor-pointer'"
                          @click="moveRegex(index, 1)"
                        >
                          ↓
                        </button>
                      </div>
                      <div class="text-[10px] text-slate-500 font-mono-num mt-1">#{{ index + 1 }}</div>
                    </td>
                    <td class="py-2 px-2 align-top min-w-[200px] space-y-1">
                      <input
                        v-model="item.pattern"
                        type="text"
                        placeholder="正则表达式..."
                        class="obs-input w-full rounded-md px-2.5 py-1 text-xs font-mono-num text-cyan-200"
                        @input="markDirty"
                      />
                      <div class="flex items-center gap-1.5">
                        <span class="text-[10px] text-slate-500 shrink-0">替换为</span>
                        <input
                          v-model="item.replacement"
                          type="text"
                          placeholder="空字符串或空格"
                          class="obs-input flex-1 rounded-md px-2 py-0.5 text-[11px] font-mono-num text-slate-200"
                          @input="markDirty"
                        />
                      </div>
                    </td>
                    <td class="py-2 px-2 align-top min-w-[100px]">
                      <input
                        v-model="item.note"
                        type="text"
                        placeholder="备注..."
                        class="obs-input w-full rounded-md px-2.5 py-1 text-xs text-slate-300"
                        @input="markDirty"
                      />
                    </td>
                    <td class="py-2 px-2.5 align-top text-right w-14">
                      <button
                        type="button"
                        class="obs-btn-danger px-2 py-1 rounded text-[11px] cursor-pointer"
                        @click="bundle.ad_regexes.splice(index, 1); markDirty()"
                      >
                        删
                      </button>
                    </td>
                  </tr>

                  <tr v-if="filteredRegexes.length === 0">
                    <td colspan="4" class="py-8 px-4 text-center text-xs text-slate-500">
                      {{ regexKeyword.trim() ? "未找到匹配的广告正则" : "暂无广告正则规则，请点击右上角「+ 添加」" }}
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>

            <div
              v-if="!bundle.policy.cleaner.enable_ad_regexes"
              class="absolute inset-0 z-20 bg-[#080a0f]/80 backdrop-blur-[2px] flex flex-col items-center justify-center gap-3 p-6 text-center"
            >
              <span class="px-3 py-1 rounded-full bg-amber-500/15 border border-amber-500/35 text-amber-300 font-mono-num text-[11px] font-semibold">
                阶段 7 · 动态广告正则替换已关闭
              </span>
              <p class="text-xs text-slate-300 max-w-xs leading-relaxed">
                当前阶段开关处于关闭状态，列表中的 <span class="font-mono-num text-amber-200">{{ bundle.ad_regexes.length }}</span> 条广告正则暂不参与标题清洗。
              </p>
              <button
                type="button"
                class="obs-btn obs-btn-primary px-4! py-1.5! rounded-lg text-xs!"
                @click="bundle.policy.cleaner.enable_ad_regexes = true; markDirty()"
              >
                ⚡ 立即开启阶段 7
              </button>
            </div>
          </section>
        </div>

        <!-- Bypass Unsupervised Discovery Card -->
        <section class="obs-inset rounded-xl p-4 space-y-3 text-xs">
          <div class="flex flex-wrap items-center justify-between gap-2.5">
            <div class="space-y-0.5">
              <div class="flex items-center gap-2">
                <span class="font-semibold text-slate-100 text-sm">智能统计自发现 · 疑似新广告模板</span>
                <span class="px-1.5 py-0.5 rounded bg-cyan-500/15 border border-cyan-500/30 text-cyan-300 font-mono text-[10px]">
                  无监督 DF 挖掘
                </span>
              </div>
              <p class="text-[11px] text-slate-400">
                旁路统计高频挂载在多个不同种子首尾、且尚未被当前词库拦截的新站前缀与水印候选
              </p>
            </div>

            <div class="flex flex-wrap items-center gap-2">
              <button
                type="button"
                class="obs-btn obs-btn-ghost px-3! py-1.5! text-xs! inline-flex items-center gap-1"
                :disabled="discoveryLoading"
                @click="loadLiveDiscoveryCandidates(false)"
              >
                <RefreshCw class="w-3.5 h-3.5" :class="discoveryLoading ? 'animate-spin text-cyan-400' : ''" />
                拉取在线 Indexer 发现
              </button>
              <button
                type="button"
                class="obs-btn obs-btn-primary px-3! py-1.5! text-xs! inline-flex items-center gap-1"
                :disabled="discoveryScanning"
                @click="scanManticoreDiscoveryCandidates"
              >
                <Sparkles class="w-3.5 h-3.5" />
                {{ discoveryScanning ? "扫描分析中…" : "扫描库内近期种子" }}
              </button>
            </div>
          </div>

          <div v-if="discoveryMeta" class="text-[11px] font-mono text-cyan-300/90">
            {{ discoveryMeta }}
          </div>

          <div v-if="visibleDiscoveryCandidates.length > 0" class="grid grid-cols-1 lg:grid-cols-2 gap-2.5">
            <div
              v-for="cand in visibleDiscoveryCandidates"
              :key="cand.phrase"
              class="rounded-lg border border-slate-800 bg-[#080b11] p-3 flex items-start justify-between gap-3"
            >
              <div class="min-w-0 space-y-1">
                <div class="flex flex-wrap items-center gap-1.5">
                  <span class="px-2 py-0.5 rounded bg-amber-500/12 border border-amber-500/30 text-amber-200 font-mono-num text-xs font-semibold break-all">
                    {{ cand.phrase }}
                  </span>
                  <span class="px-1.5 py-0.5 rounded bg-slate-800 text-slate-300 font-mono-num text-[10px]">
                    出现 {{ cand.occurrences }} 次 · 覆盖 {{ cand.distinct_titles }} 个不同标题
                  </span>
                  <span
                    v-if="cand.suggested_score > 0"
                    class="px-1.5 py-0.5 rounded bg-cyan-500/15 border border-cyan-500/30 text-cyan-300 font-mono-num text-[10px]"
                  >
                    风险分 {{ cand.suggested_score }}
                  </span>
                </div>
                <div v-if="cand.sample_titles.length > 0" class="text-[11px] font-mono text-slate-500 truncate">
                  样本: {{ cand.sample_titles[0] }}
                </div>
              </div>

              <div class="flex items-center gap-1.5 shrink-0">
                <button
                  type="button"
                  class="obs-btn obs-btn-primary px-2.5! py-1! rounded text-[11px]! cursor-pointer"
                  @click="adoptDiscoveredCandidate(cand)"
                >
                  + 加入短语
                </button>
                <button
                  type="button"
                  class="obs-btn obs-btn-ghost px-2! py-1! rounded text-[11px]! cursor-pointer"
                  @click="dismissDiscoveredCandidate(cand)"
                >
                  忽略
                </button>
              </div>
            </div>
          </div>

          <div v-else class="py-5 text-center text-[11px] text-slate-500 border border-dashed border-slate-800 rounded-lg">
            暂无待处理的疑似新广告模板；可点击右上角「扫描库内近期种子」对近期入库标题做即时挖掘
          </div>
        </section>
      </div>

      <!-- TAB 3: A/B PLAYGROUND -->
      <div v-else-if="activeTab === 'playground'" class="p-4">
        <div class="grid grid-cols-1 xl:grid-cols-12 gap-4 items-start">
          <!-- Left 5 cols: Input & Mode -->
          <section class="xl:col-span-5 obs-inset rounded-xl p-4 space-y-3 text-xs">
            <div class="flex items-center justify-between">
              <h3 class="font-semibold text-white">演练样本与对比模式</h3>
              <span class="text-[11px] font-mono text-slate-400">支持真库抽样或自由输入</span>
            </div>

            <div class="space-y-2.5">
              <ObsSelect v-model="pgMode" :options="PG_MODE_OPTIONS" class="w-full" />
              <label class="inline-flex items-center gap-2 text-xs text-slate-300 cursor-pointer">
                <input v-model="pgAutoPlaceholder" type="checkbox" class="obs-checkbox" />
                纯标题样本自动附加合理媒体占位文件（避免触发空文件闸门）
              </label>
            </div>

            <div class="flex flex-wrap gap-1.5">
              <button type="button" class="obs-btn obs-btn-ghost px-2.5! py-1! text-[11px]!" :disabled="pgSampling" @click="loadSamplesFromBackend('manticore_latest')">
                抽样最新入库
              </button>
              <button type="button" class="obs-btn obs-btn-ghost px-2.5! py-1! text-[11px]!" :disabled="pgSampling" @click="loadSamplesFromBackend('manticore_random')">
                随机库内抽样
              </button>
              <button type="button" class="obs-btn obs-btn-ghost px-2.5! py-1! text-[11px]!" :disabled="pgSampling" @click="loadSamplesFromBackend('rejected_recent')">
                抽样最近拦截
              </button>
            </div>

            <div class="flex items-center gap-2">
              <input
                v-model="pgQuickTitle"
                type="text"
                placeholder="快速插入一条待测种子标题…"
                class="obs-input rounded-lg px-3 py-1.5 text-xs flex-1"
                @keyup.enter="applyTitleFromQuickOrLines"
              />
              <button type="button" class="obs-btn obs-btn-ghost px-2.5! py-1.5! text-xs!" @click="applyTitleFromQuickOrLines">插入</button>
            </div>

            <div>
              <label class="block text-[11px] text-slate-400 mb-1">测试标题列表（每行 1 条）</label>
              <textarea
                v-model="pgSamplesText"
                rows="10"
                class="w-full rounded-lg bg-[#07090e] border border-slate-800 focus:border-cyan-500/60 focus:outline-none p-2.5 text-xs font-mono text-cyan-200 leading-relaxed obs-scroll"
              />
            </div>

            <button
              type="button"
              class="obs-btn obs-btn-primary w-full py-2! text-xs! font-semibold inline-flex items-center justify-center gap-1.5"
              :disabled="pgRunning"
              @click="runPlayground"
            >
              <Play class="w-3.5 h-3.5" />
              {{ pgRunning ? "正在真实 Rust 引擎执行演练…" : "运行规则与标题清洗演练" }}
            </button>
          </section>

          <!-- Right 7 cols: Simulation Results & Trace -->
          <section class="xl:col-span-7 obs-inset rounded-xl p-4 space-y-4">
            <div class="flex items-center justify-between border-b border-slate-800 pb-2.5">
              <h3 class="text-xs font-semibold text-white">实时演练评估结果</h3>
              <span v-if="pgResult" class="text-[11px] font-mono text-slate-400">
                Published v{{ pgResult.published_version }}
                <template v-if="pgResult.draft_version !== null"> · Draft v{{ pgResult.draft_version }}</template>
              </span>
            </div>

            <template v-if="pgResult">
              <div v-if="pgResult.ab_report" class="bg-[#0b0f17] border border-slate-800 p-3 rounded-xl grid grid-cols-5 gap-2 text-center font-mono-num text-xs">
                <div>
                  <div class="text-[10px] text-slate-500">样本总数</div>
                  <div class="text-white font-bold mt-0.5">{{ pgResult.ab_report.summary.total }}</div>
                </div>
                <div>
                  <div class="text-[10px] text-emerald-400">新增放行</div>
                  <div class="text-emerald-300 font-bold mt-0.5">{{ pgResult.ab_report.summary.newly_accepted }}</div>
                </div>
                <div>
                  <div class="text-[10px] text-rose-400">新增拦截</div>
                  <div class="text-rose-300 font-bold mt-0.5">{{ pgResult.ab_report.summary.newly_rejected }}</div>
                </div>
                <div>
                  <div class="text-[10px] text-amber-400">清洗标题变</div>
                  <div class="text-amber-300 font-bold mt-0.5">{{ pgResult.ab_report.summary.refined_name_changed }}</div>
                </div>
                <div>
                  <div class="text-[10px] text-slate-500">一致状态</div>
                  <div class="text-slate-300 font-bold mt-0.5">
                    {{ pgResult.ab_report.summary.unchanged_accepted + pgResult.ab_report.summary.unchanged_rejected }}
                  </div>
                </div>
              </div>

              <div v-if="pgResult.ab_report" class="space-y-2 max-h-[520px] overflow-auto obs-scroll pr-1">
                <div
                  v-for="item in pgResult.ab_report.items"
                  :key="item.sample_index"
                  class="bg-[#0b0f17] border border-slate-800 p-3 rounded-xl space-y-1.5 text-xs"
                >
                  <div class="flex flex-wrap items-center justify-between gap-2">
                    <span class="px-2 py-0.5 rounded-md text-[10px] border font-mono" :class="abDiffLabel(item.diff_status).cls">
                      {{ abDiffLabel(item.diff_status).text }}
                    </span>
                    <div class="text-[11px] font-mono text-slate-400">
                      线上: <b :class="item.published_accepted ? 'text-emerald-300' : 'text-rose-300'">{{ item.published_accepted ? "放行" : "拦截" }}</b>
                      →
                      草稿: <b :class="item.draft_accepted ? 'text-emerald-300' : 'text-rose-300'">{{ item.draft_accepted ? "放行" : "拦截" }}</b>
                    </div>
                  </div>
                  <div class="font-mono text-[11px] text-slate-400 break-all">raw: {{ item.raw_name }}</div>
                  <div class="text-emerald-200 break-all">draft_refined: {{ item.draft_refined_name }}</div>
                  <div v-if="item.draft_hit" class="text-[11px] text-rose-300 font-mono">
                    草稿命中规则: [{{ item.draft_hit.rule_label }}] {{ item.draft_hit.detail }}
                  </div>
                </div>
              </div>

              <div v-else class="space-y-2.5 max-h-[520px] overflow-auto obs-scroll pr-1">
                <div
                  v-for="item in pgResult.results"
                  :key="item.sample_index"
                  class="bg-[#0b0f17] border border-slate-800 p-3 rounded-xl space-y-2 text-xs"
                >
                  <div class="flex items-center justify-between gap-2">
                    <span
                      class="px-2 py-0.5 rounded-md text-[10px] font-mono border"
                      :class="item.accepted ? 'bg-emerald-500/12 text-emerald-300 border-emerald-500/30' : 'bg-rose-500/12 text-rose-300 border-rose-500/30'"
                    >
                      {{ item.accepted ? "ACCEPTED 放行" : "REJECTED 拦截" }}
                    </span>
                    <span v-if="item.hit" class="text-[11px] font-mono text-rose-300">
                      {{ item.hit.rule_label }} · {{ item.hit.detail }}
                    </span>
                    <span v-else-if="item.allowed_by" class="text-[11px] font-mono text-emerald-300">
                      豁免: {{ item.allowed_by.rule_label }}
                    </span>
                  </div>
                  <div class="font-mono text-[11px] text-slate-400 break-all">原标题: {{ item.raw_name }}</div>
                  <div class="text-emerald-200 font-medium break-all">清洗后: {{ item.refined_name }}</div>

                  <div
                    v-if="item.cleaning_trace.steps.some((s) => s.changed)"
                    class="pt-1.5 border-t border-slate-800 space-y-1 text-[11px] font-mono"
                  >
                    <div
                      v-for="step in item.cleaning_trace.steps.filter((s) => s.changed)"
                      :key="step.stage"
                      class="text-slate-400 break-all space-y-0.5"
                    >
                      <div>
                        <span class="text-sky-300">[{{ step.stage }}]</span> {{ step.before }} → <span class="text-emerald-300">{{ step.after }}</span>
                      </div>
                      <div v-if="step.detail" class="pl-3 text-[10px] text-cyan-300/90">
                        ↳ {{ step.detail }}
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </template>

            <div v-else class="py-20 text-center text-xs text-slate-500">
              在左侧设置好测试标题或抽取库内真实种子，点击运行即可查看逐步清洗轨迹与规则差异对比
            </div>
          </section>
        </div>
      </div>

      <!-- TAB 4: HIT STATS -->
      <div v-else-if="activeTab === 'stats'" class="p-4 space-y-4">
        <div class="flex items-center justify-between">
          <span class="text-xs text-slate-400">汇总在线 Indexer 子进程的实时规则评估与最近拦截样本</span>
          <button type="button" class="obs-btn obs-btn-ghost px-3! py-1.5! text-xs! inline-flex items-center gap-1" @click="loadStats">
            <RefreshCw class="w-3.5 h-3.5" :class="statsLoading ? 'animate-spin text-emerald-400' : ''" />
            刷新统计
          </button>
        </div>

        <template v-if="stats">
          <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
            <div class="obs-inset rounded-xl p-4">
              <div class="text-[11px] text-slate-400">累计规则评估总量</div>
              <div class="mt-1.5 text-2xl font-bold font-mono-num text-white">{{ fmtCount(stats.total_evaluated) }}</div>
            </div>
            <div class="obs-inset rounded-xl p-4">
              <div class="text-[11px] text-slate-400">规则引擎放行</div>
              <div class="mt-1.5 text-2xl font-bold font-mono-num text-emerald-300">{{ fmtCount(stats.total_accepted) }}</div>
            </div>
            <div class="obs-inset rounded-xl p-4">
              <div class="text-[11px] text-slate-400">规则引擎拦截</div>
              <div class="mt-1.5 text-2xl font-bold font-mono-num text-rose-300">{{ fmtCount(stats.total_rejected) }}</div>
            </div>
          </div>

          <div class="grid grid-cols-1 xl:grid-cols-2 gap-4">
            <div class="obs-inset rounded-xl p-4 space-y-3">
              <h3 class="text-xs font-semibold text-white">规则命中次数分布</h3>
              <ObsHorizontalBars v-if="statsBarItems.length > 0" :items="statsBarItems" color="#f59e0b" />
              <div v-else class="py-10 text-center text-xs text-slate-500">暂无规则命中计数</div>
            </div>

            <div class="obs-inset rounded-xl p-4 space-y-3">
              <h3 class="text-xs font-semibold text-white">在线 Indexer 子进程执行概览</h3>
              <div class="space-y-2">
                <div v-for="w in stats.workers" :key="w.id" class="bg-[#0b0f17] border border-slate-800 p-3 rounded-xl flex items-center justify-between text-xs font-mono">
                  <div>
                    <div class="text-white font-semibold">{{ w.display_name }} ({{ w.id }})</div>
                    <div class="text-[11px] text-slate-400">状态: {{ w.runtime_state }} · 规则 v{{ w.active_rule_version ?? "—" }}</div>
                  </div>
                  <div class="text-right font-mono-num">
                    <div class="text-emerald-300">{{ fmtCount(w.total_accepted) }} 放行</div>
                    <div class="text-rose-300">{{ fmtCount(w.total_rejected) }} 拦截</div>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <section class="obs-inset rounded-xl overflow-hidden">
            <div class="px-4 py-3 border-b border-slate-800 bg-[#0b0f17] text-xs font-semibold text-white">
              最近被拦截样本 ({{ stats.recent_rejected.length }})
            </div>
            <div class="overflow-x-auto max-h-96 obs-scroll">
              <table class="w-full text-left border-collapse text-xs">
                <thead class="bg-[#0b0f17] border-b border-slate-800 text-[10px] font-mono uppercase tracking-wider text-slate-400">
                  <tr>
                    <th class="px-4 py-2">时间</th>
                    <th class="px-3 py-2">标题 &amp; InfoHash</th>
                    <th class="px-3 py-2">体积 / 文件数</th>
                    <th class="px-3 py-2">命中规则 / 详情</th>
                    <th class="px-4 py-2 text-right">复测</th>
                  </tr>
                </thead>
                <tbody>
                  <tr
                    v-for="(s, idx) in stats.recent_rejected"
                    :key="`${s.info_hash}:${idx}`"
                    class="border-b border-slate-800/50 hover:bg-slate-800/35"
                  >
                    <td class="px-4 py-2 font-mono-num text-[11px] text-slate-400 whitespace-nowrap">{{ formatDateTime(s.rejected_at_unix) }}</td>
                    <td class="px-3 py-2 max-w-md">
                      <div class="text-slate-100 truncate">{{ s.raw_name }}</div>
                      <div class="text-[10px] font-mono text-cyan-400/80">{{ s.info_hash }}</div>
                    </td>
                    <td class="px-3 py-2 font-mono-num text-[11px] text-slate-300 whitespace-nowrap">
                      {{ fmtBytes(s.total_size) }} · {{ s.file_count }}f
                    </td>
                    <td class="px-3 py-2 text-[11px] font-mono text-rose-300">
                      [{{ s.hit.rule_label }}] {{ s.hit.detail }}
                    </td>
                    <td class="px-4 py-2 text-right whitespace-nowrap">
                      <button type="button" class="obs-btn obs-btn-ghost px-2! py-1! text-[11px]!" @click="sendTitleToPlayground(s.raw_name)">
                        送入演练场
                      </button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </section>
        </template>
      </div>

      <!-- TAB 5: VERSION HISTORY -->
      <div v-else-if="activeTab === 'versions'">
        <div class="px-4 py-3 border-b border-slate-800 bg-[#0b0f17] flex items-center justify-between">
          <span class="text-xs font-semibold text-white">过滤规则持久化发布历史</span>
          <button type="button" class="obs-btn obs-btn-ghost px-2.5! py-1! text-xs! inline-flex items-center gap-1" @click="loadVersions">
            <RefreshCw class="w-3.5 h-3.5" :class="versionsLoading ? 'animate-spin text-emerald-400' : ''" />
            刷新
          </button>
        </div>

        <div class="divide-y divide-slate-800/60">
          <div v-for="v in versions" :key="v.id" class="p-3.5 flex flex-wrap items-center justify-between gap-3 text-xs">
            <div class="space-y-1">
              <div class="flex items-center gap-2">
                <span class="font-mono font-bold text-emerald-300">v{{ v.version }}</span>
                <span
                  class="px-2 py-0.5 rounded-md text-[10px] font-mono border"
                  :class="
                    v.status === 'published'
                      ? 'bg-emerald-500/12 text-emerald-300 border-emerald-500/30'
                      : v.status === 'draft'
                        ? 'bg-amber-500/12 text-amber-300 border-amber-500/30'
                        : 'bg-slate-800 text-slate-400 border-slate-700'
                  "
                >
                  {{ v.status }}
                </span>
                <span class="text-slate-100">{{ v.note || "无发布说明" }}</span>
              </div>
              <div class="text-[11px] font-mono text-slate-500">
                创建人 {{ v.created_by }} · 创建于 {{ formatDateTime(v.created_at) }}
                <template v-if="v.published_at"> · 发布于 {{ formatDateTime(v.published_at) }}</template>
              </div>
            </div>

            <button
              v-if="v.status !== 'draft'"
              type="button"
              class="obs-btn obs-btn-ghost px-3! py-1.5! text-xs!"
              :disabled="rollbackBusyVer === v.version"
              @click="handleRollback(v.version)"
            >
              {{ rollbackBusyVer === v.version ? "回滚中…" : "回滚并热生效" }}
            </button>
          </div>
          <div v-if="versions.length === 0" class="py-12 text-center text-xs text-slate-500">暂无版本记录</div>
        </div>
      </div>
    </section>

    <!-- Rule Editor Drawer -->
    <RuleEditorDrawer
      v-model="drawerVisible"
      :mode="drawerMode"
      :rule="editingRuleObj"
      :extension-packs="presets?.extension_packs ?? []"
      :keyword-packs="presets?.keyword_packs ?? []"
      @save="saveRuleFromDrawer"
    />

    <!-- Rule Templates Modal -->
    <ObsModal v-model="templatesModalVisible" title="从规则模板库快速追加" max-width="640px">
      <div class="max-h-96 overflow-auto obs-scroll space-y-2 text-xs">
        <div
          v-for="tpl in presets?.rule_templates ?? []"
          :key="tpl.id"
          class="obs-inset p-3 rounded-lg flex items-center justify-between gap-3"
        >
          <div class="min-w-0">
            <div class="flex items-center gap-2">
              <span class="px-1.5 py-0.5 rounded bg-sky-500/15 text-sky-300 text-[10px] font-mono">{{ tpl.group }}</span>
              <span class="font-semibold text-slate-100">{{ tpl.name }}</span>
            </div>
            <div class="text-[11px] text-slate-400 mt-1">{{ tpl.description }}</div>
          </div>
          <button type="button" class="obs-btn obs-btn-primary px-3 py-1 text-xs shrink-0" @click="addFromTemplate(tpl.rule)">
            添加
          </button>
        </div>
      </div>
    </ObsModal>

    <!-- Add Phrase Modal (Single / Batch Tabs) -->
    <ObsModal
      v-model="addPhraseVisible"
      title="添加动态广告短语"
      subtitle="命中后仅从标题剥离词条本身，由阶段 8 开关统一控制生效"
      max-width="520px"
    >
      <div class="-mx-5 -mt-5 mb-4 px-5 pt-3 border-b border-slate-800/80 flex gap-2 text-xs bg-[#0b0f17]">
        <button
          type="button"
          class="px-3 py-1.5 rounded-t-lg border border-b-0 cursor-pointer transition"
          :class="phraseModalTab === 'single' ? 'obs-tab-active font-semibold' : 'border-transparent text-slate-400 hover:text-slate-200'"
          @click="phraseModalTab = 'single'"
        >
          单条添加
        </button>
        <button
          type="button"
          class="px-3 py-1.5 rounded-t-lg border border-b-0 cursor-pointer transition"
          :class="phraseModalTab === 'batch' ? 'obs-tab-active font-semibold' : 'border-transparent text-slate-400 hover:text-slate-200'"
          @click="phraseModalTab = 'batch'"
        >
          批量粘贴
        </button>
      </div>

      <div v-if="phraseModalTab === 'single'" class="space-y-3 text-xs">
        <div>
          <label class="text-slate-400 block mb-1.5">广告短语 <span class="text-rose-400">*</span></label>
          <input
            v-model="singlePhraseText"
            type="text"
            placeholder="输入推广固定短语，如：高清完整版无删减"
            class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
            @keyup.enter="submitAddPhraseModal"
          />
        </div>
        <div>
          <label class="text-slate-400 block mb-1.5">备注（可选）</label>
          <input
            v-model="singlePhraseNote"
            type="text"
            placeholder="规则说明，如：视频站水印"
            class="obs-input w-full rounded-lg px-3 py-2"
            @keyup.enter="submitAddPhraseModal"
          />
        </div>
      </div>

      <div v-else class="space-y-3 text-xs">
        <div class="text-[11px] text-cyan-300">每行一条广告短语，自动去除首尾空白并跳过已存在词条</div>
        <textarea
          v-model="bulkPhraseText"
          rows="5"
          placeholder="例如：&#10;更多精彩资源&#10;永久地址发布页&#10;扫码加群"
          class="obs-input w-full rounded-lg p-3 font-mono-num"
        />
        <div>
          <label class="text-slate-400 block mb-1.5">批量备注（可选）</label>
          <input v-model="bulkPhraseNote" type="text" placeholder="默认：批量导入" class="obs-input w-full rounded-lg px-3 py-2" />
        </div>
      </div>

      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 rounded-lg text-xs" @click="addPhraseVisible = false">取消</button>
        <button type="button" class="obs-btn obs-btn-primary px-4 py-1.5 rounded-lg text-xs" @click="submitAddPhraseModal">确认添加</button>
      </template>
    </ObsModal>

    <!-- Add Regex Modal (Single / Batch Tabs) -->
    <ObsModal
      v-model="addRegexVisible"
      title="添加动态广告正则"
      subtitle="按顺序依次正则替换，由阶段 7 开关统一控制生效"
      max-width="580px"
    >
      <div class="-mx-5 -mt-5 mb-4 px-5 pt-3 border-b border-slate-800/80 flex gap-2 text-xs bg-[#0b0f17]">
        <button
          type="button"
          class="px-3 py-1.5 rounded-t-lg border border-b-0 cursor-pointer transition"
          :class="regexModalTab === 'single' ? 'obs-tab-active font-semibold' : 'border-transparent text-slate-400 hover:text-slate-200'"
          @click="regexModalTab = 'single'"
        >
          单条添加
        </button>
        <button
          type="button"
          class="px-3 py-1.5 rounded-t-lg border border-b-0 cursor-pointer transition"
          :class="regexModalTab === 'batch' ? 'obs-tab-active font-semibold' : 'border-transparent text-slate-400 hover:text-slate-200'"
          @click="regexModalTab = 'batch'"
        >
          批量粘贴
        </button>
      </div>

      <div v-if="regexModalTab === 'single'" class="space-y-3 text-xs">
        <div class="flex flex-wrap items-center gap-1.5 text-[11px]">
          <span class="text-slate-500">快捷模板:</span>
          <button
            type="button"
            class="obs-btn-ghost px-2 py-0.5 rounded cursor-pointer"
            @click="fillRegexPreset('【[^】]{0,24}(广告|推广|下载|首发)】', ' ', '方括号广告标签')"
          >
            + 方括号推广
          </button>
          <button
            type="button"
            class="obs-btn-ghost px-2 py-0.5 rounded cursor-pointer"
            @click="fillRegexPreset('\\[(?:QQ|WX|TG)[^\\]]{2,18}\\]', '', '社群号推广')"
          >
            + QQ/微信群
          </button>
          <button
            type="button"
            class="obs-btn-ghost px-2 py-0.5 rounded cursor-pointer"
            @click="fillRegexPreset('\\((?:访问|备用)[:：]?https?://[^)]+\\)', '', '备用网址括号')"
          >
            + 网址后缀
          </button>
        </div>

        <div>
          <label class="text-slate-400 block mb-1.5">正则表达式 <span class="text-rose-400">*</span></label>
          <input
            v-model="singleRegexPattern"
            type="text"
            placeholder="正则表达式，如：【[^】]*广告】"
            class="obs-input w-full rounded-lg px-3 py-2 font-mono-num text-cyan-200"
            @keyup.enter="submitAddRegexModal"
          />
        </div>

        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <label class="text-slate-400 block mb-1.5">替换为</label>
            <input
              v-model="singleRegexReplacement"
              type="text"
              placeholder="空字符串或空格"
              class="obs-input w-full rounded-lg px-3 py-2 font-mono-num"
              @keyup.enter="submitAddRegexModal"
            />
          </div>
          <div>
            <label class="text-slate-400 block mb-1.5">备注（可选）</label>
            <input
              v-model="singleRegexNote"
              type="text"
              placeholder="规则说明..."
              class="obs-input w-full rounded-lg px-3 py-2"
              @keyup.enter="submitAddRegexModal"
            />
          </div>
        </div>
      </div>

      <div v-else class="space-y-3 text-xs">
        <div class="text-[11px] text-cyan-300">每行一条 Rust 正则表达式，自动去重后追加至列表顶部</div>
        <textarea
          v-model="bulkRegexText"
          rows="5"
          placeholder="例如：&#10;【[^】]{0,24}推广】&#10;\[QQ群[:：]?\d{5,11}\]"
          class="obs-input w-full rounded-lg p-3 font-mono-num text-cyan-200"
        />
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <label class="text-slate-400 block mb-1.5">批量替换为</label>
            <input v-model="bulkRegexReplacement" type="text" placeholder="默认空格" class="obs-input w-full rounded-lg px-3 py-2 font-mono-num" />
          </div>
          <div>
            <label class="text-slate-400 block mb-1.5">批量备注（可选）</label>
            <input v-model="bulkRegexNote" type="text" placeholder="默认：批量正则导入" class="obs-input w-full rounded-lg px-3 py-2" />
          </div>
        </div>
      </div>

      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 rounded-lg text-xs" @click="addRegexVisible = false">取消</button>
        <button type="button" class="obs-btn obs-btn-primary px-4 py-1.5 rounded-lg text-xs" @click="submitAddRegexModal">确认添加</button>
      </template>
    </ObsModal>

    <!-- Publish Modal -->
    <ObsModal v-model="publishVisible" title="发布规则集并热推送给 Indexer" max-width="480px">
      <div class="space-y-3 text-xs text-slate-300">
        <p>发布后将自动生成新的正式版本，并通过 IPC 热通知所有在线 Indexer 子进程立即切换新规则，无需重启写入队列。</p>
        <div>
          <label class="block text-[11px] text-slate-400 mb-1">本次发布变更说明（可选）</label>
          <input v-model="publishNote" type="text" placeholder="例如：收紧垃圾文件体积占比与推广前缀" class="obs-input px-3 py-1.5 text-xs" />
        </div>
      </div>
      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 text-xs" @click="publishVisible = false">取消</button>
        <button type="button" class="obs-btn obs-btn-primary px-4 py-1.5 text-xs" :disabled="publishing" @click="confirmPublish">
          {{ publishing ? "发布并推送中…" : "确认发布生效" }}
        </button>
      </template>
    </ObsModal>

    <!-- Import Legacy JSON Modal -->
    <ObsModal v-model="importVisible" title="导入旧版 Filter JSON 配置" max-width="580px">
      <div class="space-y-3 text-xs">
        <label class="block text-[11px] text-slate-400">粘贴旧版 rules.json / filter 配置 JSON（自动转换为新格式草稿）</label>
        <textarea v-model="importRawJson" rows="10" class="obs-input p-2.5 text-xs font-mono" />
        <input v-model="importNote" type="text" placeholder="导入备注（可选）" class="obs-input px-3 py-1.5 text-xs" />
      </div>
      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3 py-1.5 text-xs" @click="importVisible = false">取消</button>
        <button type="button" class="obs-btn obs-btn-primary px-4 py-1.5 text-xs" :disabled="importBusy" @click="submitImportLegacy">
          {{ importBusy ? "解析导入中…" : "转换并存入草稿" }}
        </button>
      </template>
    </ObsModal>

    <!-- Reset Preset Modal -->
    <ObsModal v-model="resetVisible" title="重置为官方预设规则策略" max-width="460px">
      <div class="space-y-3 text-xs">
        <div>
          <label class="block text-[11px] text-slate-400 mb-1">选择预设方案</label>
          <ObsSelect v-model="resetPresetId" :options="presetSelectOptions" class="w-full" />
        </div>
        <label class="inline-flex items-center gap-2 text-xs text-slate-300 cursor-pointer">
          <input v-model="resetIncludeDict" type="checkbox" class="obs-checkbox" />
          同时恢复默认广告短语与广告清洗正则词典
        </label>
      </div>
      <template #footer>
        <button type="button" class="obs-btn obs-btn-ghost px-3 py-1.5 text-xs" @click="resetVisible = false">取消</button>
        <button type="button" class="obs-btn obs-btn-danger px-4 py-1.5 text-xs" :disabled="resetBusy" @click="submitResetPreset">
          {{ resetBusy ? "重置中…" : "覆盖为新草稿" }}
        </button>
      </template>
    </ObsModal>
  </div>
</template>
