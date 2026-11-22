<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import {
  AlertTriangle,
  ArrowDown,
  ArrowRight,
  ArrowUp,
  CheckCircle2,
  Copy,
  Download,
  Edit3,
  FileJson,
  Filter,
  FlaskConical,
  History,
  Play,
  Plus,
  RefreshCw,
  Save,
  Search,
  ShieldCheck,
  Sparkles,
  Trash2,
  Upload,
  Zap,
} from "lucide-vue-next";
import { ElMessage, ElMessageBox } from "element-plus";
import ByteSizeInput from "@/components/filter/ByteSizeInput.vue";
import RuleEditorDrawer from "@/components/filter/RuleEditorDrawer.vue";
import {
  discardFilterDraft,
  getFilterPresets,
  getFilterState,
  getFilterStats,
  importLegacyFilter,
  listFilterVersions,
  publishFilterDraft,
  resetFilterPreset,
  rollbackFilterVersion,
  runPlaygroundTest,
  samplePlaygroundTorrents,
  saveFilterDraft,
  validateFilterBundle,
  type AbComparisonItem,
  type AbComparisonReport,
  type AbDiffStatus,
  type CleanerOptions,
  type FilterBundle,
  type FilterCategory,
  type FilterPresetsResponse,
  type FilterStateResponse,
  type FilterStatsResponse,
  type FilterVersionDto,
  type PlaygroundInput,
  type PlaygroundItemResult,
  type PolicyRule,
  type RuleMatcher,
  type RuleTemplateInfo,
  type WorkerRejectedSampleDto,
} from "@/api/filter";

const loading = ref(true);
const saving = ref(false);
const publishing = ref(false);
const validating = ref(false);

const filterState = ref<FilterStateResponse | null>(null);
const presetsCatalog = ref<FilterPresetsResponse | null>(null);
const versions = ref<FilterVersionDto[]>([]);
const versionsLoading = ref(false);
const stats = ref<FilterStatsResponse | null>(null);
const statsLoading = ref(false);

const workingBundle = ref<FilterBundle>(emptyBundle());
const baselineJson = ref("");
const draftNote = ref("");

const activeMainTab = ref("policy");
const gatesExpanded = ref(true);

// 规则链筛选
const ruleSearch = ref("");
const ruleActionFilter = ref<"all" | "reject" | "allow">("all");
const ruleTypeFilter = ref<"all" | RuleMatcher["type"]>("all");

// 规则编辑抽屉
const drawerVisible = ref(false);
const drawerRule = ref<PolicyRule | null>(null);
const drawerIsNew = ref(false);
const drawerInsertIndex = ref<number | null>(null);

// 标题清洗词库搜索与新增
const phraseSearch = ref("");
const newAdPhrase = ref("");
const newAdPhraseNote = ref("");
const newAdRegex = ref("");
const newAdRegexReplacement = ref(" ");
const newAdRegexNote = ref("");

// 导入弹窗与转换提示
const legacyImportVisible = ref(false);
const legacyImportText = ref("");
const legacyImportNote = ref("");
const legacyImporting = ref(false);
const legacyFileRef = ref<HTMLInputElement | null>(null);
const lastConversionNotes = ref<string[]>([]);

const inspectVersionVisible = ref(false);
const inspectVersionData = ref<FilterVersionDto | null>(null);

// 演练场状态
const pgMode = ref<"ab_compare" | "draft" | "published">("ab_compare");
const singleTitle = ref("【高清影视】Ubuntu.24.04.LTS.Desktop.amd64[example.com]");
const singleInfoHash = ref("");
const singleAutoFile = ref(true);
const singleFiles = ref<Array<{ path: string; size: number }>>([
  { path: "Ubuntu-24.04/release.mkv", size: 104857600 },
]);
const singleRunning = ref(false);
const singleResult = ref<PlaygroundItemResult | null>(null);
const singleAbItem = ref<AbComparisonItem | null>(null);

const batchSamples = ref<PlaygroundInput[]>([]);
const batchKeyword = ref("");
const batchPasteVisible = ref(false);
const batchPasteText = ref("");
const batchSampling = ref(false);
const batchRunning = ref(false);
const batchAbReport = ref<AbComparisonReport | null>(null);
const batchDiffFilter = ref<"all" | "changed" | AbDiffStatus>("all");

// 统计与样本过滤
const rejectedSearch = ref("");
const rejectedCategoryFilter = ref<string>("all");

const CLEANER_STAGE_META: Array<{
  key: keyof Omit<CleanerOptions, "reject_if_empty_after_clean">;
  label: string;
  desc: string;
}> = [
  { key: "enable_nfkc", label: "1. NFKC 全半角归一化", desc: "全角字母数字、标点与异常间距归一" },
  {
    key: "enable_deobfuscate_brackets",
    label: "2. 括号内单字反混淆",
    desc: "还原 [ w w w . x y z ] 为 [www.xyz]",
  },
  {
    key: "enable_collapse_spaced_chars",
    label: "3. 连续单字拆分还原",
    desc: "还原 最 新 地 址 为 最新地址",
  },
  {
    key: "enable_strip_leading_site_prefix",
    label: "4. 开头站点发布前缀剥离",
    desc: "剥离开头 www.xxx.com - 及【xxx网】",
  },
  {
    key: "enable_strip_urls_and_domains",
    label: "5. URL 与裸域名清理",
    desc: "清理 http(s):// 及 1,284 个 IANA 顶级域名",
  },
  { key: "enable_ad_regexes", label: "6. 动态广告正则替换", desc: "按下方 ad_regexes 顺序正则替换" },
  { key: "enable_ad_phrases", label: "7. 动态广告短语剥离", desc: "按长度降序剥离推广固定短语" },
  { key: "enable_cleanup", label: "8. 空括号与首尾符号修剪", desc: "清空残留 []/() 与边缘分隔符" },
];

const hasUnsavedChanges = computed(() => {
  if (!baselineJson.value) return false;
  return JSON.stringify(workingBundle.value) !== baselineJson.value;
});

const hitCountByRuleId = computed(() => {
  const map = new Map<string, number>();
  for (const h of stats.value?.rule_hits ?? []) {
    map.set(h.rule_id, (map.get(h.rule_id) ?? 0) + h.hits);
  }
  return map;
});

const filteredRules = computed(() => {
  const q = ruleSearch.value.trim().toLowerCase();
  const act = ruleActionFilter.value;
  const typ = ruleTypeFilter.value;
  return workingBundle.value.policy.rules.filter((r) => {
    if (act !== "all" && r.action !== act) return false;
    if (typ !== "all" && r.matcher.type !== typ) return false;
    if (!q) return true;
    if (
      r.name.toLowerCase().includes(q) ||
      r.id.toLowerCase().includes(q) ||
      r.note.toLowerCase().includes(q) ||
      r.tags.some((t) => t.toLowerCase().includes(q))
    ) {
      return true;
    }
    const m = r.matcher;
    if (m.type === "keyword_group") return m.words.some((w) => w.toLowerCase().includes(q));
    if (m.type === "extension_rule" || m.type === "file_constraint" || m.type === "size_ratio") {
      return m.extensions.some((e) => e.toLowerCase().includes(q));
    }
    if (m.type === "substantive_content") {
      return m.junk_extensions.some((e) => e.toLowerCase().includes(q));
    }
    return false;
  });
});

const filteredAdPhrases = computed(() => {
  const q = phraseSearch.value.trim().toLowerCase();
  if (!q) return workingBundle.value.ad_phrases;
  return workingBundle.value.ad_phrases.filter(
    (r) => r.phrase.toLowerCase().includes(q) || r.note.toLowerCase().includes(q),
  );
});

const filteredAbItems = computed(() => {
  if (!batchAbReport.value) return [];
  const items = batchAbReport.value.items;
  if (batchDiffFilter.value === "all") return items;
  if (batchDiffFilter.value === "changed") {
    return items.filter(
      (i) =>
        i.diff_status === "newly_rejected" ||
        i.diff_status === "newly_accepted" ||
        i.diff_status === "refined_name_changed",
    );
  }
  return items.filter((i) => i.diff_status === batchDiffFilter.value);
});

const filteredRejectedSamples = computed(() => {
  if (!stats.value) return [];
  const q = rejectedSearch.value.trim().toLowerCase();
  const cat = rejectedCategoryFilter.value;
  return stats.value.recent_rejected.filter((row) => {
    if (cat !== "all" && row.hit.category !== cat) return false;
    if (!q) return true;
    return (
      row.raw_name.toLowerCase().includes(q) ||
      row.info_hash.toLowerCase().includes(q) ||
      row.hit.rule_label.toLowerCase().includes(q) ||
      row.hit.detail.toLowerCase().includes(q)
    );
  });
});

const allowRulesCount = computed(
  () => workingBundle.value.policy.rules.filter((r) => r.action === "allow").length,
);

const rejectRulesCount = computed(
  () => workingBundle.value.policy.rules.filter((r) => r.action === "reject").length,
);

function emptyBundle(): FilterBundle {
  return {
    version: 1,
    policy: {
      schema_version: 2,
      preset_id: null,
      gates: {
        reject_empty_name: true,
        min_name_length: { enabled: true, value: 4 },
        max_name_length: { enabled: true, value: 128 },
        reject_empty_files: true,
        reject_all_empty_files: true,
        max_empty_files: { enabled: false, value: 0 },
        max_file_count: { enabled: true, value: 2000 },
        max_file_name_length: { enabled: false, value: 255 },
        min_total_size: { enabled: true, value: 1 },
        max_total_size: { enabled: true, value: 2199023255552 },
        density_check: { enabled: true, min_file_count: 100, min_avg_file_size: 102400 },
      },
      cleaner: {
        enable_nfkc: true,
        enable_deobfuscate_brackets: true,
        enable_collapse_spaced_chars: true,
        enable_strip_leading_site_prefix: true,
        enable_strip_urls_and_domains: true,
        enable_ad_regexes: true,
        enable_ad_phrases: true,
        enable_cleanup: true,
        reject_if_empty_after_clean: true,
      },
      rules: [],
    },
    ad_phrases: [],
    ad_regexes: [],
  };
}

function cloneBundle(bundle: FilterBundle): FilterBundle {
  return JSON.parse(JSON.stringify(bundle)) as FilterBundle;
}

function formatBytes(bytes: number): string {
  if (!bytes || Number.isNaN(bytes)) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let val = bytes / 1024;
  let idx = 0;
  while (val >= 1024 && idx < units.length - 1) {
    val /= 1024;
    idx += 1;
  }
  return `${val.toFixed(val >= 100 ? 0 : 2)} ${units[idx]}`;
}

function formatTs(ts: number | null | undefined): string {
  if (!ts) return "-";
  return new Date(ts * 1000).toLocaleString("zh-CN", { hour12: false });
}

function categoryLabel(cat: FilterCategory): string {
  const map: Record<FilterCategory, string> = {
    empty_name: "空标题门禁",
    title_length: "标题长度门禁",
    empty_files: "空文件列表门禁",
    all_empty_files: "全零字节门禁",
    max_empty_files: "空文件数门禁",
    max_file_count: "文件总数门禁",
    max_file_name_length: "单文件名长度门禁",
    min_total_size: "最小体积门禁",
    max_total_size: "最大体积门禁",
    file_density: "碎文件密度门禁",
    keyword_group: "关键词词组",
    extension_rule: "扩展名后缀",
    title_pattern_preset: "垃圾标题特征",
    file_constraint: "文件约束",
    substantive_content: "实质内容底线",
    size_ratio: "体积占比上限",
    custom_regex: "自定义正则",
    composite: "组合逻辑规则",
    empty_refined_name: "清洗后为空",
  };
  return map[cat] ?? cat;
}

function matcherTypeLabel(t: RuleMatcher["type"]): string {
  const map: Record<RuleMatcher["type"], string> = {
    keyword_group: "关键词词组",
    extension_rule: "扩展名后缀",
    title_pattern_preset: "标题特征开关",
    file_constraint: "文件体积/长度",
    substantive_content: "有效实质内容",
    size_ratio: "体积占比",
    custom_regex: "自定义正则",
    composite: "复合条件",
  };
  return map[t];
}

function summarizeMatcher(m: RuleMatcher): string {
  switch (m.type) {
    case "keyword_group": {
      const targetName =
        m.target === "raw_name"
          ? "原始标题"
          : m.target === "refined_name"
            ? "清洗后标题"
            : m.target === "any_file_path"
              ? "文件路径"
              : "单文件名";
      const modeName = m.match_mode === "word_boundary" ? "整词边界" : "包含子串";
      const preview = m.words.slice(0, 6).join(", ");
      const more = m.words.length > 6 ? ` 等共 ${m.words.length} 词` : ` (${m.words.length} 词)`;
      return `${targetName} · ${modeName}: ${preview || "(空)"}${m.words.length > 0 ? more : ""}`;
    }
    case "extension_rule": {
      const scopeName =
        m.scope === "torrent_name"
          ? "标题后缀"
          : m.scope === "any_file"
            ? "含任意该后缀文件"
            : "全部为此后缀";
      const preview = m.extensions
        .slice(0, 8)
        .map((e) => `.${e}`)
        .join(" ");
      const more =
        m.extensions.length > 8
          ? ` 等共 ${m.extensions.length} 个`
          : ` (${m.extensions.length} 个)`;
      return `${scopeName}: ${preview || "(未选)"}${m.extensions.length > 0 ? more : ""}`;
    }
    case "title_pattern_preset":
      return `已勾选 ${m.patterns.length} 项结构特征（西里尔阈值 ${m.min_cyrillic_chars}）`;
    case "file_constraint": {
      const modeStr = m.mode === "at_least_one_matched" ? "至少 1 个主文件达标" : "每个匹配文件都达标";
      const parts: string[] = [];
      if (m.min_file_size) parts.push(`>= ${formatBytes(m.min_file_size)}`);
      if (m.max_file_size) parts.push(`<= ${formatBytes(m.max_file_size)}`);
      if (m.min_file_name_length) parts.push(`文件名>=${m.min_file_name_length}字符`);
      if (m.max_file_name_length) parts.push(`文件名<=${m.max_file_name_length}字符`);
      const extPreview = m.extensions
        .slice(0, 5)
        .map((e) => `.${e}`)
        .join(" ");
      return `${extPreview || "自定义正则"} · ${modeStr} · ${parts.join(" 且 ") || "命中即生效"}`;
    }
    case "substantive_content":
      return `${m.include_system_junk ? "排除系统垃圾 + " : ""}排除 ${m.junk_extensions.length} 类后缀后：至少 ${m.min_files} 个文件且合计 >= ${formatBytes(m.min_total_size)}`;
    case "size_ratio": {
      const extPreview = m.extensions
        .slice(0, 5)
        .map((e) => `.${e}`)
        .join(" ");
      return `${extPreview}${m.include_split_archives ? " (+分卷压缩)" : ""} · 占比阈值 ${(m.max_ratio * 100).toFixed(0)}%`;
    }
    case "custom_regex":
      return `${m.patterns.length} 条 Regex (${m.match_all ? "AND 全部匹配" : "OR 任一匹配"})`;
    case "composite":
      return `${m.logic === "all" ? "ALL 全部满足" : "ANY 任一满足"} · ${m.conditions.length} 个子条件`;
  }
}

function abDiffLabel(st: AbDiffStatus): string {
  const map: Record<AbDiffStatus, string> = {
    unchanged_accepted: "维持放行",
    unchanged_rejected: "维持拦截",
    newly_rejected: "新增拦截",
    newly_accepted: "解除拦截",
    refined_name_changed: "清洗标题变化",
  };
  return map[st];
}

function abDiffTagType(
  st: AbDiffStatus,
): "success" | "danger" | "warning" | "info" | "primary" {
  switch (st) {
    case "newly_rejected":
      return "danger";
    case "newly_accepted":
      return "success";
    case "refined_name_changed":
      return "warning";
    case "unchanged_rejected":
      return "info";
    default:
      return "primary";
  }
}

async function fetchState() {
  loading.value = true;
  try {
    const res = await getFilterState();
    filterState.value = res;
    const base = res.draft?.bundle ?? res.published?.bundle ?? emptyBundle();
    workingBundle.value = cloneBundle(base);
    baselineJson.value = JSON.stringify(workingBundle.value);
    draftNote.value = res.draft?.note ?? "";
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "加载过滤规则失败");
  } finally {
    loading.value = false;
  }
}

async function fetchPresets() {
  try {
    presetsCatalog.value = await getFilterPresets();
  } catch {
    presetsCatalog.value = null;
  }
}

async function fetchVersions() {
  versionsLoading.value = true;
  try {
    versions.value = await listFilterVersions(30);
  } catch {
    versions.value = [];
  } finally {
    versionsLoading.value = false;
  }
}

async function fetchStats() {
  statsLoading.value = true;
  try {
    stats.value = await getFilterStats();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "加载拦截统计失败");
  } finally {
    statsLoading.value = false;
  }
}

async function refreshAll() {
  await Promise.all([fetchState(), fetchPresets(), fetchVersions(), fetchStats()]);
}

async function handleValidate() {
  validating.value = true;
  try {
    const res = await validateFilterBundle(workingBundle.value);
    ElMessage.success(
      `策略校验通过：门禁 ${res.counts.enabled_gates} 项 / 规则 ${res.counts.enabled_rules}/${res.counts.total_rules} 条（放行 ${res.counts.allow_rules} · 拦截 ${res.counts.reject_rules}）/ 清洗词库 ${res.counts.ad_phrases + res.counts.ad_regexes}`,
    );
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "规则校验失败");
  } finally {
    validating.value = false;
  }
}

async function handleSaveDraft(): Promise<boolean> {
  saving.value = true;
  try {
    const saved = await saveFilterDraft(
      workingBundle.value,
      draftNote.value.trim() || "面板策略草稿更新",
    );
    workingBundle.value = cloneBundle(saved.bundle);
    baselineJson.value = JSON.stringify(workingBundle.value);
    await fetchState();
    await fetchVersions();
    ElMessage.success(`已保存草稿 v${saved.version}`);
    return true;
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "保存草稿失败");
    return false;
  } finally {
    saving.value = false;
  }
}

async function handleDiscardDraft() {
  try {
    await ElMessageBox.confirm(
      "确认放弃当前草稿并将编辑区恢复为线上正在生效的版本吗？",
      "放弃草稿确认",
      { type: "warning", confirmButtonText: "确认放弃", cancelButtonText: "取消" },
    );
  } catch {
    return;
  }
  saving.value = true;
  try {
    await discardFilterDraft();
    lastConversionNotes.value = [];
    await refreshAll();
    ElMessage.success("已放弃草稿并恢复为线上版本");
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "放弃草稿失败");
  } finally {
    saving.value = false;
  }
}

async function handlePublish() {
  if (!filterState.value?.has_draft && !hasUnsavedChanges.value) {
    ElMessage.info("当前内容与线上版本一致，暂无待发布变更");
    return;
  }
  try {
    await ElMessageBox.confirm(
      "发布后将覆盖当前线上生效版本，并通过本地 IPC 向所有在线 Indexer 无停机广播热更新，确认发布吗？",
      "发布并热更新",
      { type: "warning", confirmButtonText: "立即发布并热更新", cancelButtonText: "取消" },
    );
  } catch {
    return;
  }

  publishing.value = true;
  try {
    if (hasUnsavedChanges.value || !filterState.value?.has_draft) {
      const ok = await handleSaveDraft();
      if (!ok) return;
    }
    const res = await publishFilterDraft(draftNote.value.trim() || undefined);
    const failed = res.reload_results.filter((r) => !r.ok);
    if (failed.length > 0) {
      ElMessage.warning(
        `已发布 v${res.published.version}，但部分 Indexer 热更新失败：${failed.map((f) => `${f.instance_id}(${f.message})`).join("；")}`,
      );
    } else if (res.reload_results.length > 0) {
      ElMessage.success(
        `已发布 v${res.published.version}，并热更新 ${res.reload_results.length} 个在线 Indexer（无重启生效）`,
      );
    } else {
      ElMessage.success(`已发布线上版本 v${res.published.version}（当前无在线 Indexer）`);
    }
    await refreshAll();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "发布规则失败");
  } finally {
    publishing.value = false;
  }
}

function openCreateRuleDrawer(tpl?: RuleTemplateInfo) {
  drawerIsNew.value = true;
  drawerInsertIndex.value = null;
  if (tpl) {
    const cloned = JSON.parse(JSON.stringify(tpl.rule)) as PolicyRule;
    cloned.id = `${tpl.id.replace(/^tpl-/, "")}-${Date.now()}`;
    drawerRule.value = cloned;
  } else {
    drawerRule.value = null;
  }
  drawerVisible.value = true;
}

function openEditRuleDrawer(rule: PolicyRule) {
  drawerIsNew.value = false;
  drawerInsertIndex.value = workingBundle.value.policy.rules.indexOf(rule);
  drawerRule.value = JSON.parse(JSON.stringify(rule)) as PolicyRule;
  drawerVisible.value = true;
}

function handleSaveRuleFromDrawer(saved: PolicyRule) {
  if (drawerIsNew.value) {
    if (drawerInsertIndex.value !== null && drawerInsertIndex.value >= 0) {
      workingBundle.value.policy.rules.splice(drawerInsertIndex.value, 0, saved);
    } else if (saved.action === "allow") {
      // 白名单默认插入最顶部以高于一般拦截规则，安全后缀之后用户可微调顺序
      workingBundle.value.policy.rules.unshift(saved);
    } else {
      workingBundle.value.policy.rules.push(saved);
    }
    ElMessage.success(`已添加规则「${saved.name}」（记得点击保存/发布生效）`);
  } else if (drawerInsertIndex.value !== null && drawerInsertIndex.value >= 0) {
    workingBundle.value.policy.rules[drawerInsertIndex.value] = saved;
    ElMessage.success(`已更新规则「${saved.name}」`);
  }
}

function duplicateRule(rule: PolicyRule) {
  const idx = workingBundle.value.policy.rules.indexOf(rule);
  if (idx < 0) return;
  const copy = JSON.parse(JSON.stringify(rule)) as PolicyRule;
  copy.id = `${rule.id}-copy-${Date.now()}`;
  copy.name = `${rule.name} (副本)`;
  workingBundle.value.policy.rules.splice(idx + 1, 0, copy);
}

function moveRule(rule: PolicyRule, delta: -1 | 1) {
  const idx = workingBundle.value.policy.rules.indexOf(rule);
  const nextIdx = idx + delta;
  if (idx < 0 || nextIdx < 0 || nextIdx >= workingBundle.value.policy.rules.length) return;
  const arr = workingBundle.value.policy.rules;
  const [item] = arr.splice(idx, 1);
  arr.splice(nextIdx, 0, item);
}

function removeRule(rule: PolicyRule) {
  workingBundle.value.policy.rules = workingBundle.value.policy.rules.filter((x) => x !== rule);
}

function locateRuleFromSample(sample: WorkerRejectedSampleDto) {
  const target = workingBundle.value.policy.rules.find((r) => r.id === sample.hit.rule_id);
  activeMainTab.value = "policy";
  if (target) {
    openEditRuleDrawer(target);
  } else {
    ElMessage.info(
      `命中规则 ${sample.hit.rule_id}（${sample.hit.rule_label}）属于基础门禁或旧版本规则，可在上方「基础门禁」或演练场调试`,
    );
  }
}

function quickAllowFromSample(sample: WorkerRejectedSampleDto) {
  activeMainTab.value = "policy";
  drawerIsNew.value = true;
  drawerInsertIndex.value = 0;
  drawerRule.value = {
    id: `allow-sample-${Date.now()}`,
    name: `白名单放行：${sample.raw_name.slice(0, 28)}`,
    enabled: true,
    action: "allow",
    tags: ["快速白名单"],
    note: `基于被拦截样本 ${sample.info_hash.slice(0, 12)} 快速生成`,
    matcher: {
      type: "keyword_group",
      target: "raw_name",
      match_mode: "word_boundary",
      case_sensitive: false,
      words: [sample.raw_name],
    },
  };
  drawerVisible.value = true;
}

function addAdPhrase() {
  const val = newAdPhrase.value.trim();
  if (!val) return;
  workingBundle.value.ad_phrases.unshift({
    id: `ad-phrase-${Date.now()}`,
    phrase: val,
    enabled: true,
    note: newAdPhraseNote.value.trim(),
  });
  newAdPhrase.value = "";
  newAdPhraseNote.value = "";
}

function addAdRegex() {
  const pat = newAdRegex.value.trim();
  if (!pat) return;
  workingBundle.value.ad_regexes.unshift({
    id: `ad-re-${Date.now()}`,
    pattern: pat,
    replacement: newAdRegexReplacement.value ?? " ",
    enabled: true,
    note: newAdRegexNote.value.trim(),
  });
  newAdRegex.value = "";
  newAdRegexReplacement.value = " ";
  newAdRegexNote.value = "";
}

function openLegacyImport() {
  legacyImportText.value = "";
  legacyImportNote.value = "";
  legacyImportVisible.value = true;
}

function handleLegacyFileSelected(ev: Event) {
  const input = ev.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;
  const reader = new FileReader();
  reader.onload = () => {
    legacyImportText.value = String(reader.result ?? "");
  };
  reader.readAsText(file);
  input.value = "";
}

async function submitLegacyImport() {
  if (!legacyImportText.value.trim()) {
    ElMessage.warning("请粘贴 JSON 或选择配置文件");
    return;
  }
  legacyImporting.value = true;
  try {
    const res = await importLegacyFilter(
      legacyImportText.value,
      true,
      legacyImportNote.value.trim() || "导入并转换为 Hameta v2 策略",
    );
    workingBundle.value = cloneBundle(res.bundle);
    baselineJson.value = JSON.stringify(workingBundle.value);
    lastConversionNotes.value = res.conversion_notes;
    legacyImportVisible.value = false;
    await fetchState();
    await fetchVersions();
    ElMessage.success(
      `导入成功（${res.source_format}）：已写入草稿 v${res.saved_draft?.version ?? ""}（共 ${res.bundle.policy.rules.length} 条策略规则）`,
    );
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "导入 JSON 失败");
  } finally {
    legacyImporting.value = false;
  }
}

function downloadJson(data: unknown, filename: string) {
  const blob = new Blob([JSON.stringify(data, null, 2)], {
    type: "application/json;charset=utf-8",
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

function handleExportV2Bundle() {
  const ver = workingBundle.value.version || 0;
  downloadJson(workingBundle.value, `hameta-filter-v2-bundle-v${ver}.json`);
}

async function handleApplyPolicyPreset(presetId: string) {
  const info = presetsCatalog.value?.policy_presets.find((p) => p.id === presetId);
  const name = info?.name ?? presetId;
  try {
    await ElMessageBox.confirm(
      `将切换并加载预设「${name}」（${info?.summary ?? ""}）到草稿，原有草稿内容将被替换，是否继续？`,
      "应用内置场景预设",
      {
        confirmButtonText: "加载预设到草稿",
        cancelButtonText: "取消",
        type: "warning",
      },
    );
  } catch {
    return;
  }
  saving.value = true;
  try {
    const saved = await resetFilterPreset(presetId, true, `切换场景预设: ${name}`);
    workingBundle.value = cloneBundle(saved.bundle);
    baselineJson.value = JSON.stringify(workingBundle.value);
    lastConversionNotes.value = [];
    await refreshAll();
    ElMessage.success(`已将预设「${name}」加载至草稿 v${saved.version}`);
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "应用预设失败");
  } finally {
    saving.value = false;
  }
}

async function handleRollback(targetVersion: number) {
  try {
    await ElMessageBox.confirm(
      `确认将规则直接回滚到历史版本 v${targetVersion} 并立刻向所有在线 Indexer 广播热更新吗？`,
      "回滚确认",
      { type: "warning", confirmButtonText: "确认回滚并热更新", cancelButtonText: "取消" },
    );
  } catch {
    return;
  }
  publishing.value = true;
  try {
    const res = await rollbackFilterVersion(targetVersion);
    ElMessage.success(
      `已回滚生成线上版本 v${res.published.version}，通知 ${res.reload_results.length} 个 Indexer 节点`,
    );
    await refreshAll();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "回滚失败");
  } finally {
    publishing.value = false;
  }
}

// 演练场逻辑
async function runSingleTest() {
  if (!singleTitle.value.trim()) {
    ElMessage.warning("请输入测试标题");
    return;
  }
  singleRunning.value = true;
  try {
    const res = await runPlaygroundTest({
      mode: pgMode.value,
      draft_bundle: workingBundle.value,
      auto_placeholder_file: singleAutoFile.value,
      samples: [
        {
          info_hash: singleInfoHash.value.trim() || undefined,
          name: singleTitle.value,
          files: singleAutoFile.value ? [] : singleFiles.value,
        },
      ],
    });
    singleResult.value = res.results[0] ?? null;
    singleAbItem.value = res.ab_report?.items[0] ?? null;
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "单条试跑失败");
  } finally {
    singleRunning.value = false;
  }
}

async function loadSamplesFrom(source: "manticore_latest" | "manticore_random" | "rejected_recent") {
  batchSampling.value = true;
  try {
    const res = await samplePlaygroundTorrents({
      source,
      limit: 30,
      keyword: batchKeyword.value.trim() || undefined,
    });
    batchSamples.value = res.samples;
    batchAbReport.value = null;
    ElMessage.success(`已加载 ${res.samples.length} 条回归样本`);
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "抽样加载失败");
  } finally {
    batchSampling.value = false;
  }
}

function applyPasteSamples() {
  const lines = batchPasteText.value
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean)
    .slice(0, 200);
  if (lines.length === 0) {
    ElMessage.warning("请输入至少一行种子标题");
    return;
  }
  batchSamples.value = lines.map((name) => ({ name }));
  batchPasteVisible.value = false;
  batchAbReport.value = null;
  ElMessage.success(`已加载 ${lines.length} 条粘贴样本`);
}

async function runBatchAbCompare() {
  if (batchSamples.value.length === 0) {
    ElMessage.warning("请先从 Manticore / 拦截缓存抽样或粘贴测试标题");
    return;
  }
  batchRunning.value = true;
  try {
    const res = await runPlaygroundTest({
      mode: "ab_compare",
      draft_bundle: workingBundle.value,
      auto_placeholder_file: true,
      samples: batchSamples.value.slice(0, 200),
    });
    batchAbReport.value = res.ab_report;
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "批量 A/B 对比失败");
  } finally {
    batchRunning.value = false;
  }
}

function sendSampleToPlayground(rawName: string, infoHash?: string) {
  singleTitle.value = rawName;
  singleInfoHash.value = infoHash ?? "";
  activeMainTab.value = "playground";
  void runSingleTest();
}

onMounted(() => {
  void refreshAll();
});
</script>

<template>
  <div class="max-w-7xl mx-auto space-y-5" v-loading="loading">
    <!-- 顶部状态与版本发布控制栏 -->
    <div class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-3.5">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <div class="flex flex-wrap items-center gap-2">
            <h2 class="text-base font-semibold text-slate-900">过滤清洗工作台 (Filter Policy v2)</h2>
            <el-tag v-if="filterState?.published" type="success" size="small" effect="light">
              线上已发布: v{{ filterState.published.version }}
            </el-tag>
            <el-tag v-if="filterState?.has_draft" type="warning" size="small" effect="light">
              草稿: v{{ filterState.draft?.version }}
            </el-tag>
            <el-tag v-if="workingBundle.policy.preset_id" type="info" size="small" effect="plain">
              基线: {{ workingBundle.policy.preset_id }}
            </el-tag>
            <el-tag v-if="hasUnsavedChanges" type="danger" size="small" effect="plain">
              编辑器有未保存变更
            </el-tag>
          </div>
          <p class="text-xs text-slate-500 mt-1">
            三段式流水线：基础门禁 (Gates) → 8 步可控标题清洗 (Cleaner) → Allow/Reject 有序策略规则链 (`ArcSwap` 无停机热更新)
          </p>
        </div>

        <div class="flex flex-wrap items-center gap-2">
          <!-- 切换内置场景预设 -->
          <el-dropdown @command="(id: string) => handleApplyPolicyPreset(id)">
            <el-button size="small">
              <Sparkles class="h-3.5 w-3.5 mr-1 text-indigo-600" />
              切换内置预设
            </el-button>
            <template #dropdown>
              <el-dropdown-menu>
                <el-dropdown-item
                  v-for="preset in presetsCatalog?.policy_presets ?? []"
                  :key="preset.id"
                  :command="preset.id"
                >
                  <div class="py-0.5 max-w-xs">
                    <div class="font-medium text-slate-800 flex items-center gap-2">
                      <span>{{ preset.name }}</span>
                      <el-tag size="small" type="info" effect="plain">{{ preset.badge }}</el-tag>
                    </div>
                    <div class="text-[11px] text-slate-500 whitespace-normal">
                      {{ preset.summary }}（{{ preset.rule_count }} 条规则）
                    </div>
                  </div>
                </el-dropdown-item>
              </el-dropdown-menu>
            </template>
          </el-dropdown>

          <el-button size="small" @click="openLegacyImport">
            <Upload class="h-3.5 w-3.5 mr-1" />
            导入旧版 / 策略 JSON
          </el-button>

          <el-button size="small" @click="handleExportV2Bundle">
            <Download class="h-3.5 w-3.5 mr-1" />
            导出 Hameta v2 JSON
          </el-button>

          <el-button size="small" :loading="validating" @click="handleValidate">
            <CheckCircle2 class="h-3.5 w-3.5 mr-1" />
            校验规则
          </el-button>

          <el-button
            v-if="filterState?.has_draft"
            size="small"
            type="info"
            plain
            :loading="saving"
            @click="handleDiscardDraft"
          >
            放弃草稿
          </el-button>

          <el-button size="small" type="warning" :loading="saving" @click="handleSaveDraft">
            <Save class="h-3.5 w-3.5 mr-1" />
            保存草稿
          </el-button>

          <el-button size="small" type="primary" :loading="publishing" @click="handlePublish">
            <Zap class="h-3.5 w-3.5 mr-1" />
            发布并热更新 Indexer
          </el-button>
        </div>
      </div>

      <!-- 智能转换提示条 -->
      <el-alert
        v-if="lastConversionNotes.length > 0"
        type="success"
        show-icon
        closable
        @close="lastConversionNotes = []"
      >
        <template #title>
          <span class="text-xs font-medium">
            智能转换完成：{{ lastConversionNotes.join(" · ") }}
          </span>
        </template>
      </el-alert>

      <!-- 在线 Indexer 规则同步状态条 -->
      <div class="flex flex-wrap items-center justify-between gap-3 pt-2.5 border-t border-slate-100 text-xs">
        <div class="flex flex-wrap items-center gap-2">
          <span class="text-slate-500">在线 Indexer 规则状态:</span>
          <span v-if="!filterState?.online_indexers.length" class="text-slate-400">
            暂无 Indexer 实例
          </span>
          <el-tag
            v-for="idx in filterState?.online_indexers ?? []"
            :key="idx.id"
            size="small"
            :type="
              idx.runtime_state === 'running'
                ? idx.active_rule_version === filterState?.published?.version
                  ? 'success'
                  : 'warning'
                : 'info'
            "
            effect="plain"
          >
            {{ idx.id }} ({{ idx.runtime_state }}) ·
            {{ idx.active_rule_version ? `v${idx.active_rule_version}` : "未上报" }}
          </el-tag>
        </div>

        <el-input
          v-model="draftNote"
          size="small"
          placeholder="本次草稿/发布说明（可选）"
          style="width: 260px"
        />
      </div>
    </div>

    <!-- 主工作区 Tabs -->
    <el-tabs v-model="activeMainTab" type="border-card">
      <!-- Tab 1: 策略编排（基础门禁 + 有序规则链） -->
      <el-tab-pane name="policy">
        <template #label>
          <span class="inline-flex items-center gap-1.5">
            <Filter class="h-3.5 w-3.5" />
            策略编排 ({{ workingBundle.policy.rules.length }} 规则)
          </span>
        </template>

        <div class="space-y-5">
          <!-- ① 基础门禁 (Gates) -->
          <div class="rounded-xl border border-slate-200 bg-slate-50/50">
            <div
              class="flex items-center justify-between px-4 py-3 cursor-pointer select-none"
              @click="gatesExpanded = !gatesExpanded"
            >
              <div class="flex items-center gap-2">
                <span class="text-sm font-semibold text-slate-900">① 基础门禁 (Gates)</span>
                <span class="text-xs text-slate-500">
                  在清洗与复杂规则之前执行 O(1) 快速剪枝，支持显式开关与可视化容量单位
                </span>
              </div>
              <el-button size="small" link type="primary">
                {{ gatesExpanded ? "收起门禁面板" : "展开门禁配置" }}
              </el-button>
            </div>

            <div
              v-show="gatesExpanded"
              class="px-4 pb-4 pt-2 border-t border-slate-200 grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3 text-xs"
            >
              <!-- 标题门禁 -->
              <div class="rounded-lg border border-slate-200 bg-white p-3 space-y-2.5">
                <div class="font-semibold text-slate-800">标题合规检查</div>
                <div class="flex items-center justify-between">
                  <span class="text-slate-600">拒绝空标题</span>
                  <el-switch v-model="workingBundle.policy.gates.reject_empty_name" size="small" />
                </div>
                <div class="flex items-center justify-between gap-2">
                  <el-switch
                    v-model="workingBundle.policy.gates.min_name_length.enabled"
                    size="small"
                    active-text="标题最短字符"
                  />
                  <el-input-number
                    v-if="workingBundle.policy.gates.min_name_length.enabled"
                    v-model="workingBundle.policy.gates.min_name_length.value"
                    :min="1"
                    size="small"
                    style="width: 110px"
                  />
                </div>
                <div class="flex items-center justify-between gap-2">
                  <el-switch
                    v-model="workingBundle.policy.gates.max_name_length.enabled"
                    size="small"
                    active-text="标题最长字符"
                  />
                  <el-input-number
                    v-if="workingBundle.policy.gates.max_name_length.enabled"
                    v-model="workingBundle.policy.gates.max_name_length.value"
                    :min="1"
                    size="small"
                    style="width: 110px"
                  />
                </div>
              </div>

              <!-- 文件数量与空文件门禁 -->
              <div class="rounded-lg border border-slate-200 bg-white p-3 space-y-2.5">
                <div class="font-semibold text-slate-800">文件清单与空文件检查</div>
                <div class="flex items-center justify-between">
                  <span class="text-slate-600">拒绝空文件列表 (file_count = 0)</span>
                  <el-switch v-model="workingBundle.policy.gates.reject_empty_files" size="small" />
                </div>
                <div class="flex items-center justify-between">
                  <span class="text-slate-600">拒绝全部 0 字节种子</span>
                  <el-switch
                    v-model="workingBundle.policy.gates.reject_all_empty_files"
                    size="small"
                  />
                </div>
                <div class="flex items-center justify-between gap-2">
                  <el-switch
                    v-model="workingBundle.policy.gates.max_file_count.enabled"
                    size="small"
                    active-text="最大文件数"
                  />
                  <el-input-number
                    v-if="workingBundle.policy.gates.max_file_count.enabled"
                    v-model="workingBundle.policy.gates.max_file_count.value"
                    :min="1"
                    size="small"
                    style="width: 110px"
                  />
                </div>
                <div class="flex items-center justify-between gap-2">
                  <el-switch
                    v-model="workingBundle.policy.gates.max_empty_files.enabled"
                    size="small"
                    active-text="最大空文件数"
                  />
                  <el-input-number
                    v-if="workingBundle.policy.gates.max_empty_files.enabled"
                    v-model="workingBundle.policy.gates.max_empty_files.value"
                    :min="0"
                    size="small"
                    style="width: 110px"
                  />
                </div>
              </div>

              <!-- 体积与碎文件密度门禁 -->
              <div class="rounded-lg border border-slate-200 bg-white p-3 space-y-2.5">
                <div class="font-semibold text-slate-800">整种体积与密度防护</div>
                <div class="flex items-center justify-between gap-2">
                  <el-switch
                    v-model="workingBundle.policy.gates.min_total_size.enabled"
                    size="small"
                    active-text="最小体积"
                  />
                  <ByteSizeInput
                    v-if="workingBundle.policy.gates.min_total_size.enabled"
                    v-model="workingBundle.policy.gates.min_total_size.value"
                    :min="1"
                  />
                </div>
                <div class="flex items-center justify-between gap-2">
                  <el-switch
                    v-model="workingBundle.policy.gates.max_total_size.enabled"
                    size="small"
                    active-text="最大体积"
                  />
                  <ByteSizeInput
                    v-if="workingBundle.policy.gates.max_total_size.enabled"
                    v-model="workingBundle.policy.gates.max_total_size.value"
                    :min="1"
                  />
                </div>
                <div class="flex items-center justify-between gap-2">
                  <el-switch
                    v-model="workingBundle.policy.gates.max_file_name_length.enabled"
                    size="small"
                    active-text="单文件名限长"
                  />
                  <el-input-number
                    v-if="workingBundle.policy.gates.max_file_name_length.enabled"
                    v-model="workingBundle.policy.gates.max_file_name_length.value"
                    :min="1"
                    size="small"
                    style="width: 110px"
                  />
                </div>
                <div class="pt-1 border-t border-slate-100 space-y-1.5">
                  <div class="flex items-center justify-between">
                    <span class="text-slate-600">超多小碎文件密度拦截</span>
                    <el-switch
                      v-model="workingBundle.policy.gates.density_check.enabled"
                      size="small"
                    />
                  </div>
                  <div
                    v-if="workingBundle.policy.gates.density_check.enabled"
                    class="flex flex-wrap items-center gap-1.5"
                  >
                    <span class="text-slate-500">文件数 &gt;</span>
                    <el-input-number
                      v-model="workingBundle.policy.gates.density_check.min_file_count"
                      :min="1"
                      size="small"
                      controls-position="right"
                      style="width: 82px"
                    />
                    <span class="text-slate-500">且均重 &lt;</span>
                    <ByteSizeInput
                      v-model="workingBundle.policy.gates.density_check.min_avg_file_size"
                      :min="1"
                    />
                  </div>
                </div>
              </div>
            </div>
          </div>

          <!-- ② 有序策略规则链 -->
          <div class="space-y-3">
            <div class="flex flex-wrap items-center justify-between gap-3">
              <div class="flex flex-wrap items-center gap-2">
                <span class="text-sm font-semibold text-slate-900">② 有序策略规则链</span>
                <el-tag size="small" type="success" effect="light">
                  白名单 Allow: {{ allowRulesCount }}
                </el-tag>
                <el-tag size="small" type="danger" effect="light">
                  黑名单 Reject: {{ rejectRulesCount }}
                </el-tag>
                <span class="text-xs text-slate-500">
                  按列表顺序从上到下短路执行（推荐：致命木马先 Reject → 信任资源 Allow 放行 → 常规过滤）
                </span>
              </div>

              <div class="flex flex-wrap items-center gap-2">
                <el-input
                  v-model="ruleSearch"
                  size="small"
                  clearable
                  placeholder="搜索规则名 / 关键词 / 后缀..."
                  style="width: 200px"
                >
                  <template #prefix><Search class="h-3.5 w-3.5 text-slate-400" /></template>
                </el-input>

                <el-select v-model="ruleActionFilter" size="small" style="width: 115px">
                  <el-option label="全部动作" value="all" />
                  <el-option label="仅 Reject" value="reject" />
                  <el-option label="仅 Allow" value="allow" />
                </el-select>

                <el-select v-model="ruleTypeFilter" size="small" style="width: 135px">
                  <el-option label="全部类型" value="all" />
                  <el-option label="关键词词组" value="keyword_group" />
                  <el-option label="扩展名后缀" value="extension_rule" />
                  <el-option label="标题特征开关" value="title_pattern_preset" />
                  <el-option label="文件体积/长度" value="file_constraint" />
                  <el-option label="实质内容底线" value="substantive_content" />
                  <el-option label="体积占比上限" value="size_ratio" />
                  <el-option label="自定义正则" value="custom_regex" />
                  <el-option label="复合条件" value="composite" />
                </el-select>

                <el-dropdown @command="(tpl: RuleTemplateInfo) => openCreateRuleDrawer(tpl)">
                  <el-button size="small" type="success" plain>
                    <Sparkles class="h-3.5 w-3.5 mr-1" />
                    从内置模板添加
                  </el-button>
                  <template #dropdown>
                    <el-dropdown-menu>
                      <el-dropdown-item
                        v-for="tpl in presetsCatalog?.rule_templates ?? []"
                        :key="tpl.id"
                        :command="tpl"
                      >
                        <div class="max-w-sm">
                          <div class="font-medium text-xs text-slate-800">
                            [{{ tpl.group }}] {{ tpl.name }}
                          </div>
                          <div class="text-[11px] text-slate-500 truncate">
                            {{ tpl.description }}
                          </div>
                        </div>
                      </el-dropdown-item>
                    </el-dropdown-menu>
                  </template>
                </el-dropdown>

                <el-button size="small" type="primary" @click="openCreateRuleDrawer()">
                  <Plus class="h-3.5 w-3.5 mr-1" />
                  新建规则
                </el-button>
              </div>
            </div>

            <div
              v-if="filteredRules.length === 0"
              class="rounded-xl border border-dashed border-slate-300 py-12 text-center text-xs text-slate-500"
            >
              当前过滤条件下没有策略规则，可点击右上角「从内置模板添加」或「新建规则」。
            </div>

            <div class="space-y-2.5">
              <div
                v-for="rule in filteredRules"
                :key="rule.id"
                class="rounded-xl border p-3.5 transition-colors flex flex-wrap md:flex-nowrap items-start justify-between gap-3"
                :class="[
                  rule.enabled
                    ? rule.action === 'allow'
                      ? 'border-emerald-200 bg-emerald-50/20'
                      : 'border-slate-200 bg-white'
                    : 'border-slate-200 bg-slate-50 opacity-65',
                ]"
              >
                <div class="flex items-start gap-3 flex-1 min-w-0">
                  <!-- 优先级编号与上下移动 -->
                  <div class="flex flex-col items-center gap-0.5 pt-0.5">
                    <span class="text-[11px] font-mono font-semibold text-slate-400">
                      #{{ workingBundle.policy.rules.indexOf(rule) + 1 }}
                    </span>
                    <div class="flex items-center">
                      <el-button
                        size="small"
                        link
                        :disabled="workingBundle.policy.rules.indexOf(rule) === 0"
                        @click="moveRule(rule, -1)"
                      >
                        <ArrowUp class="h-3.5 w-3.5" />
                      </el-button>
                      <el-button
                        size="small"
                        link
                        :disabled="
                          workingBundle.policy.rules.indexOf(rule) ===
                          workingBundle.policy.rules.length - 1
                        "
                        @click="moveRule(rule, 1)"
                      >
                        <ArrowDown class="h-3.5 w-3.5" />
                      </el-button>
                    </div>
                  </div>

                  <div class="flex-1 min-w-0 space-y-1.5">
                    <div class="flex flex-wrap items-center gap-2">
                      <el-switch v-model="rule.enabled" size="small" />
                      <el-tag
                        size="small"
                        :type="rule.action === 'allow' ? 'success' : 'danger'"
                        effect="dark"
                      >
                        {{ rule.action === "allow" ? "ALLOW 放行" : "REJECT 拦截" }}
                      </el-tag>
                      <el-tag size="small" type="info" effect="plain">
                        {{ matcherTypeLabel(rule.matcher.type) }}
                      </el-tag>
                      <span class="font-semibold text-sm text-slate-900 truncate">
                        {{ rule.name }}
                      </span>
                      <span class="font-mono text-[11px] text-slate-400">{{ rule.id }}</span>
                      <el-tag
                        v-if="(hitCountByRuleId.get(rule.id) ?? 0) > 0"
                        size="small"
                        :type="rule.action === 'allow' ? 'success' : 'warning'"
                        effect="light"
                      >
                        线上命中 {{ hitCountByRuleId.get(rule.id) }} 次
                      </el-tag>
                    </div>

                    <div class="text-xs text-slate-700 font-mono bg-slate-50 rounded px-2.5 py-1.5 border border-slate-100 break-all">
                      {{ summarizeMatcher(rule.matcher) }}
                    </div>

                    <div v-if="rule.note" class="text-[11px] text-slate-500">
                      {{ rule.note }}
                    </div>
                  </div>
                </div>

                <div class="flex items-center gap-1 shrink-0">
                  <el-button size="small" plain @click="openEditRuleDrawer(rule)">
                    <Edit3 class="h-3.5 w-3.5 mr-1" />
                    配置
                  </el-button>
                  <el-button size="small" link @click="duplicateRule(rule)">
                    <Copy class="h-3.5 w-3.5" />
                  </el-button>
                  <el-button size="small" type="danger" link @click="removeRule(rule)">
                    <Trash2 class="h-3.5 w-3.5" />
                  </el-button>
                </div>
              </div>
            </div>
          </div>
        </div>
      </el-tab-pane>

      <!-- Tab 2: 标题清洗管线与动态词库 -->
      <el-tab-pane name="cleaner">
        <template #label>
          <span class="inline-flex items-center gap-1.5">
            <Sparkles class="h-3.5 w-3.5" />
            标题清洗管线 ({{ workingBundle.ad_phrases.length + workingBundle.ad_regexes.length }})
          </span>
        </template>

        <div class="space-y-5">
          <!-- 8 阶段可视化开关 -->
          <div class="rounded-xl border border-indigo-200 bg-indigo-50/40 p-4 space-y-3">
            <div class="flex flex-wrap items-center justify-between gap-3">
              <div>
                <div class="text-sm font-semibold text-indigo-950">8 阶段本地标题去广告清洗开关</div>
                <div class="text-xs text-indigo-800/80">
                  内置 1,284 个 IANA 顶级域智能识别；支持按需关闭任意清洗阶段
                </div>
              </div>
              <div class="flex items-center gap-2 bg-white rounded-lg border border-indigo-200 px-3 py-1.5">
                <span class="text-xs font-medium text-slate-700">清洗后有效标题为空时直接拦截：</span>
                <el-switch
                  v-model="workingBundle.policy.cleaner.reject_if_empty_after_clean"
                  size="small"
                />
              </div>
            </div>

            <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-2.5">
              <div
                v-for="st in CLEANER_STAGE_META"
                :key="st.key"
                class="rounded-lg border border-slate-200 bg-white p-2.5 flex items-start justify-between gap-2"
              >
                <div class="min-w-0">
                  <div class="text-xs font-semibold text-slate-800">{{ st.label }}</div>
                  <div class="text-[11px] text-slate-500 mt-0.5">{{ st.desc }}</div>
                </div>
                <el-switch v-model="workingBundle.policy.cleaner[st.key]" size="small" />
              </div>
            </div>
          </div>

          <div class="grid grid-cols-1 lg:grid-cols-2 gap-5">
            <!-- 动态广告短语 -->
            <div class="rounded-lg border border-slate-200 p-4 space-y-3">
              <div class="flex items-center justify-between">
                <h3 class="text-sm font-semibold text-slate-900">
                  动态广告短语 (`ad_phrases` · {{ workingBundle.ad_phrases.length }})
                </h3>
                <el-input
                  v-model="phraseSearch"
                  size="small"
                  clearable
                  placeholder="筛选短语..."
                  style="width: 150px"
                />
              </div>

              <div class="flex items-center gap-2">
                <el-input v-model="newAdPhrase" size="small" placeholder="新增广告短语" class="flex-1" />
                <el-input v-model="newAdPhraseNote" size="small" placeholder="备注" style="width: 110px" />
                <el-button size="small" type="primary" @click="addAdPhrase">添加</el-button>
              </div>

              <el-table :data="filteredAdPhrases" size="small" border stripe max-height="420">
                <el-table-column label="启用" width="60">
                  <template #default="{ row }">
                    <el-switch v-model="row.enabled" size="small" />
                  </template>
                </el-table-column>
                <el-table-column label="广告短语" min-width="150">
                  <template #default="{ row }">
                    <el-input v-model="row.phrase" size="small" />
                  </template>
                </el-table-column>
                <el-table-column label="备注" width="115">
                  <template #default="{ row }">
                    <el-input v-model="row.note" size="small" />
                  </template>
                </el-table-column>
                <el-table-column label="删" width="55">
                  <template #default="{ row }">
                    <el-button
                      size="small"
                      type="danger"
                      link
                      @click="workingBundle.ad_phrases = workingBundle.ad_phrases.filter((x) => x !== row)"
                    >
                      删
                    </el-button>
                  </template>
                </el-table-column>
              </el-table>
            </div>

            <!-- 动态广告正则 -->
            <div class="rounded-lg border border-slate-200 p-4 space-y-3">
              <h3 class="text-sm font-semibold text-slate-900">
                动态广告正则 (`ad_regexes` · {{ workingBundle.ad_regexes.length }})
              </h3>

              <div class="flex flex-wrap items-center gap-2">
                <el-input
                  v-model="newAdRegex"
                  size="small"
                  placeholder="新增广告清洗正则"
                  class="flex-1 min-w-[180px] font-mono"
                />
                <el-input
                  v-model="newAdRegexReplacement"
                  size="small"
                  placeholder="替换为"
                  style="width: 85px"
                  class="font-mono"
                />
                <el-input v-model="newAdRegexNote" size="small" placeholder="备注" style="width: 95px" />
                <el-button size="small" type="primary" @click="addAdRegex">添加</el-button>
              </div>

              <el-table :data="workingBundle.ad_regexes" size="small" border stripe max-height="420">
                <el-table-column label="启用" width="55">
                  <template #default="{ row }">
                    <el-switch v-model="row.enabled" size="small" />
                  </template>
                </el-table-column>
                <el-table-column label="正则表达式" min-width="170">
                  <template #default="{ row }">
                    <el-input v-model="row.pattern" size="small" class="font-mono" />
                  </template>
                </el-table-column>
                <el-table-column label="替换为" width="85">
                  <template #default="{ row }">
                    <el-input v-model="row.replacement" size="small" class="font-mono" />
                  </template>
                </el-table-column>
                <el-table-column label="备注" width="105">
                  <template #default="{ row }">
                    <el-input v-model="row.note" size="small" />
                  </template>
                </el-table-column>
                <el-table-column label="删" width="50">
                  <template #default="{ row }">
                    <el-button
                      size="small"
                      type="danger"
                      link
                      @click="workingBundle.ad_regexes = workingBundle.ad_regexes.filter((x) => x !== row)"
                    >
                      删
                    </el-button>
                  </template>
                </el-table-column>
              </el-table>
            </div>
          </div>
        </div>
      </el-tab-pane>

      <!-- Tab 3: 在线演练场与 A/B 对比 -->
      <el-tab-pane name="playground">
        <template #label>
          <span class="inline-flex items-center gap-1.5">
            <FlaskConical class="h-3.5 w-3.5" />
            在线演练场 & A/B 对比
          </span>
        </template>

        <div class="space-y-6">
          <!-- 单条精调调试 -->
          <div class="rounded-lg border border-slate-200 p-4 space-y-4">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="text-sm font-semibold text-slate-900">单条实时调试（清洗步骤跟踪 + Allow/Reject 命中归因）</div>
              <el-radio-group v-model="pgMode" size="small">
                <el-radio-button value="ab_compare">线上 vs 编辑器 A/B 对比</el-radio-button>
                <el-radio-button value="draft">仅测编辑器当前规则</el-radio-button>
                <el-radio-button value="published">仅测线上已发布规则</el-radio-button>
              </el-radio-group>
            </div>

            <div class="flex flex-wrap items-center gap-2.5">
              <el-input
                v-model="singleTitle"
                size="small"
                placeholder="输入待测种子标题..."
                class="flex-1 min-w-[260px]"
                @keyup.enter="runSingleTest"
              />
              <el-input
                v-model="singleInfoHash"
                size="small"
                placeholder="可选 40 位 info_hash"
                style="width: 220px"
                class="font-mono"
              />
              <el-checkbox v-model="singleAutoFile" size="small">自动合成 100MiB 视频占位文件</el-checkbox>
              <el-button size="small" type="primary" :loading="singleRunning" @click="runSingleTest">
                <Play class="h-3.5 w-3.5 mr-1" />
                运行测试
              </el-button>
            </div>

            <div v-if="!singleAutoFile" class="rounded border border-slate-200 p-3 space-y-2 bg-slate-50">
              <div class="flex items-center justify-between text-xs text-slate-600">
                <span>自定义模拟文件列表</span>
                <el-button size="small" link type="primary" @click="singleFiles.push({ path: '', size: 0 })">
                  + 添加文件
                </el-button>
              </div>
              <div v-for="(f, idx) in singleFiles" :key="idx" class="flex items-center gap-2">
                <el-input v-model="f.path" size="small" placeholder="path" class="flex-1 font-mono" />
                <ByteSizeInput v-model="f.size" :min="0" />
                <el-button size="small" type="danger" link @click="singleFiles.splice(idx, 1)">删</el-button>
              </div>
            </div>

            <div v-if="singleResult" class="space-y-3 pt-2 border-t border-slate-100">
              <div class="flex flex-wrap items-center gap-3 text-xs">
                <el-tag :type="singleResult.accepted ? 'success' : 'danger'">
                  {{ singleResult.accepted ? "放行 (Accepted)" : "拦截 (Rejected)" }}
                </el-tag>
                <el-tag v-if="singleAbItem" :type="abDiffTagType(singleAbItem.diff_status)" effect="plain">
                  A/B 状态: {{ abDiffLabel(singleAbItem.diff_status) }}
                </el-tag>
                <span class="text-slate-500">原始:</span>
                <span class="font-mono text-slate-800">{{ singleResult.raw_name }}</span>
                <ArrowRight class="h-3.5 w-3.5 text-slate-400" />
                <span class="text-slate-500">清洗后:</span>
                <span class="font-mono font-semibold text-indigo-700">
                  {{ singleResult.refined_name || "(空字符串)" }}
                </span>
              </div>

              <div
                v-if="singleResult.allowed_by"
                class="rounded border border-emerald-200 bg-emerald-50 px-3.5 py-2 text-xs text-emerald-900 flex flex-wrap gap-3"
              >
                <span><strong>白名单放行:</strong> {{ singleResult.allowed_by.rule_label }}</span>
                <span><strong>规则 ID:</strong> <code class="font-mono">{{ singleResult.allowed_by.rule_id }}</code></span>
                <span><strong>详情:</strong> {{ singleResult.allowed_by.detail }}</span>
              </div>

              <div
                v-if="singleResult.hit"
                class="rounded border border-rose-200 bg-rose-50 px-3.5 py-2 text-xs text-rose-800 flex flex-wrap gap-3"
              >
                <span><strong>命中类别:</strong> {{ categoryLabel(singleResult.hit.category) }}</span>
                <span><strong>规则 ID:</strong> <code class="font-mono">{{ singleResult.hit.rule_id }}</code></span>
                <span><strong>规则标签:</strong> {{ singleResult.hit.rule_label }}</span>
                <span><strong>详情:</strong> {{ singleResult.hit.detail }}</span>
              </div>

              <el-table :data="singleResult.cleaning_trace.steps" size="small" border>
                <el-table-column prop="stage" label="清洗阶段" width="190" />
                <el-table-column prop="before" label="处理前" min-width="210" show-overflow-tooltip>
                  <template #default="{ row }">
                    <span class="font-mono text-xs text-slate-500">{{ row.before }}</span>
                  </template>
                </el-table-column>
                <el-table-column prop="after" label="处理后" min-width="210" show-overflow-tooltip>
                  <template #default="{ row }">
                    <span
                      class="font-mono text-xs"
                      :class="row.changed ? 'text-emerald-700 font-semibold' : 'text-slate-600'"
                    >
                      {{ row.after }}
                    </span>
                  </template>
                </el-table-column>
                <el-table-column label="状态" width="85">
                  <template #default="{ row }">
                    <el-tag
                      size="small"
                      :type="!row.enabled ? 'warning' : row.changed ? 'success' : 'info'"
                      effect="plain"
                    >
                      {{ !row.enabled ? "已关闭" : row.changed ? "已改" : "未变" }}
                    </el-tag>
                  </template>
                </el-table-column>
              </el-table>
            </div>
          </div>

          <!-- 批量抽样回归 & A/B Diff 报告 -->
          <div class="rounded-lg border border-slate-200 p-4 space-y-4">
            <div class="flex flex-wrap items-center justify-between gap-3">
              <div>
                <div class="text-sm font-semibold text-slate-900">
                  批量抽样回归 & A/B 影响评估 (当前已载入 {{ batchSamples.length }} 条样本)
                </div>
                <div class="text-xs text-slate-500 mt-0.5">
                  直接使用编辑器当前尚未发布的变更对比线上已发布规则，评估是否引发误杀或意外放行
                </div>
              </div>
              <div class="flex flex-wrap items-center gap-2">
                <el-input
                  v-model="batchKeyword"
                  size="small"
                  clearable
                  placeholder="抽样过滤词（可选）"
                  style="width: 150px"
                />
                <el-button
                  size="small"
                  :loading="batchSampling"
                  @click="loadSamplesFrom('manticore_latest')"
                >
                  Manticore 最新 30
                </el-button>
                <el-button
                  size="small"
                  :loading="batchSampling"
                  @click="loadSamplesFrom('manticore_random')"
                >
                  Manticore 随机 30
                </el-button>
                <el-button
                  size="small"
                  :loading="batchSampling"
                  @click="loadSamplesFrom('rejected_recent')"
                >
                  最近拦截样本 30
                </el-button>
                <el-button size="small" @click="batchPasteVisible = true">粘贴多行标题</el-button>
                <el-button
                  size="small"
                  type="primary"
                  :loading="batchRunning"
                  :disabled="batchSamples.length === 0"
                  @click="runBatchAbCompare"
                >
                  <Play class="h-3.5 w-3.5 mr-1" />
                  执行批量 A/B 对比
                </el-button>
              </div>
            </div>

            <div v-if="batchAbReport" class="space-y-3">
              <div class="grid grid-cols-2 sm:grid-cols-6 gap-2.5 text-center text-xs">
                <div class="rounded border border-slate-200 bg-slate-50 p-2">
                  <div class="text-slate-500">总样本数</div>
                  <div class="text-lg font-bold font-mono text-slate-900">{{ batchAbReport.summary.total }}</div>
                </div>
                <div class="rounded border border-rose-200 bg-rose-50 p-2">
                  <div class="text-rose-700">新增拦截</div>
                  <div class="text-lg font-bold font-mono text-rose-700">{{ batchAbReport.summary.newly_rejected }}</div>
                </div>
                <div class="rounded border border-emerald-200 bg-emerald-50 p-2">
                  <div class="text-emerald-700">解除拦截</div>
                  <div class="text-lg font-bold font-mono text-emerald-700">{{ batchAbReport.summary.newly_accepted }}</div>
                </div>
                <div class="rounded border border-amber-200 bg-amber-50 p-2">
                  <div class="text-amber-700">清洗变化</div>
                  <div class="text-lg font-bold font-mono text-amber-700">{{ batchAbReport.summary.refined_name_changed }}</div>
                </div>
                <div class="rounded border border-slate-200 p-2">
                  <div class="text-slate-500">维持放行</div>
                  <div class="text-lg font-bold font-mono text-slate-700">{{ batchAbReport.summary.unchanged_accepted }}</div>
                </div>
                <div class="rounded border border-slate-200 p-2">
                  <div class="text-slate-500">维持拦截</div>
                  <div class="text-lg font-bold font-mono text-slate-700">{{ batchAbReport.summary.unchanged_rejected }}</div>
                </div>
              </div>

              <div class="flex flex-wrap items-center gap-2">
                <el-radio-group v-model="batchDiffFilter" size="small">
                  <el-radio-button value="all">全部 ({{ batchAbReport.items.length }})</el-radio-button>
                  <el-radio-button value="changed">仅显示差异</el-radio-button>
                  <el-radio-button value="newly_rejected">新增拦截</el-radio-button>
                  <el-radio-button value="newly_accepted">解除拦截</el-radio-button>
                  <el-radio-button value="refined_name_changed">清洗变化</el-radio-button>
                </el-radio-group>
              </div>

              <el-table :data="filteredAbItems" size="small" border stripe max-height="400">
                <el-table-column label="差异" width="115">
                  <template #default="{ row }">
                    <el-tag size="small" :type="abDiffTagType(row.diff_status)">
                      {{ abDiffLabel(row.diff_status) }}
                    </el-tag>
                  </template>
                </el-table-column>
                <el-table-column prop="raw_name" label="原始标题" min-width="220" show-overflow-tooltip />
                <el-table-column label="线上版本结果" min-width="210" show-overflow-tooltip>
                  <template #default="{ row }">
                    <span v-if="row.published_accepted" class="text-xs text-emerald-700 font-mono">
                      [{{ row.published_allowed_by ? `白名单: ${row.published_allowed_by.rule_label}` : "放行" }}]
                      {{ row.published_refined_name }}
                    </span>
                    <span v-else class="text-xs text-rose-700">
                      [拦截] {{ row.published_hit?.rule_label || "-" }}
                    </span>
                  </template>
                </el-table-column>
                <el-table-column label="草稿规则结果" min-width="210" show-overflow-tooltip>
                  <template #default="{ row }">
                    <span v-if="row.draft_accepted" class="text-xs text-emerald-700 font-mono">
                      [{{ row.draft_allowed_by ? `白名单: ${row.draft_allowed_by.rule_label}` : "放行" }}]
                      {{ row.draft_refined_name }}
                    </span>
                    <span v-else class="text-xs text-rose-700">
                      [拦截] {{ row.draft_hit?.rule_label || "-" }}
                    </span>
                  </template>
                </el-table-column>
                <el-table-column label="调试" width="75">
                  <template #default="{ row }">
                    <el-button
                      size="small"
                      link
                      type="primary"
                      @click="sendSampleToPlayground(row.raw_name, row.info_hash)"
                    >
                      单测
                    </el-button>
                  </template>
                </el-table-column>
              </el-table>
            </div>
          </div>
        </div>
      </el-tab-pane>

      <!-- Tab 4: 规则命中统计与样本巡检 -->
      <el-tab-pane name="stats">
        <template #label>
          <span class="inline-flex items-center gap-1.5">
            <AlertTriangle class="h-3.5 w-3.5" />
            命中排行 & 最近 200 样本
          </span>
        </template>

        <div class="space-y-5" v-loading="statsLoading">
          <div class="flex flex-wrap items-center justify-between gap-3">
            <div class="grid grid-cols-1 sm:grid-cols-3 gap-3 flex-1">
              <div class="rounded-lg border border-slate-200 px-4 py-3">
                <div class="text-xs text-slate-500">累计过滤评估总次数</div>
                <div class="text-xl font-bold font-mono text-slate-900 mt-0.5">
                  {{ (stats?.total_evaluated ?? 0).toLocaleString("zh-CN") }}
                </div>
              </div>
              <div class="rounded-lg border border-emerald-200 bg-emerald-50/40 px-4 py-3">
                <div class="text-xs text-emerald-700">累计放行入库数</div>
                <div class="text-xl font-bold font-mono text-emerald-700 mt-0.5">
                  {{ (stats?.total_accepted ?? 0).toLocaleString("zh-CN") }}
                </div>
              </div>
              <div class="rounded-lg border border-rose-200 bg-rose-50/40 px-4 py-3">
                <div class="text-xs text-rose-700">累计拦截丢弃数</div>
                <div class="text-xl font-bold font-mono text-rose-700 mt-0.5">
                  {{ (stats?.total_rejected ?? 0).toLocaleString("zh-CN") }}
                </div>
              </div>
            </div>
            <el-button size="small" :loading="statsLoading" @click="fetchStats">
              <RefreshCw class="h-3.5 w-3.5 mr-1" />
              刷新统计
            </el-button>
          </div>

          <div class="grid grid-cols-1 lg:grid-cols-2 gap-5">
            <!-- Top 规则命中排行 -->
            <div class="rounded-lg border border-slate-200 p-4 space-y-2.5">
              <div class="text-sm font-semibold text-slate-900">Top 50 规则命中归因排行</div>
              <el-table :data="stats?.rule_hits ?? []" size="small" border stripe max-height="380">
                <el-table-column label="动作" width="75">
                  <template #default="{ row }">
                    <el-tag
                      size="small"
                      :type="row.action === 'allow' ? 'success' : 'danger'"
                      effect="light"
                    >
                      {{ row.action === "allow" ? "放行" : "拦截" }}
                    </el-tag>
                  </template>
                </el-table-column>
                <el-table-column label="规则类别" width="120">
                  <template #default="{ row }">
                    <el-tag size="small" effect="plain">{{ categoryLabel(row.category) }}</el-tag>
                  </template>
                </el-table-column>
                <el-table-column prop="rule_label" label="规则名称" min-width="150" show-overflow-tooltip />
                <el-table-column prop="rule_id" label="ID" width="100" show-overflow-tooltip>
                  <template #default="{ row }">
                    <span class="font-mono text-xs text-slate-500">{{ row.rule_id }}</span>
                  </template>
                </el-table-column>
                <el-table-column prop="hits" label="命中数" width="80">
                  <template #default="{ row }">
                    <span
                      class="font-mono font-semibold"
                      :class="row.action === 'allow' ? 'text-emerald-700' : 'text-rose-700'"
                    >
                      {{ row.hits }}
                    </span>
                  </template>
                </el-table-column>
              </el-table>
            </div>

            <!-- Indexer 节点分布 -->
            <div class="rounded-lg border border-slate-200 p-4 space-y-2.5">
              <div class="text-sm font-semibold text-slate-900">各 Indexer 节点过滤吞吐统计</div>
              <el-table :data="stats?.workers ?? []" size="small" border stripe max-height="380">
                <el-table-column prop="id" label="节点 ID" width="120">
                  <template #default="{ row }">
                    <span class="font-mono font-medium">{{ row.id }}</span>
                  </template>
                </el-table-column>
                <el-table-column prop="runtime_state" label="状态" width="95" />
                <el-table-column label="规则版本" width="85">
                  <template #default="{ row }">
                    <span class="font-mono text-xs">{{ row.active_rule_version ? `v${row.active_rule_version}` : "-" }}</span>
                  </template>
                </el-table-column>
                <el-table-column prop="total_evaluated" label="总判定" width="90" />
                <el-table-column prop="total_rejected" label="已拦截" width="90" />
              </el-table>
            </div>
          </div>

          <!-- 最近 200 条拦截样本巡检 -->
          <div class="rounded-lg border border-slate-200 p-4 space-y-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div class="text-sm font-semibold text-slate-900">
                最近被拦截种子样本巡检 (共 {{ filteredRejectedSamples.length }} 条)
              </div>
              <div class="flex items-center gap-2">
                <el-select v-model="rejectedCategoryFilter" size="small" style="width: 155px">
                  <el-option label="全部拦截类别" value="all" />
                  <el-option label="关键词词组" value="keyword_group" />
                  <el-option label="扩展名后缀" value="extension_rule" />
                  <el-option label="垃圾标题特征" value="title_pattern_preset" />
                  <el-option label="文件体积/长度约束" value="file_constraint" />
                  <el-option label="实质内容底线" value="substantive_content" />
                  <el-option label="体积占比上限" value="size_ratio" />
                  <el-option label="自定义正则" value="custom_regex" />
                  <el-option label="复合条件规则" value="composite" />
                  <el-option label="清洗后为空" value="empty_refined_name" />
                </el-select>
                <el-input
                  v-model="rejectedSearch"
                  size="small"
                  clearable
                  placeholder="搜索样本标题或 Hash..."
                  style="width: 220px"
                />
              </div>
            </div>

            <el-table :data="filteredRejectedSamples" size="small" border stripe max-height="420">
              <el-table-column prop="worker_id" label="节点" width="100" />
              <el-table-column prop="raw_name" label="原始标题" min-width="230" show-overflow-tooltip />
              <el-table-column label="大小 / 文件数" width="120">
                <template #default="{ row }">
                  <span class="font-mono text-xs">{{ formatBytes(row.total_size) }} / {{ row.file_count }}</span>
                </template>
              </el-table-column>
              <el-table-column label="命中归因" min-width="210" show-overflow-tooltip>
                <template #default="{ row }">
                  <span class="text-xs text-rose-700">
                    [{{ categoryLabel(row.hit.category) }}] {{ row.hit.rule_label }}: {{ row.hit.detail }}
                  </span>
                </template>
              </el-table-column>
              <el-table-column label="时间" width="150">
                <template #default="{ row }">
                  <span class="font-mono text-[11px] text-slate-500">{{ formatTs(row.rejected_at_unix) }}</span>
                </template>
              </el-table-column>
              <el-table-column label="操作" width="215" fixed="right">
                <template #default="{ row }">
                  <div class="flex items-center gap-1.5">
                    <el-button
                      size="small"
                      type="primary"
                      link
                      @click="sendSampleToPlayground(row.raw_name, row.info_hash)"
                    >
                      演练
                    </el-button>
                    <el-button size="small" link @click="locateRuleFromSample(row)">
                      定位规则
                    </el-button>
                    <el-button
                      size="small"
                      type="success"
                      link
                      @click="quickAllowFromSample(row)"
                    >
                      <ShieldCheck class="h-3.5 w-3.5 mr-0.5" />
                      加白名单
                    </el-button>
                  </div>
                </template>
              </el-table-column>
            </el-table>
          </div>
        </div>
      </el-tab-pane>

      <!-- Tab 5: 历史版本与回滚 -->
      <el-tab-pane name="versions">
        <template #label>
          <span class="inline-flex items-center gap-1.5">
            <History class="h-3.5 w-3.5" />
            规则版本历史 ({{ versions.length }})
          </span>
        </template>

        <el-table :data="versions" size="small" border stripe v-loading="versionsLoading">
          <el-table-column prop="version" label="版本" width="80">
            <template #default="{ row }">
              <span class="font-mono font-semibold">v{{ row.version }}</span>
            </template>
          </el-table-column>
          <el-table-column prop="status" label="状态" width="105">
            <template #default="{ row }">
              <el-tag
                size="small"
                :type="
                  row.status === 'published'
                    ? 'success'
                    : row.status === 'draft'
                      ? 'warning'
                      : 'info'
                "
              >
                {{
                  row.status === "published"
                    ? "当前线上"
                    : row.status === "draft"
                      ? "草稿"
                      : "已归档"
                }}
              </el-tag>
            </template>
          </el-table-column>
          <el-table-column label="策略摘要" min-width="250">
            <template #default="{ row }">
              <span class="text-xs font-mono text-slate-600">
                规则 {{ row.bundle.policy.rules.length }} 条（Allow {{ row.bundle.policy.rules.filter((x: PolicyRule) => x.action === "allow").length }} / Reject {{ row.bundle.policy.rules.filter((x: PolicyRule) => x.action === "reject").length }}）· 广告词库 {{ row.bundle.ad_phrases.length + row.bundle.ad_regexes.length }}
              </span>
            </template>
          </el-table-column>
          <el-table-column prop="created_by" label="操作人" width="100" />
          <el-table-column prop="note" label="变更说明" min-width="170" />
          <el-table-column label="发布/创建时间" width="165">
            <template #default="{ row }">{{ formatTs(row.published_at ?? row.created_at) }}</template>
          </el-table-column>
          <el-table-column label="操作" width="180" fixed="right">
            <template #default="{ row }">
              <el-button
                size="small"
                link
                @click="
                  inspectVersionData = row;
                  inspectVersionVisible = true;
                "
              >
                <FileJson class="h-3.5 w-3.5 mr-0.5" />
                查看 JSON
              </el-button>
              <el-button
                size="small"
                type="warning"
                link
                :disabled="row.status === 'published' || row.status === 'draft'"
                @click="handleRollback(row.version)"
              >
                回滚并热更新
              </el-button>
            </template>
          </el-table-column>
        </el-table>
      </el-tab-pane>
    </el-tabs>

    <!-- 规则编辑抽屉 -->
    <RuleEditorDrawer
      v-model:visible="drawerVisible"
      :rule="drawerRule"
      :is-new="drawerIsNew"
      :templates="presetsCatalog?.rule_templates ?? []"
      :extension-packs="presetsCatalog?.extension_packs ?? []"
      :keyword-packs="presetsCatalog?.keyword_packs ?? []"
      @save="handleSaveRuleFromDrawer"
    />

    <!-- 导入 JSON（旧版 config.json / Hameta v1 / Hameta v2）弹窗 -->
    <el-dialog v-model="legacyImportVisible" title="导入并智能转换过滤配置 JSON" width="660px">
      <div class="space-y-3 text-xs">
        <p class="text-slate-600">
          自动识别 <strong>旧版 torrent-indexer config.json</strong>（含 <code>"filter": &#123; ... &#125;</code> 或裸对象）、<strong>Hameta v1</strong> 或 <strong>Hameta v2 策略包</strong>；旧版中的扩展名正则与特征正则会自动转为可视化开关与后缀芯片，通过预编译校验后写入草稿。
        </p>
        <input
          ref="legacyFileRef"
          type="file"
          accept=".json,application/json"
          class="hidden"
          @change="handleLegacyFileSelected"
        />
        <div class="flex items-center gap-2">
          <el-button size="small" @click="legacyFileRef?.click()">选择本地 JSON 文件</el-button>
          <el-input
            v-model="legacyImportNote"
            size="small"
            placeholder="导入说明（可选）"
            class="flex-1"
          />
        </div>
        <el-input
          v-model="legacyImportText"
          type="textarea"
          :rows="14"
          placeholder='粘贴旧版 config.json / Hameta v1 / Hameta v2 JSON...'
          class="font-mono text-xs"
        />
      </div>
      <template #footer>
        <el-button size="small" @click="legacyImportVisible = false">取消</el-button>
        <el-button size="small" type="primary" :loading="legacyImporting" @click="submitLegacyImport">
          智能解析并写入草稿
        </el-button>
      </template>
    </el-dialog>

    <!-- 粘贴多行标题样本弹窗 -->
    <el-dialog v-model="batchPasteVisible" title="粘贴多行待测标题文本（最多 200 行）" width="560px">
      <el-input
        v-model="batchPasteText"
        type="textarea"
        :rows="12"
        placeholder="每行粘贴一条种子标题..."
      />
      <template #footer>
        <el-button size="small" @click="batchPasteVisible = false">取消</el-button>
        <el-button size="small" type="primary" @click="applyPasteSamples">加载样本</el-button>
      </template>
    </el-dialog>

    <!-- 查看历史版本 JSON 弹窗 -->
    <el-dialog
      v-model="inspectVersionVisible"
      :title="`过滤策略快照 v${inspectVersionData?.version ?? ''}`"
      width="720px"
    >
      <pre class="rounded-lg bg-slate-950 text-slate-100 p-4 text-xs font-mono max-h-[500px] overflow-auto">{{ inspectVersionData ? JSON.stringify(inspectVersionData.bundle, null, 2) : "" }}</pre>
      <template #footer>
        <el-button size="small" @click="inspectVersionVisible = false">关闭</el-button>
      </template>
    </el-dialog>
  </div>
</template>
