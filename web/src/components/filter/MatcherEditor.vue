<script setup lang="ts">
import { computed, ref } from "vue";
import { ArrowDown, ArrowUp, Plus, Trash2, X } from "lucide-vue-next";
import type {
  CompositeLogic,
  ExtensionPackInfo,
  ExtensionScope,
  FileConstraintMode,
  KeywordMatchMode,
  KeywordPackInfo,
  RuleMatcher,
  TextTarget,
  TitlePatternKind,
} from "@/api/filter";
import ByteSizeInput from "@/components/filter/ByteSizeInput.vue";
import ObsNumberInput from "@/components/ui/ObsNumberInput.vue";
import ObsSelect from "@/components/ui/ObsSelect.vue";

const props = withDefaults(
  defineProps<{
    modelValue: RuleMatcher;
    extensionPacks?: ExtensionPackInfo[];
    keywordPacks?: KeywordPackInfo[];
    allowComposite?: boolean;
  }>(),
  {
    extensionPacks: () => [],
    keywordPacks: () => [],
    allowComposite: true,
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", v: RuleMatcher): void;
}>();

const current = computed(() => props.modelValue);

const kwInput = ref("");
const extInput = ref("");
const childTypeToAdd = ref<Exclude<RuleMatcher["type"], "composite">>("keyword_group");

const MATCHER_TYPE_LABELS: Record<RuleMatcher["type"], string> = {
  keyword_group: "关键词词组",
  extension_rule: "扩展名后缀集合",
  title_pattern_preset: "标题结构特征",
  file_constraint: "文件体积/长度约束",
  substantive_content: "有效实质内容",
  size_ratio: "类型体积占比",
  custom_regex: "专家自定义正则",
  composite: "多条件复合逻辑",
};

const MATCHER_TYPE_OPTIONS = computed(() => {
  const base: Array<{ label: string; value: RuleMatcher["type"] }> = [
    { label: "关键词词组 (keyword_group)", value: "keyword_group" },
    { label: "扩展名集合 (extension_rule)", value: "extension_rule" },
    { label: "标题结构特征 (title_pattern_preset)", value: "title_pattern_preset" },
    { label: "单文件大小/命名长度 (file_constraint)", value: "file_constraint" },
    { label: "有效实质内容检验 (substantive_content)", value: "substantive_content" },
    { label: "指定后缀体积占比 (size_ratio)", value: "size_ratio" },
    { label: "自定义 Rust 正则 (custom_regex)", value: "custom_regex" },
  ];
  if (props.allowComposite) {
    base.push({ label: "复合逻辑组 AND/OR (composite)", value: "composite" });
  }
  return base;
});

const TARGET_OPTIONS = [
  { label: "原始标题 (raw_name)", value: "raw_name" },
  { label: "清洗后标题 (refined_name)", value: "refined_name" },
  { label: "任一文件完整路径", value: "any_file_path" },
  { label: "任一单文件名", value: "any_file_name" },
];

const EXT_SCOPE_OPTIONS = [
  { label: "仅检查 Torrent 标题末尾", value: "torrent_name" },
  { label: "存在任一命中后缀的文件", value: "any_file" },
  { label: "所有文件都必须属于这些后缀", value: "all_files" },
];

const FILE_MODE_OPTIONS = [
  { label: "每个匹配文件都必须达标 (every_matched)", value: "every_matched" },
  { label: "至少一个匹配文件达标 (at_least_one_matched)", value: "at_least_one_matched" },
];

const TITLE_PATTERNS: Array<{ value: TitlePatternKind; label: string; desc: string }> = [
  { value: "cyrillic", label: "西里尔字母标题", desc: "常见俄语推广/ spam" },
  { value: "garbled_text", label: "编码乱码标题", desc: "含 U+FFFD 替换符 / Mojibake 乱码" },
  { value: "digits_only", label: "纯数字标题", desc: "无意义的纯编号" },
  { value: "sha1_hex", label: "40 位十六进制 Hash", desc: "纯哈希充当标题" },
  { value: "md5_hex", label: "32 位 MD5 字符串", desc: "纯哈希字符串" },
  { value: "uuid", label: "UUID 格式标题", desc: "8-4-4-4-12 结构" },
  { value: "decimal_code", label: "十进制代码串", desc: "如 12345.67890" },
  { value: "magnet_prefix", label: "magnet:? 前缀", desc: "伪装磁力文本" },
  { value: "http_url_prefix", label: "http/https URL 标题", desc: "纯网址标题" },
  { value: "split_archive_suffix", label: "分卷压缩结尾", desc: ".part1.rar / .001" },
];

const CHILD_TYPE_OPTIONS: Array<{ label: string; value: Exclude<RuleMatcher["type"], "composite"> }> = [
  { label: "关键词词组", value: "keyword_group" },
  { label: "扩展名后缀集合", value: "extension_rule" },
  { label: "文件体积/长度约束", value: "file_constraint" },
  { label: "标题结构特征", value: "title_pattern_preset" },
  { label: "有效实质内容", value: "substantive_content" },
  { label: "类型体积占比", value: "size_ratio" },
  { label: "专家自定义正则", value: "custom_regex" },
];

function createDefaultMatcher(kind: RuleMatcher["type"]): RuleMatcher {
  switch (kind) {
    case "keyword_group":
      return {
        type: "keyword_group",
        target: "raw_name",
        match_mode: "contains",
        case_sensitive: false,
        words: [],
      };
    case "extension_rule":
      return { type: "extension_rule", scope: "any_file", extensions: [] };
    case "title_pattern_preset":
      return { type: "title_pattern_preset", patterns: ["digits_only"], min_cyrillic_chars: 3 };
    case "file_constraint":
      return {
        type: "file_constraint",
        extensions: ["mkv", "mp4"],
        extra_regexes: [],
        mode: "at_least_one_matched",
        min_file_size: 50 * 1024 * 1024,
        max_file_size: null,
        min_file_name_length: null,
        max_file_name_length: null,
      };
    case "substantive_content":
      return {
        type: "substantive_content",
        junk_extensions: ["txt", "url", "lnk", "html", "htm", "nfo"],
        include_system_junk: true,
        extra_junk_regexes: [],
        min_files: 1,
        min_total_size: 1024 * 1024,
      };
    case "size_ratio":
      return {
        type: "size_ratio",
        extensions: ["zip", "rar", "7z"],
        include_split_archives: true,
        extra_regexes: [],
        max_ratio: 0.9,
      };
    case "custom_regex":
      return { type: "custom_regex", target: "raw_name", match_all: false, patterns: [] };
    case "composite":
      return {
        type: "composite",
        logic: "all",
        conditions: [createDefaultMatcher("keyword_group")],
      };
  }
}

function updateMatcher(next: RuleMatcher) {
  emit("update:modelValue", next);
}

function patchCurrent(patch: Record<string, unknown>) {
  emit("update:modelValue", { ...props.modelValue, ...patch } as RuleMatcher);
}

function switchType(nextType: string | number) {
  const k = String(nextType) as RuleMatcher["type"];
  if (k === current.value.type) return;
  updateMatcher(createDefaultMatcher(k));
}

function normalizeExt(raw: string): string {
  return raw.trim().replace(/^\.+/, "").toLowerCase();
}

function linesToText(arr: string[]): string {
  return arr.join("\n");
}

function textToLines(s: string): string[] {
  return s
    .split(/\r?\n/)
    .map((x) => x.trim())
    .filter((x) => x.length > 0);
}

function addKeywordsFromInput() {
  if (current.value.type !== "keyword_group") return;
  const parts = kwInput.value
    .split(/[,，\n]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  if (parts.length === 0) return;
  const set = new Set([...current.value.words, ...parts]);
  updateMatcher({ ...current.value, words: Array.from(set) });
  kwInput.value = "";
}

function appendKeywordPack(pack: KeywordPackInfo) {
  if (current.value.type !== "keyword_group") return;
  const set = new Set([...current.value.words, ...pack.words]);
  updateMatcher({ ...current.value, words: Array.from(set) });
}

function removeWord(w: string) {
  if (current.value.type !== "keyword_group") return;
  updateMatcher({ ...current.value, words: current.value.words.filter((x) => x !== w) });
}

function getExtList(): string[] {
  const c = current.value;
  if (c.type === "extension_rule" || c.type === "file_constraint" || c.type === "size_ratio") {
    return c.extensions;
  }
  if (c.type === "substantive_content") return c.junk_extensions;
  return [];
}

function setExtList(next: string[]) {
  const c = current.value;
  if (c.type === "extension_rule" || c.type === "file_constraint" || c.type === "size_ratio") {
    updateMatcher({ ...c, extensions: next });
  } else if (c.type === "substantive_content") {
    updateMatcher({ ...c, junk_extensions: next });
  }
}

function addExtsFromInput() {
  const parts = extInput.value
    .split(/[,，\s]+/)
    .map(normalizeExt)
    .filter((s) => s.length > 0);
  if (parts.length === 0) return;
  const set = new Set([...getExtList(), ...parts]);
  setExtList(Array.from(set));
  extInput.value = "";
}

function appendExtPack(pack: ExtensionPackInfo) {
  const set = new Set([...getExtList(), ...pack.extensions.map(normalizeExt)]);
  setExtList(Array.from(set));
}

function removeExt(ext: string) {
  setExtList(getExtList().filter((e) => e !== ext));
}

function toggleTitlePattern(p: TitlePatternKind) {
  if (current.value.type !== "title_pattern_preset") return;
  const exists = current.value.patterns.includes(p);
  const next = exists
    ? current.value.patterns.filter((x) => x !== p)
    : [...current.value.patterns, p];
  updateMatcher({ ...current.value, patterns: next });
}

function addCompositeCondition() {
  if (current.value.type !== "composite") return;
  updateMatcher({
    ...current.value,
    conditions: [...current.value.conditions, createDefaultMatcher(childTypeToAdd.value)],
  });
}

function moveCompositeChild(idx: number, delta: -1 | 1) {
  if (current.value.type !== "composite") return;
  const target = idx + delta;
  if (target < 0 || target >= current.value.conditions.length) return;
  const arr = [...current.value.conditions];
  [arr[idx], arr[target]] = [arr[target], arr[idx]];
  updateMatcher({ ...current.value, conditions: arr });
}

function removeCompositeChild(idx: number) {
  if (current.value.type !== "composite") return;
  updateMatcher({
    ...current.value,
    conditions: current.value.conditions.filter((_, i) => i !== idx),
  });
}

function updateCompositeChild(idx: number, v: RuleMatcher) {
  if (current.value.type !== "composite") return;
  const arr = [...current.value.conditions];
  arr[idx] = v;
  updateMatcher({ ...current.value, conditions: arr });
}
</script>

<template>
  <div class="space-y-3 text-xs">
    <!-- Matcher Type Selector -->
    <div>
      <label class="block text-[11px] text-slate-400 mb-1">匹配器类型</label>
      <ObsSelect
        :model-value="current.type"
        :options="MATCHER_TYPE_OPTIONS"
        @update:model-value="switchType"
      />
    </div>

    <!-- 1. KeywordGroup -->
    <template v-if="current.type === 'keyword_group'">
      <div class="obs-inset p-3 rounded-lg grid grid-cols-1 md:grid-cols-3 gap-3 items-end">
        <div>
          <label class="block text-[11px] text-slate-400 mb-1">匹配目标</label>
          <ObsSelect
            :model-value="current.target"
            :options="TARGET_OPTIONS"
            @update:model-value="(v) => patchCurrent({ target: String(v) as TextTarget })"
          />
        </div>
        <div>
          <label class="block text-[11px] text-slate-400 mb-1">匹配方式</label>
          <div class="bg-[#090d16] border border-[#1e293b] p-0.5 rounded-lg inline-flex w-full">
            <button
              type="button"
              class="flex-1 py-1 rounded text-[11px] transition"
              :class="current.match_mode === 'contains' ? 'bg-emerald-500/15 text-emerald-300' : 'text-slate-400'"
              @click="patchCurrent({ match_mode: 'contains' as KeywordMatchMode })"
            >
              子串包含
            </button>
            <button
              type="button"
              class="flex-1 py-1 rounded text-[11px] transition"
              :class="current.match_mode === 'word_boundary' ? 'bg-emerald-500/15 text-emerald-300' : 'text-slate-400'"
              @click="patchCurrent({ match_mode: 'word_boundary' as KeywordMatchMode })"
            >
              整词边界
            </button>
          </div>
        </div>
        <label class="inline-flex items-center gap-2 text-xs text-slate-300 cursor-pointer pb-1.5">
          <input
            type="checkbox"
            class="obs-checkbox"
            :checked="current.case_sensitive"
            @change="(e) => patchCurrent({ case_sensitive: (e.target as HTMLInputElement).checked })"
          />
          区分大小写
        </label>
      </div>

      <div v-if="keywordPacks.length > 0" class="flex flex-wrap items-center gap-1.5">
        <span class="text-[11px] text-slate-400 mr-1">快捷词库：</span>
        <button
          v-for="pack in keywordPacks"
          :key="pack.id"
          type="button"
          class="obs-btn obs-btn-ghost px-2 py-1 text-[11px]"
          @click="appendKeywordPack(pack)"
        >
          + {{ pack.name }}
        </button>
      </div>

      <div class="flex items-center gap-2">
        <input
          v-model="kwInput"
          type="text"
          placeholder="输入关键词（支持逗号或换行分隔批量追加）"
          class="obs-input px-3 py-1.5 text-xs flex-1"
          @keyup.enter="addKeywordsFromInput"
        />
        <button type="button" class="obs-btn obs-btn-primary px-3 py-1.5 text-xs" @click="addKeywordsFromInput">
          追加词条
        </button>
      </div>

      <div class="obs-inset p-2.5 rounded-lg flex flex-wrap gap-1.5 min-h-[44px]">
        <span
          v-for="w in current.words"
          :key="w"
          class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-slate-800/90 border border-slate-700/80 text-[11px] font-mono text-slate-200"
        >
          {{ w }}
          <button type="button" class="text-slate-400 hover:text-rose-300" @click="removeWord(w)">
            <X class="w-3 h-3" />
          </button>
        </span>
        <span v-if="current.words.length === 0" class="text-[11px] text-slate-500">尚未添加关键词</span>
      </div>
    </template>

    <!-- 2. ExtensionRule -->
    <template v-else-if="current.type === 'extension_rule'">
      <div class="obs-inset p-3 rounded-lg">
        <label class="block text-[11px] text-slate-400 mb-1">扩展名检查范围</label>
        <ObsSelect
          :model-value="current.scope"
          :options="EXT_SCOPE_OPTIONS"
          @update:model-value="(v) => patchCurrent({ scope: String(v) as ExtensionScope })"
        />
      </div>

      <div v-if="extensionPacks.length > 0" class="flex flex-wrap items-center gap-1.5">
        <span class="text-[11px] text-slate-400 mr-1">快捷后缀包：</span>
        <button
          v-for="pack in extensionPacks"
          :key="pack.id"
          type="button"
          class="obs-btn obs-btn-ghost px-2 py-1 text-[11px]"
          @click="appendExtPack(pack)"
        >
          + {{ pack.name }}
        </button>
      </div>

      <div class="flex items-center gap-2">
        <input
          v-model="extInput"
          type="text"
          placeholder="追加扩展名（如 exe, scr, mkv, flac）"
          class="obs-input px-3 py-1.5 text-xs font-mono flex-1"
          @keyup.enter="addExtsFromInput"
        />
        <button type="button" class="obs-btn obs-btn-primary px-3 py-1.5 text-xs" @click="addExtsFromInput">
          追加后缀
        </button>
      </div>

      <div class="obs-inset p-2.5 rounded-lg flex flex-wrap gap-1.5 min-h-[44px]">
        <span
          v-for="ext in current.extensions"
          :key="ext"
          class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-cyan-500/10 border border-cyan-500/30 text-[11px] font-mono text-cyan-200"
        >
          .{{ ext }}
          <button type="button" class="text-cyan-400 hover:text-rose-300" @click="removeExt(ext)">
            <X class="w-3 h-3" />
          </button>
        </span>
        <span v-if="current.extensions.length === 0" class="text-[11px] text-slate-500">尚未指定后缀</span>
      </div>
    </template>

    <!-- 3. TitlePatternPreset -->
    <template v-else-if="current.type === 'title_pattern_preset'">
      <div class="obs-inset p-3 rounded-lg space-y-3">
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
          <label
            v-for="pat in TITLE_PATTERNS"
            :key="pat.value"
            class="flex items-start gap-2 p-2 rounded border cursor-pointer transition"
            :class="
              current.patterns.includes(pat.value)
                ? 'bg-emerald-500/10 border-emerald-500/35 text-slate-100'
                : 'bg-[#090d16] border-[#1e293b] text-slate-400'
            "
          >
            <input
              type="checkbox"
              class="obs-checkbox mt-0.5"
              :checked="current.patterns.includes(pat.value)"
              @change="toggleTitlePattern(pat.value)"
            />
            <div>
              <div class="font-medium text-xs">{{ pat.label }}</div>
              <div class="text-[10px] text-slate-500">{{ pat.desc }}</div>
            </div>
          </label>
        </div>

        <div class="w-48">
          <label class="block text-[11px] text-slate-400 mb-1">西里尔字母最少连续字符数</label>
          <ObsNumberInput
            :model-value="current.min_cyrillic_chars"
            :min="1"
            :max="64"
            @update:model-value="(v) => patchCurrent({ min_cyrillic_chars: v ?? 3 })"
          />
        </div>
      </div>
    </template>

    <!-- 4. FileConstraint -->
    <template v-else-if="current.type === 'file_constraint'">
      <div class="obs-inset p-3 rounded-lg space-y-3">
        <div>
          <label class="block text-[11px] text-slate-400 mb-1">文件约束判定模式</label>
          <ObsSelect
            :model-value="current.mode"
            :options="FILE_MODE_OPTIONS"
            @update:model-value="(v) => patchCurrent({ mode: String(v) as FileConstraintMode })"
          />
        </div>

        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">最小单文件体积</label>
            <ByteSizeInput
              :model-value="current.min_file_size"
              allow-empty
              @update:model-value="(v) => patchCurrent({ min_file_size: v })"
            />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">最大单文件体积</label>
            <ByteSizeInput
              :model-value="current.max_file_size"
              allow-empty
              @update:model-value="(v) => patchCurrent({ max_file_size: v })"
            />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">最短文件名字符数</label>
            <ObsNumberInput
              :model-value="current.min_file_name_length ?? undefined"
              allow-empty
              :min="0"
              placeholder="不限"
              @update:model-value="(v) => patchCurrent({ min_file_name_length: v ?? null })"
            />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">最长文件名字符数</label>
            <ObsNumberInput
              :model-value="current.max_file_name_length ?? undefined"
              allow-empty
              :min="0"
              placeholder="不限"
              @update:model-value="(v) => patchCurrent({ max_file_name_length: v ?? null })"
            />
          </div>
        </div>
      </div>

      <div class="flex flex-wrap items-center gap-1.5" v-if="extensionPacks.length > 0">
        <span class="text-[11px] text-slate-400 mr-1">目标后缀包：</span>
        <button
          v-for="pack in extensionPacks"
          :key="pack.id"
          type="button"
          class="obs-btn obs-btn-ghost px-2 py-1 text-[11px]"
          @click="appendExtPack(pack)"
        >
          + {{ pack.name }}
        </button>
      </div>
      <div class="flex items-center gap-2">
        <input
          v-model="extInput"
          type="text"
          placeholder="追加目标扩展名，如 mkv, mp4, flac"
          class="obs-input px-3 py-1.5 text-xs font-mono flex-1"
          @keyup.enter="addExtsFromInput"
        />
        <button type="button" class="obs-btn obs-btn-primary px-3 py-1.5 text-xs" @click="addExtsFromInput">追加</button>
      </div>
      <div class="obs-inset p-2.5 rounded-lg flex flex-wrap gap-1.5">
        <span
          v-for="ext in current.extensions"
          :key="ext"
          class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-sky-500/10 border border-sky-500/30 text-[11px] font-mono text-sky-200"
        >
          .{{ ext }}
          <button type="button" class="text-sky-400 hover:text-rose-300" @click="removeExt(ext)">
            <X class="w-3 h-3" />
          </button>
        </span>
      </div>
      <div>
        <label class="block text-[11px] text-slate-400 mb-1">附加路径正则（每行 1 条）</label>
        <textarea
          :value="linesToText(current.extra_regexes)"
          rows="2"
          class="obs-input p-2.5 text-xs font-mono"
          @input="(e) => patchCurrent({ extra_regexes: textToLines((e.target as HTMLTextAreaElement).value) })"
        />
      </div>
    </template>

    <!-- 5. SubstantiveContent -->
    <template v-else-if="current.type === 'substantive_content'">
      <div class="obs-inset p-3 rounded-lg space-y-3">
        <label class="inline-flex items-center gap-2 text-xs text-slate-200 cursor-pointer">
          <input
            type="checkbox"
            class="obs-checkbox"
            :checked="current.include_system_junk"
            @change="(e) => patchCurrent({ include_system_junk: (e.target as HTMLInputElement).checked })"
          />
          <span>自动剔除常见系统残留文件 (<code class="font-mono text-slate-400">.DS_Store</code> / <code class="font-mono text-slate-400">Thumbs.db</code> / <code class="font-mono text-slate-400">__MACOSX/</code>)</span>
        </label>

        <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">最少有效实质文件个数</label>
            <ObsNumberInput
              :model-value="current.min_files"
              :min="0"
              @update:model-value="(v) => patchCurrent({ min_files: v ?? 0 })"
            />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">有效实质文件合计最小体积</label>
            <ByteSizeInput
              :model-value="current.min_total_size"
              :min="0"
              @update:model-value="(v) => patchCurrent({ min_total_size: v ?? 0 })"
            />
          </div>
        </div>
      </div>

      <div v-if="extensionPacks.length > 0" class="flex flex-wrap items-center gap-1.5">
        <span class="text-[11px] text-slate-400 mr-1">视为垃圾的扩展名包：</span>
        <button
          v-for="pack in extensionPacks"
          :key="pack.id"
          type="button"
          class="obs-btn obs-btn-ghost px-2 py-1 text-[11px]"
          @click="appendExtPack(pack)"
        >
          + {{ pack.name }}
        </button>
      </div>
      <div class="flex items-center gap-2">
        <input
          v-model="extInput"
          type="text"
          placeholder="追加垃圾文件后缀，如 url, lnk, txt, html"
          class="obs-input px-3 py-1.5 text-xs font-mono flex-1"
          @keyup.enter="addExtsFromInput"
        />
        <button type="button" class="obs-btn obs-btn-primary px-3 py-1.5 text-xs" @click="addExtsFromInput">追加</button>
      </div>
      <div class="obs-inset p-2.5 rounded-lg flex flex-wrap gap-1.5">
        <span
          v-for="ext in current.junk_extensions"
          :key="ext"
          class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-amber-500/10 border border-amber-500/30 text-[11px] font-mono text-amber-200"
        >
          .{{ ext }}
          <button type="button" class="text-amber-400 hover:text-rose-300" @click="removeExt(ext)">
            <X class="w-3 h-3" />
          </button>
        </span>
      </div>

      <div>
        <label class="block text-[11px] text-slate-400 mb-1">附加垃圾文件正则（可选，每行 1 条）</label>
        <textarea
          :value="linesToText(current.extra_junk_regexes)"
          rows="2"
          class="obs-input p-2.5 text-xs font-mono"
          @input="(e) => patchCurrent({ extra_junk_regexes: textToLines((e.target as HTMLTextAreaElement).value) })"
        />
      </div>
    </template>

    <!-- 6. SizeRatio -->
    <template v-else-if="current.type === 'size_ratio'">
      <div class="obs-inset p-3 rounded-lg space-y-3">
        <label class="inline-flex items-center gap-2 text-xs text-slate-200 cursor-pointer">
          <input
            type="checkbox"
            class="obs-checkbox"
            :checked="current.include_split_archives"
            @change="(e) => patchCurrent({ include_split_archives: (e.target as HTMLInputElement).checked })"
          />
          <span>自动计入分卷压缩格式 (<code class="font-mono text-slate-400">.part1.rar</code> / <code class="font-mono text-slate-400">.zip.001</code>)</span>
        </label>

        <div>
          <div class="flex items-center justify-between text-xs mb-1">
            <span class="text-slate-300">体积占比阈值上限</span>
            <span class="font-mono font-semibold text-emerald-300">{{ (current.max_ratio * 100).toFixed(0) }}%</span>
          </div>
          <input
            type="range"
            min="5"
            max="100"
            step="5"
            class="w-full accent-emerald-500"
            :value="Math.round(current.max_ratio * 100)"
            @input="(e) => patchCurrent({ max_ratio: Number((e.target as HTMLInputElement).value) / 100 })"
          />
        </div>
      </div>

      <div v-if="extensionPacks.length > 0" class="flex flex-wrap items-center gap-1.5">
        <span class="text-[11px] text-slate-400 mr-1">统计目标后缀：</span>
        <button
          v-for="pack in extensionPacks"
          :key="pack.id"
          type="button"
          class="obs-btn obs-btn-ghost px-2 py-1 text-[11px]"
          @click="appendExtPack(pack)"
        >
          + {{ pack.name }}
        </button>
      </div>
      <div class="flex items-center gap-2">
        <input
          v-model="extInput"
          type="text"
          placeholder="追加后缀，如 zip, rar, 7z, iso"
          class="obs-input px-3 py-1.5 text-xs font-mono flex-1"
          @keyup.enter="addExtsFromInput"
        />
        <button type="button" class="obs-btn obs-btn-primary px-3 py-1.5 text-xs" @click="addExtsFromInput">追加</button>
      </div>
      <div class="obs-inset p-2.5 rounded-lg flex flex-wrap gap-1.5">
        <span
          v-for="ext in current.extensions"
          :key="ext"
          class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-slate-800 border border-slate-700 text-[11px] font-mono text-slate-200"
        >
          .{{ ext }}
          <button type="button" class="text-slate-400 hover:text-rose-300" @click="removeExt(ext)">
            <X class="w-3 h-3" />
          </button>
        </span>
      </div>

      <div>
        <label class="block text-[11px] text-slate-400 mb-1">附加路径正则（可选，每行 1 条）</label>
        <textarea
          :value="linesToText(current.extra_regexes)"
          rows="2"
          class="obs-input p-2.5 text-xs font-mono"
          @input="(e) => patchCurrent({ extra_regexes: textToLines((e.target as HTMLTextAreaElement).value) })"
        />
      </div>
    </template>

    <!-- 7. CustomRegex -->
    <template v-else-if="current.type === 'custom_regex'">
      <div class="obs-inset p-3 rounded-lg grid grid-cols-1 md:grid-cols-2 gap-3">
        <div>
          <label class="block text-[11px] text-slate-400 mb-1">匹配目标</label>
          <ObsSelect
            :model-value="current.target"
            :options="TARGET_OPTIONS"
            @update:model-value="(v) => patchCurrent({ target: String(v) as TextTarget })"
          />
        </div>
        <div>
          <label class="block text-[11px] text-slate-400 mb-1">多条正则组合逻辑</label>
          <div class="bg-[#090d16] border border-[#1e293b] p-0.5 rounded-lg inline-flex w-full">
            <button
              type="button"
              class="flex-1 py-1.5 rounded text-[11px] transition"
              :class="!current.match_all ? 'bg-emerald-500/15 text-emerald-300' : 'text-slate-400'"
              @click="patchCurrent({ match_all: false })"
            >
              任一条命中 (OR)
            </button>
            <button
              type="button"
              class="flex-1 py-1.5 rounded text-[11px] transition"
              :class="current.match_all ? 'bg-emerald-500/15 text-emerald-300' : 'text-slate-400'"
              @click="patchCurrent({ match_all: true })"
            >
              全部命中 (AND)
            </button>
          </div>
        </div>
      </div>

      <div>
        <label class="block text-[11px] text-slate-400 mb-1">Rust 正则表达式列表（每行 1 条）</label>
        <textarea
          :value="linesToText(current.patterns)"
          rows="4"
          placeholder="(?i)\bexample\b"
          class="obs-input p-2.5 text-xs font-mono"
          @input="(e) => patchCurrent({ patterns: textToLines((e.target as HTMLTextAreaElement).value) })"
        />
      </div>
    </template>

    <!-- 8. Composite -->
    <template v-else-if="current.type === 'composite' && allowComposite">
      <div class="obs-inset p-3 rounded-lg border-emerald-500/25 flex flex-wrap items-center justify-between gap-2">
        <div class="flex items-center gap-2">
          <span class="font-medium text-emerald-300">复合逻辑：</span>
          <div class="bg-[#090d16] border border-[#1e293b] p-0.5 rounded-lg inline-flex">
            <button
              type="button"
              class="px-2.5 py-1 rounded text-[11px] transition"
              :class="current.logic === 'all' ? 'bg-emerald-500/15 text-emerald-300' : 'text-slate-400'"
              @click="updateMatcher({ ...current, logic: 'all' as CompositeLogic })"
            >
              全部满足 (AND)
            </button>
            <button
              type="button"
              class="px-2.5 py-1 rounded text-[11px] transition"
              :class="current.logic === 'any' ? 'bg-emerald-500/15 text-emerald-300' : 'text-slate-400'"
              @click="updateMatcher({ ...current, logic: 'any' as CompositeLogic })"
            >
              满足任一 (OR)
            </button>
          </div>
        </div>

        <div class="flex items-center gap-1.5">
          <ObsSelect
            v-model="childTypeToAdd"
            :options="CHILD_TYPE_OPTIONS"
            trigger-class="text-[11px] py-1 min-w-[140px]"
          />
          <button type="button" class="obs-btn obs-btn-primary px-2.5 py-1 text-[11px] inline-flex items-center gap-1" @click="addCompositeCondition">
            <Plus class="w-3 h-3" />
            添加子条件
          </button>
        </div>
      </div>

      <div v-if="current.conditions.length === 0" class="py-6 text-center text-xs text-slate-500 border border-dashed border-[#1e293b] rounded-lg">
        暂无子条件，可上方选择类型添加（适合精准组合判断）
      </div>

      <div class="space-y-3">
        <div
          v-for="(sub, idx) in current.conditions"
          :key="idx"
          class="obs-inset p-3 rounded-lg space-y-2"
        >
          <div class="flex items-center justify-between border-b border-[#1e293b] pb-2">
            <div class="flex items-center gap-2">
              <span class="font-mono font-semibold text-slate-200">子条件 #{{ idx + 1 }}</span>
              <span class="px-2 py-0.5 rounded bg-sky-500/10 border border-sky-500/25 text-[10px] text-sky-300">
                {{ MATCHER_TYPE_LABELS[sub.type] }}
              </span>
            </div>
            <div class="flex items-center gap-1">
              <button type="button" class="obs-btn obs-btn-ghost p-1" :disabled="idx === 0" @click="moveCompositeChild(idx, -1)">
                <ArrowUp class="w-3 h-3" />
              </button>
              <button type="button" class="obs-btn obs-btn-ghost p-1" :disabled="idx === current.conditions.length - 1" @click="moveCompositeChild(idx, 1)">
                <ArrowDown class="w-3 h-3" />
              </button>
              <button type="button" class="obs-btn obs-btn-danger p-1" @click="removeCompositeChild(idx)">
                <Trash2 class="w-3 h-3" />
              </button>
            </div>
          </div>

          <MatcherEditor
            :model-value="sub"
            :extension-packs="extensionPacks"
            :keyword-packs="keywordPacks"
            :allow-composite="false"
            @update:model-value="(v) => updateCompositeChild(idx, v)"
          />
        </div>
      </div>
    </template>
  </div>
</template>
