<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { ElMessage } from "element-plus";
import { Sparkles } from "lucide-vue-next";
import MatcherEditor from "./MatcherEditor.vue";
import type {
  ExtensionPackInfo,
  KeywordPackInfo,
  PolicyRule,
  RuleMatcher,
  RuleTemplateInfo,
} from "@/api/filter";

const props = defineProps<{
  visible: boolean;
  rule: PolicyRule | null;
  isNew: boolean;
  templates: RuleTemplateInfo[];
  extensionPacks: ExtensionPackInfo[];
  keywordPacks: KeywordPackInfo[];
}>();

const emit = defineEmits<{
  (e: "update:visible", value: boolean): void;
  (e: "save", rule: PolicyRule): void;
}>();

const draft = ref<PolicyRule>(createEmptyRule());
const selectedTemplateId = ref("");

watch(
  () => [props.visible, props.rule] as const,
  ([vis, r]) => {
    if (vis) {
      draft.value = r ? (JSON.parse(JSON.stringify(r)) as PolicyRule) : createEmptyRule();
      selectedTemplateId.value = "";
    }
  },
  { immediate: true },
);

const groupedTemplates = computed(() => {
  const groups = new Map<string, RuleTemplateInfo[]>();
  for (const tpl of props.templates) {
    const arr = groups.get(tpl.group) ?? [];
    arr.push(tpl);
    groups.set(tpl.group, arr);
  }
  return Array.from(groups.entries()).map(([group, items]) => ({ group, items }));
});

function createEmptyRule(): PolicyRule {
  return {
    id: `rule-${Date.now()}`,
    name: "新建过滤规则",
    enabled: true,
    action: "reject",
    tags: [],
    note: "",
    matcher: {
      type: "keyword_group",
      target: "raw_name",
      match_mode: "word_boundary",
      case_sensitive: false,
      words: [],
    },
  };
}

function applyTemplate(tplId: string) {
  const tpl = props.templates.find((t) => t.id === tplId);
  if (!tpl) return;
  const keepId = draft.value.id;
  const cloned = JSON.parse(JSON.stringify(tpl.rule)) as PolicyRule;
  cloned.id = keepId || `rule-${Date.now()}`;
  draft.value = cloned;
  ElMessage.success(`已填充模板「${tpl.name}」的所有预设配置`);
}

function switchMatcherType(nextType: RuleMatcher["type"]) {
  if (draft.value.matcher.type === nextType) return;
  switch (nextType) {
    case "keyword_group":
      draft.value.matcher = {
        type: "keyword_group",
        target: "raw_name",
        match_mode: "word_boundary",
        case_sensitive: false,
        words: [],
      };
      break;
    case "extension_rule":
      draft.value.matcher = {
        type: "extension_rule",
        scope: "any_file",
        extensions: [],
      };
      break;
    case "title_pattern_preset":
      draft.value.matcher = {
        type: "title_pattern_preset",
        patterns: [],
        min_cyrillic_chars: 2,
      };
      break;
    case "file_constraint":
      draft.value.matcher = {
        type: "file_constraint",
        extensions: ["mp4", "mkv"],
        extra_regexes: [],
        mode: "at_least_one_matched",
        min_file_size: 10 * 1024 * 1024,
        max_file_size: null,
        min_file_name_length: null,
        max_file_name_length: null,
      };
      break;
    case "substantive_content":
      draft.value.matcher = {
        type: "substantive_content",
        junk_extensions: ["ink", "url", "lnk", "scf", "html", "txt"],
        include_system_junk: true,
        extra_junk_regexes: [],
        min_files: 1,
        min_total_size: 1024 * 1024,
      };
      break;
    case "size_ratio":
      draft.value.matcher = {
        type: "size_ratio",
        extensions: ["zip", "rar", "7z", "iso"],
        include_split_archives: true,
        extra_regexes: [],
        max_ratio: 0.7,
      };
      break;
    case "custom_regex":
      draft.value.matcher = {
        type: "custom_regex",
        target: "raw_name",
        match_all: false,
        patterns: [],
      };
      break;
    case "composite":
      draft.value.matcher = {
        type: "composite",
        logic: "all",
        conditions: [],
      };
      break;
  }
}

function handleSave() {
  if (!draft.value.name.trim()) {
    ElMessage.warning("请填写规则名称");
    return;
  }
  if (!draft.value.id.trim()) {
    draft.value.id = `rule-${Date.now()}`;
  }
  emit("save", JSON.parse(JSON.stringify(draft.value)) as PolicyRule);
  emit("update:visible", false);
}
</script>

<template>
  <el-drawer
    :model-value="visible"
    :title="isNew ? '新建过滤策略规则' : `编辑规则：${draft.name}`"
    size="740px"
    destroy-on-close
    @update:model-value="(v: boolean) => emit('update:visible', v)"
  >
    <div class="space-y-4 pb-6">
      <!-- 内置规则模板一键套用栏 -->
      <div class="rounded-lg border border-indigo-200 bg-indigo-50/70 p-3 space-y-2">
        <div class="flex items-center gap-1.5 text-xs font-semibold text-indigo-900">
          <Sparkles class="h-3.5 w-3.5" />
          <span>快速套用内置预设模板（选中即自动填充扩展名 / 关键词 / 阈值）</span>
        </div>
        <el-select
          v-model="selectedTemplateId"
          placeholder="选择内置规则模板（后缀拦截 / 关键词拦截 / 影视约束 / 白名单豁免...）"
          filterable
          clearable
          size="small"
          class="w-full"
          @change="applyTemplate"
        >
          <el-option-group
            v-for="grp in groupedTemplates"
            :key="grp.group"
            :label="grp.group"
          >
            <el-option
              v-for="item in grp.items"
              :key="item.id"
              :label="`${item.name} — ${item.description}`"
              :value="item.id"
            />
          </el-option-group>
        </el-select>
      </div>

      <!-- 基础元信息 -->
      <div class="rounded-lg border border-slate-200 p-3.5 bg-white space-y-3">
        <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
          <div>
            <div class="text-xs font-medium text-slate-600 mb-1">规则名称</div>
            <el-input v-model="draft.name" size="small" placeholder="例如：高危可执行后缀硬拦截" />
          </div>
          <div>
            <div class="text-xs font-medium text-slate-600 mb-1">命中执行动作</div>
            <el-radio-group v-model="draft.action" size="small" class="w-full">
              <el-radio-button value="reject">Reject（直接拦截丢弃）</el-radio-button>
              <el-radio-button value="allow">Allow（白名单直接放行）</el-radio-button>
            </el-radio-group>
          </div>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-2 gap-3 items-center">
          <div>
            <div class="text-xs font-medium text-slate-600 mb-1">规则说明备注（可选）</div>
            <el-input v-model="draft.note" size="small" placeholder="便于后续团队运维回顾用途" />
          </div>
          <div class="flex items-center justify-between pt-4 md:pt-5">
            <span class="text-xs text-slate-600">启用状态</span>
            <el-switch v-model="draft.enabled" active-text="已启用" inactive-text="已停用" />
          </div>
        </div>
      </div>

      <!-- 规则类型选择 -->
      <div class="rounded-lg border border-slate-200 p-3.5 bg-white space-y-3">
        <div class="flex flex-wrap items-center justify-between gap-2 border-b border-slate-100 pb-2.5">
          <span class="text-xs font-semibold text-slate-800">匹配器类型</span>
          <el-select
            :model-value="draft.matcher.type"
            size="small"
            style="width: 240px"
            @change="switchMatcherType"
          >
            <el-option label="关键词词组 (KeywordGroup)" value="keyword_group" />
            <el-option label="扩展名后缀集合 (ExtensionRule)" value="extension_rule" />
            <el-option label="标题结构特征开关 (TitlePatternPreset)" value="title_pattern_preset" />
            <el-option label="文件体积与文件名约束 (FileConstraint)" value="file_constraint" />
            <el-option label="去噪实质内容底线 (SubstantiveContent)" value="substantive_content" />
            <el-option label="类型体积占比上限 (SizeRatio)" value="size_ratio" />
            <el-option label="专家自定义正则 (CustomRegex)" value="custom_regex" />
            <el-option label="多条件组合逻辑 (Composite)" value="composite" />
          </el-select>
        </div>

        <MatcherEditor
          v-model="draft.matcher"
          :extension-packs="extensionPacks"
          :keyword-packs="keywordPacks"
          :allow-composite="true"
        />
      </div>
    </div>

    <template #footer>
      <div class="flex justify-end gap-2">
        <el-button @click="emit('update:visible', false)">取消</el-button>
        <el-button type="primary" @click="handleSave">确定应用到编辑区</el-button>
      </div>
    </template>
  </el-drawer>
</template>
