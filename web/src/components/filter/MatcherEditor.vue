<script setup lang="ts">
import { computed, ref } from "vue";
import { ArrowDown, ArrowUp, Plus, Trash2 } from "lucide-vue-next";
import ByteSizeInput from "./ByteSizeInput.vue";
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

const props = withDefaults(
  defineProps<{
    modelValue: RuleMatcher;
    extensionPacks: ExtensionPackInfo[];
    keywordPacks: KeywordPackInfo[];
    allowComposite?: boolean;
  }>(),
  {
    allowComposite: true,
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: RuleMatcher): void;
}>();

const TITLE_PATTERNS: Array<{ kind: TitlePatternKind; label: string; example: string }> = [
  { kind: "cyrillic", label: "西里尔字母文本", example: "русский фильм" },
  { kind: "digits_only", label: "纯数字标题", example: "12345678" },
  { kind: "sha1_hex", label: "40 位 SHA1 哈希", example: "a94a8fe5ccb19ba61c4c..." },
  { kind: "md5_hex", label: "32 位 MD5 哈希", example: "d41d8cd98f00b204e980..." },
  { kind: "uuid", label: "UUID 格式编号", example: "550e8400-e29b-41d4..." },
  { kind: "decimal_code", label: "纯小数编号", example: "123.456" },
  { kind: "magnet_prefix", label: "magnet:? 磁力前缀", example: "magnet:?xt=urn:..." },
  { kind: "http_url_prefix", label: "http(s):// 链接前缀", example: "https://example.com/..." },
  { kind: "split_archive_suffix", label: "分卷压缩包后缀", example: ".part1.rar / .zip.001 / .r01" },
];

const wordInput = ref("");
const wordFilter = ref("");
const extInput = ref("");

const current = computed(() => props.modelValue);

function patch(updater: (draft: RuleMatcher) => void) {
  const copy = JSON.parse(JSON.stringify(props.modelValue)) as RuleMatcher;
  updater(copy);
  emit("update:modelValue", copy);
}

function normalizeExt(raw: string): string {
  return raw.trim().replace(/^\.+/, "").toLowerCase();
}

const filteredWords = computed(() => {
  if (current.value.type !== "keyword_group") return [];
  const q = wordFilter.value.trim().toLowerCase();
  if (!q) return current.value.words;
  return current.value.words.filter((w) => w.toLowerCase().includes(q));
});

function addWordsFromInput() {
  if (current.value.type !== "keyword_group") return;
  const parts = wordInput.value
    .split(/[\r\n,，]+/)
    .map((s) => s.trim())
    .filter(Boolean);
  if (parts.length === 0) return;
  patch((d) => {
    if (d.type !== "keyword_group") return;
    const seen = new Set(d.words.map((w) => (d.case_sensitive ? w : w.toLowerCase())));
    for (const p of parts) {
      const k = d.case_sensitive ? p : p.toLowerCase();
      if (!seen.has(k)) {
        seen.add(k);
        d.words.push(p);
      }
    }
  });
  wordInput.value = "";
}

function appendKeywordPack(pack: KeywordPackInfo) {
  patch((d) => {
    if (d.type !== "keyword_group") return;
    const seen = new Set(d.words.map((w) => (d.case_sensitive ? w : w.toLowerCase())));
    for (const w of pack.words) {
      const k = d.case_sensitive ? w : w.toLowerCase();
      if (!seen.has(k)) {
        seen.add(k);
        d.words.push(w);
      }
    }
  });
}

function removeWord(word: string) {
  patch((d) => {
    if (d.type !== "keyword_group") return;
    d.words = d.words.filter((w) => w !== word);
  });
}

function getExtList(m: RuleMatcher): string[] {
  if (m.type === "extension_rule" || m.type === "file_constraint" || m.type === "size_ratio") {
    return m.extensions;
  }
  if (m.type === "substantive_content") {
    return m.junk_extensions;
  }
  return [];
}

function setExtList(d: RuleMatcher, next: string[]) {
  if (d.type === "extension_rule" || d.type === "file_constraint" || d.type === "size_ratio") {
    d.extensions = next;
  } else if (d.type === "substantive_content") {
    d.junk_extensions = next;
  }
}

function addExtsFromInput() {
  const parts = extInput.value
    .split(/[\s,，;；\r\n]+/)
    .map(normalizeExt)
    .filter(Boolean);
  if (parts.length === 0) return;
  patch((d) => {
    const list = getExtList(d);
    const seen = new Set(list.map(normalizeExt));
    const next = [...list];
    for (const p of parts) {
      if (!seen.has(p)) {
        seen.add(p);
        next.push(p);
      }
    }
    setExtList(d, next);
  });
  extInput.value = "";
}

function appendExtPack(pack: ExtensionPackInfo) {
  patch((d) => {
    const list = getExtList(d);
    const seen = new Set(list.map(normalizeExt));
    const next = [...list];
    for (const ext of pack.extensions.map(normalizeExt)) {
      if (ext && !seen.has(ext)) {
        seen.add(ext);
        next.push(ext);
      }
    }
    setExtList(d, next);
  });
}

function removeExt(ext: string) {
  patch((d) => {
    setExtList(
      d,
      getExtList(d).filter((x) => x !== ext),
    );
  });
}

function toggleTitlePattern(kind: TitlePatternKind, checked: boolean) {
  patch((d) => {
    if (d.type !== "title_pattern_preset") return;
    if (checked) {
      if (!d.patterns.includes(kind)) d.patterns.push(kind);
    } else {
      d.patterns = d.patterns.filter((k) => k !== kind);
    }
  });
}

function linesToText(lines: string[]): string {
  return lines.join("\n");
}

function textToLines(raw: string): string[] {
  return raw
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean);
}

function createDefaultChildMatcher(kind: RuleMatcher["type"]): RuleMatcher {
  switch (kind) {
    case "keyword_group":
      return {
        type: "keyword_group",
        target: "refined_name",
        match_mode: "word_boundary",
        case_sensitive: false,
        words: [],
      };
    case "extension_rule":
      return {
        type: "extension_rule",
        scope: "any_file",
        extensions: [],
      };
    case "title_pattern_preset":
      return {
        type: "title_pattern_preset",
        patterns: [],
        min_cyrillic_chars: 2,
      };
    case "file_constraint":
      return {
        type: "file_constraint",
        extensions: ["mkv", "mp4"],
        extra_regexes: [],
        mode: "at_least_one_matched",
        min_file_size: 100 * 1024 * 1024,
        max_file_size: null,
        min_file_name_length: null,
        max_file_name_length: null,
      };
    case "substantive_content":
      return {
        type: "substantive_content",
        junk_extensions: ["url", "lnk", "txt"],
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
        max_ratio: 0.7,
      };
    default:
      return {
        type: "custom_regex",
        target: "raw_name",
        match_all: false,
        patterns: [],
      };
  }
}

function addCompositeCondition(kind: RuleMatcher["type"]) {
  patch((d) => {
    if (d.type !== "composite") return;
    d.conditions.push(createDefaultChildMatcher(kind));
  });
}

function updateCompositeChild(idx: number, next: RuleMatcher) {
  patch((d) => {
    if (d.type !== "composite") return;
    d.conditions[idx] = next;
  });
}

function removeCompositeChild(idx: number) {
  patch((d) => {
    if (d.type !== "composite") return;
    d.conditions.splice(idx, 1);
  });
}

function moveCompositeChild(idx: number, delta: -1 | 1) {
  patch((d) => {
    if (d.type !== "composite") return;
    const nextIdx = idx + delta;
    if (nextIdx < 0 || nextIdx >= d.conditions.length) return;
    const [item] = d.conditions.splice(idx, 1);
    d.conditions.splice(nextIdx, 0, item);
  });
}

function setKeywordTarget(v: unknown) {
  patch((d) => {
    if (d.type === "keyword_group") d.target = v as TextTarget;
  });
}

function setKeywordMode(v: unknown) {
  patch((d) => {
    if (d.type === "keyword_group") d.match_mode = v as KeywordMatchMode;
  });
}

function setKeywordCase(v: unknown) {
  patch((d) => {
    if (d.type === "keyword_group") d.case_sensitive = Boolean(v);
  });
}

function setExtensionScope(v: unknown) {
  patch((d) => {
    if (d.type === "extension_rule") d.scope = v as ExtensionScope;
  });
}

function setMinCyrillic(v: number | undefined) {
  patch((d) => {
    if (d.type === "title_pattern_preset") d.min_cyrillic_chars = Number(v || 2);
  });
}

function setFileConstraintMode(v: unknown) {
  patch((d) => {
    if (d.type === "file_constraint") d.mode = v as FileConstraintMode;
  });
}

function toggleFileSizeLimit(field: "min_file_size" | "max_file_size", enabled: unknown, fallback: number) {
  patch((d) => {
    if (d.type === "file_constraint") d[field] = enabled ? fallback : null;
  });
}

function setFileSizeLimit(field: "min_file_size" | "max_file_size", val: number) {
  patch((d) => {
    if (d.type === "file_constraint") d[field] = val;
  });
}

function toggleFileNameLenLimit(
  field: "min_file_name_length" | "max_file_name_length",
  enabled: unknown,
  fallback: number,
) {
  patch((d) => {
    if (d.type === "file_constraint") d[field] = enabled ? fallback : null;
  });
}

function setFileNameLenLimit(
  field: "min_file_name_length" | "max_file_name_length",
  val: number | undefined,
) {
  patch((d) => {
    if (d.type === "file_constraint") d[field] = Number(val || 1);
  });
}

function setFileExtraRegexes(v: string) {
  patch((d) => {
    if (d.type === "file_constraint") d.extra_regexes = textToLines(v);
  });
}

function setSubstantiveSysJunk(v: unknown) {
  patch((d) => {
    if (d.type === "substantive_content") d.include_system_junk = Boolean(v);
  });
}

function setSubstantiveMinFiles(v: number | undefined) {
  patch((d) => {
    if (d.type === "substantive_content") d.min_files = Number(v || 0);
  });
}

function setSubstantiveMinSize(v: number) {
  patch((d) => {
    if (d.type === "substantive_content") d.min_total_size = v;
  });
}

function setSubstantiveExtraRegexes(v: string) {
  patch((d) => {
    if (d.type === "substantive_content") d.extra_junk_regexes = textToLines(v);
  });
}

function setSizeRatioSplit(v: unknown) {
  patch((d) => {
    if (d.type === "size_ratio") d.include_split_archives = Boolean(v);
  });
}

function setSizeRatioPct(v: number | number[]) {
  const num = Array.isArray(v) ? (v[0] ?? 70) : v;
  patch((d) => {
    if (d.type === "size_ratio") d.max_ratio = Number(num) / 100;
  });
}

function setCustomRegexTarget(v: unknown) {
  patch((d) => {
    if (d.type === "custom_regex") d.target = v as TextTarget;
  });
}

function setCustomRegexMatchAll(v: unknown) {
  patch((d) => {
    if (d.type === "custom_regex") d.match_all = Boolean(v);
  });
}

function setCustomRegexPatterns(v: string) {
  patch((d) => {
    if (d.type === "custom_regex") d.patterns = textToLines(v);
  });
}

function setCompositeLogic(v: unknown) {
  patch((d) => {
    if (d.type === "composite") d.logic = v as CompositeLogic;
  });
}

function matcherLabel(t: RuleMatcher["type"]): string {
  const map: Record<RuleMatcher["type"], string> = {
    keyword_group: "关键词词组",
    extension_rule: "扩展名后缀集合",
    title_pattern_preset: "标题结构特征开关",
    file_constraint: "文件体积/文件名长度约束",
    substantive_content: "去噪后有效实质内容",
    size_ratio: "指定类型体积占比",
    custom_regex: "专家自定义正则",
    composite: "多条件复合逻辑",
  };
  return map[t];
}
</script>

<template>
  <div class="space-y-4">
    <!-- 1. KeywordGroup -->
    <template v-if="current.type === 'keyword_group'">
      <div class="grid grid-cols-1 md:grid-cols-3 gap-3 rounded-lg bg-slate-50 border border-slate-200 p-3">
        <div>
          <div class="text-xs font-medium text-slate-600 mb-1">匹配目标文本</div>
          <el-select
            :model-value="current.target"
            size="small"
            class="w-full"
            @change="setKeywordTarget"
          >
            <el-option label="原始标题 (raw_name)" value="raw_name" />
            <el-option label="清洗后标题 (refined_name)" value="refined_name" />
            <el-option label="任一文件完整路径" value="any_file_path" />
            <el-option label="任一单文件名" value="any_file_name" />
          </el-select>
        </div>

        <div>
          <div class="text-xs font-medium text-slate-600 mb-1">匹配方式</div>
          <el-select
            :model-value="current.match_mode"
            size="small"
            class="w-full"
            @change="setKeywordMode"
          >
            <el-option label="包含子串（兼容旧版行为）" value="contains" />
            <el-option label="整词边界（英文防误杀如 Shack/Dispatch）" value="word_boundary" />
          </el-select>
        </div>

        <div class="flex flex-col justify-center">
          <div class="text-xs font-medium text-slate-600 mb-1">大小写敏感</div>
          <el-switch
            :model-value="current.case_sensitive"
            active-text="区分大小写"
            inactive-text="忽略大小写"
            @change="setKeywordCase"
          />
        </div>
      </div>

      <div class="space-y-2">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <span class="text-xs font-medium text-slate-700">快捷追加内置词库：</span>
          <div class="flex flex-wrap gap-1.5">
            <el-button
              v-for="pack in keywordPacks"
              :key="pack.id"
              size="small"
              plain
              @click="appendKeywordPack(pack)"
            >
              + {{ pack.name }}
            </el-button>
          </div>
        </div>

        <div class="flex flex-wrap items-center gap-2">
          <el-input
            v-model="wordInput"
            size="small"
            placeholder="输入关键词并回车（支持英文逗号或换行批量粘贴）"
            class="flex-1 min-w-[220px]"
            @keyup.enter="addWordsFromInput"
          />
          <el-button size="small" type="primary" @click="addWordsFromInput">
            <Plus class="h-3.5 w-3.5 mr-1" />
            追加关键词
          </el-button>
          <el-input
            v-model="wordFilter"
            size="small"
            clearable
            placeholder="筛选已有词..."
            style="width: 150px"
          />
        </div>

        <div class="rounded-lg border border-slate-200 p-3 max-h-56 overflow-y-auto bg-white">
          <div v-if="filteredWords.length === 0" class="text-xs text-slate-400 text-center py-4">
            暂无关键词，可在上方输入或点击内置词库按钮追加
          </div>
          <div v-else class="flex flex-wrap gap-1.5">
            <el-tag
              v-for="w in filteredWords"
              :key="w"
              closable
              size="small"
              effect="plain"
              @close="removeWord(w)"
            >
              {{ w }}
            </el-tag>
          </div>
        </div>
        <div class="flex justify-between text-xs text-slate-400">
          <span>当前共 {{ current.words.length }} 个关键词</span>
          <el-button
            v-if="current.words.length > 0"
            size="small"
            type="danger"
            link
            @click="patch((d) => d.type === 'keyword_group' && (d.words = []))"
          >
            清空列表
          </el-button>
        </div>
      </div>
    </template>

    <!-- 2. ExtensionRule -->
    <template v-else-if="current.type === 'extension_rule'">
      <div class="rounded-lg bg-slate-50 border border-slate-200 p-3">
        <div class="text-xs font-medium text-slate-600 mb-2">作用范围</div>
        <el-radio-group
          :model-value="current.scope"
          size="small"
          @change="setExtensionScope"
        >
          <el-radio-button value="torrent_name">种子标题末尾后缀</el-radio-button>
          <el-radio-button value="any_file">种子内包含任意该后缀文件</el-radio-button>
          <el-radio-button value="all_files">种子内全部文件均为此后缀</el-radio-button>
        </el-radio-group>
      </div>

      <div class="space-y-2">
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="text-xs font-medium text-slate-700 mr-1">一键加入后缀包：</span>
          <el-button
            v-for="pack in extensionPacks"
            :key="pack.id"
            size="small"
            plain
            @click="appendExtPack(pack)"
          >
            + {{ pack.name }}
          </el-button>
        </div>

        <div class="flex items-center gap-2">
          <el-input
            v-model="extInput"
            size="small"
            placeholder="输入扩展名（自动去除前导点号与大写，支持空格或逗号分隔，如 exe, msi, .bat）"
            class="flex-1 font-mono"
            @keyup.enter="addExtsFromInput"
          />
          <el-button size="small" type="primary" @click="addExtsFromInput">
            <Plus class="h-3.5 w-3.5 mr-1" />
            添加后缀
          </el-button>
        </div>

        <div class="rounded-lg border border-slate-200 p-3 max-h-52 overflow-y-auto bg-white">
          <div v-if="current.extensions.length === 0" class="text-xs text-slate-400 text-center py-4">
            暂无后缀，请点击上方后缀包或手动输入
          </div>
          <div v-else class="flex flex-wrap gap-1.5">
            <el-tag
              v-for="ext in current.extensions"
              :key="ext"
              closable
              size="small"
              type="info"
              effect="plain"
              class="font-mono"
              @close="removeExt(ext)"
            >
              .{{ ext }}
            </el-tag>
          </div>
        </div>
        <div class="flex justify-between text-xs text-slate-400">
          <span>O(1) 哈希集合精确匹配，无需编写正则 · 共 {{ current.extensions.length }} 个后缀</span>
          <el-button
            v-if="current.extensions.length > 0"
            size="small"
            type="danger"
            link
            @click="patch((d) => d.type === 'extension_rule' && (d.extensions = []))"
          >
            清空后缀
          </el-button>
        </div>
      </div>
    </template>

    <!-- 3. TitlePatternPreset -->
    <template v-else-if="current.type === 'title_pattern_preset'">
      <div class="grid grid-cols-1 md:grid-cols-2 gap-2.5">
        <div
          v-for="item in TITLE_PATTERNS"
          :key="item.kind"
          class="rounded-lg border border-slate-200 p-2.5 bg-white flex items-start justify-between gap-2"
        >
          <el-checkbox
            :model-value="current.patterns.includes(item.kind)"
            @change="(val: string | number | boolean) => toggleTitlePattern(item.kind, Boolean(val))"
          >
            <div>
              <div class="text-xs font-medium text-slate-800">{{ item.label }}</div>
              <div class="text-[11px] text-slate-400 font-mono">示例: {{ item.example }}</div>
            </div>
          </el-checkbox>
        </div>
      </div>

      <div
        v-if="current.patterns.includes('cyrillic')"
        class="flex items-center gap-3 rounded-lg bg-slate-50 border border-slate-200 px-3 py-2 text-xs"
      >
        <span class="text-slate-600">连续西里尔字符触发阈值：</span>
        <el-input-number
          :model-value="current.min_cyrillic_chars"
          :min="1"
          :max="20"
          size="small"
          @update:model-value="setMinCyrillic"
        />
        <span class="text-slate-400">个连续字符（默认 2）</span>
      </div>
    </template>

    <!-- 4. FileConstraint -->
    <template v-else-if="current.type === 'file_constraint'">
      <div class="rounded-lg bg-slate-50 border border-slate-200 p-3 space-y-2.5">
        <div class="text-xs font-medium text-slate-600">评估模式（多文件种子关键防误杀项）</div>
        <el-radio-group
          :model-value="current.mode"
          size="small"
          @change="setFileConstraintMode"
        >
          <el-radio value="every_matched">每个匹配文件都必须达标（适合严格剔除任何小广告视频）</el-radio>
          <el-radio value="at_least_one_matched">至少有 1 个主文件达标（容忍电影内含预告片/Sample 花絮）</el-radio>
        </el-radio-group>
      </div>

      <div class="space-y-2">
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="text-xs font-medium text-slate-700 mr-1">目标文件后缀：</span>
          <el-button
            v-for="pack in extensionPacks"
            :key="pack.id"
            size="small"
            plain
            @click="appendExtPack(pack)"
          >
            + {{ pack.name }}
          </el-button>
        </div>
        <div class="flex items-center gap-2">
          <el-input
            v-model="extInput"
            size="small"
            placeholder="追加后缀，如 mp4, mkv, flac"
            class="flex-1 font-mono"
            @keyup.enter="addExtsFromInput"
          />
          <el-button size="small" type="primary" @click="addExtsFromInput">追加</el-button>
        </div>
        <div class="flex flex-wrap gap-1.5 rounded-lg border border-slate-200 p-2.5 bg-white">
          <span v-if="current.extensions.length === 0" class="text-xs text-slate-400">
            未选后缀（可补充下方附加正则）
          </span>
          <el-tag
            v-for="ext in current.extensions"
            :key="ext"
            closable
            size="small"
            type="info"
            effect="plain"
            class="font-mono"
            @close="removeExt(ext)"
          >
            .{{ ext }}
          </el-tag>
        </div>
      </div>

      <div class="grid grid-cols-1 md:grid-cols-2 gap-3 rounded-lg border border-slate-200 p-3 bg-white text-xs">
        <div class="space-y-1.5">
          <div class="flex items-center justify-between">
            <span class="font-medium text-slate-700">单文件最小体积</span>
            <el-switch
              :model-value="current.min_file_size !== null"
              size="small"
              @change="(v: string | number | boolean) => toggleFileSizeLimit('min_file_size', v, 10 * 1024 * 1024)"
            />
          </div>
          <ByteSizeInput
            v-if="current.min_file_size !== null"
            :model-value="current.min_file_size"
            :min="1"
            @update:model-value="(v: number) => setFileSizeLimit('min_file_size', v)"
          />
          <div v-else class="text-slate-400">未限制</div>
        </div>

        <div class="space-y-1.5">
          <div class="flex items-center justify-between">
            <span class="font-medium text-slate-700">单文件最大体积</span>
            <el-switch
              :model-value="current.max_file_size !== null"
              size="small"
              @change="(v: string | number | boolean) => toggleFileSizeLimit('max_file_size', v, 100 * 1024 * 1024 * 1024)"
            />
          </div>
          <ByteSizeInput
            v-if="current.max_file_size !== null"
            :model-value="current.max_file_size"
            :min="1"
            @update:model-value="(v: number) => setFileSizeLimit('max_file_size', v)"
          />
          <div v-else class="text-slate-400">未限制</div>
        </div>

        <div class="space-y-1.5">
          <div class="flex items-center justify-between">
            <span class="font-medium text-slate-700">单文件名最短字符数</span>
            <el-switch
              :model-value="current.min_file_name_length !== null"
              size="small"
              @change="(v: string | number | boolean) => toggleFileNameLenLimit('min_file_name_length', v, 4)"
            />
          </div>
          <el-input-number
            v-if="current.min_file_name_length !== null"
            :model-value="current.min_file_name_length"
            :min="1"
            size="small"
            @update:model-value="(v: number | undefined) => setFileNameLenLimit('min_file_name_length', v)"
          />
          <div v-else class="text-slate-400">未限制</div>
        </div>

        <div class="space-y-1.5">
          <div class="flex items-center justify-between">
            <span class="font-medium text-slate-700">单文件名最长字符数</span>
            <el-switch
              :model-value="current.max_file_name_length !== null"
              size="small"
              @change="(v: string | number | boolean) => toggleFileNameLenLimit('max_file_name_length', v, 128)"
            />
          </div>
          <el-input-number
            v-if="current.max_file_name_length !== null"
            :model-value="current.max_file_name_length"
            :min="1"
            size="small"
            @update:model-value="(v: number | undefined) => setFileNameLenLimit('max_file_name_length', v)"
          />
          <div v-else class="text-slate-400">未限制</div>
        </div>
      </div>

      <div>
        <div class="text-xs font-medium text-slate-600 mb-1">附加文件匹配正则（可选，每行 1 条）</div>
        <el-input
          :model-value="linesToText(current.extra_regexes)"
          type="textarea"
          :rows="2"
          class="font-mono text-xs"
          placeholder="常规扩展名请优先填上方后缀芯片；仅特殊路径匹配才需填此处"
          @update:model-value="setFileExtraRegexes"
        />
      </div>
    </template>

    <!-- 5. SubstantiveContent -->
    <template v-else-if="current.type === 'substantive_content'">
      <div class="rounded-lg border border-slate-200 p-3 bg-slate-50 space-y-2.5">
        <el-checkbox
          :model-value="current.include_system_junk"
          @change="setSubstantiveSysJunk"
        >
          <span class="text-xs font-medium text-slate-800">
            自动剔除常见系统残留文件（<code class="font-mono">.DS_Store</code>、<code class="font-mono">Thumbs.db</code>、<code class="font-mono">desktop.ini</code>、<code class="font-mono">__MACOSX/</code>、<code class="font-mono">._*</code>）
          </span>
        </el-checkbox>

        <div class="grid grid-cols-1 md:grid-cols-2 gap-3 pt-1">
          <div>
            <div class="text-xs text-slate-600 mb-1">最少有效实质文件个数</div>
            <el-input-number
              :model-value="current.min_files"
              :min="0"
              size="small"
              @update:model-value="setSubstantiveMinFiles"
            />
          </div>
          <div>
            <div class="text-xs text-slate-600 mb-1">有效实质文件合计最小体积</div>
            <ByteSizeInput
              :model-value="current.min_total_size"
              :min="0"
              @update:model-value="setSubstantiveMinSize"
            />
          </div>
        </div>
      </div>

      <div class="space-y-2">
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="text-xs font-medium text-slate-700 mr-1">视为垃圾文件的扩展名：</span>
          <el-button
            v-for="pack in extensionPacks"
            :key="pack.id"
            size="small"
            plain
            @click="appendExtPack(pack)"
          >
            + {{ pack.name }}
          </el-button>
        </div>
        <div class="flex items-center gap-2">
          <el-input
            v-model="extInput"
            size="small"
            placeholder="追加垃圾文件后缀，如 url, lnk, txt, html"
            class="flex-1 font-mono"
            @keyup.enter="addExtsFromInput"
          />
          <el-button size="small" type="primary" @click="addExtsFromInput">追加</el-button>
        </div>
        <div class="flex flex-wrap gap-1.5 rounded-lg border border-slate-200 p-2.5 bg-white">
          <el-tag
            v-for="ext in current.junk_extensions"
            :key="ext"
            closable
            size="small"
            type="warning"
            effect="plain"
            class="font-mono"
            @close="removeExt(ext)"
          >
            .{{ ext }}
          </el-tag>
          <span v-if="current.junk_extensions.length === 0" class="text-xs text-slate-400">
            未指定垃圾扩展名
          </span>
        </div>
      </div>

      <div>
        <div class="text-xs font-medium text-slate-600 mb-1">附加垃圾文件正则（可选，每行 1 条）</div>
        <el-input
          :model-value="linesToText(current.extra_junk_regexes)"
          type="textarea"
          :rows="2"
          class="font-mono text-xs"
          @update:model-value="setSubstantiveExtraRegexes"
        />
      </div>
    </template>

    <!-- 6. SizeRatio -->
    <template v-else-if="current.type === 'size_ratio'">
      <div class="rounded-lg border border-slate-200 p-3 bg-slate-50 space-y-3">
        <el-checkbox
          :model-value="current.include_split_archives"
          @change="setSizeRatioSplit"
        >
          <span class="text-xs font-medium text-slate-800">
            自动计入常见分卷压缩格式（<code class="font-mono">.part1.rar</code>、<code class="font-mono">.zip.001</code>、<code class="font-mono">.r01/.z01</code>）
          </span>
        </el-checkbox>

        <div>
          <div class="flex items-center justify-between text-xs mb-1">
            <span class="font-medium text-slate-700">体积占比阈值上限</span>
            <span class="font-mono font-semibold text-indigo-700">
              {{ (current.max_ratio * 100).toFixed(0) }}%
            </span>
          </div>
          <el-slider
            :model-value="Math.round(current.max_ratio * 100)"
            :min="5"
            :max="100"
            :step="5"
            @update:model-value="setSizeRatioPct"
          />
          <div class="text-[11px] text-slate-500">
            Reject 模式下：当命中文件体积总和 >= 整种总体积的 {{ (current.max_ratio * 100).toFixed(0) }}% 时触发拦截；Allow 模式下则要求小于该占比。
          </div>
        </div>
      </div>

      <div class="space-y-2">
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="text-xs font-medium text-slate-700 mr-1">统计目标后缀：</span>
          <el-button
            v-for="pack in extensionPacks"
            :key="pack.id"
            size="small"
            plain
            @click="appendExtPack(pack)"
          >
            + {{ pack.name }}
          </el-button>
        </div>
        <div class="flex items-center gap-2">
          <el-input
            v-model="extInput"
            size="small"
            placeholder="追加后缀，如 zip, rar, 7z, iso"
            class="flex-1 font-mono"
            @keyup.enter="addExtsFromInput"
          />
          <el-button size="small" type="primary" @click="addExtsFromInput">追加</el-button>
        </div>
        <div class="flex flex-wrap gap-1.5 rounded-lg border border-slate-200 p-2.5 bg-white">
          <el-tag
            v-for="ext in current.extensions"
            :key="ext"
            closable
            size="small"
            type="info"
            effect="plain"
            class="font-mono"
            @close="removeExt(ext)"
          >
            .{{ ext }}
          </el-tag>
        </div>
      </div>
    </template>

    <!-- 7. CustomRegex -->
    <template v-else-if="current.type === 'custom_regex'">
      <div class="grid grid-cols-1 md:grid-cols-2 gap-3 rounded-lg bg-slate-50 border border-slate-200 p-3">
        <div>
          <div class="text-xs font-medium text-slate-600 mb-1">匹配目标</div>
          <el-select
            :model-value="current.target"
            size="small"
            class="w-full"
            @change="setCustomRegexTarget"
          >
            <el-option label="原始标题 (raw_name)" value="raw_name" />
            <el-option label="清洗后标题 (refined_name)" value="refined_name" />
            <el-option label="任一文件完整路径" value="any_file_path" />
            <el-option label="任一单文件名" value="any_file_name" />
          </el-select>
        </div>
        <div>
          <div class="text-xs font-medium text-slate-600 mb-1">多条正则组合逻辑</div>
          <el-radio-group
            :model-value="current.match_all"
            size="small"
            @change="setCustomRegexMatchAll"
          >
            <el-radio-button :value="false">任一条命中 (OR)</el-radio-button>
            <el-radio-button :value="true">全部命中 (AND)</el-radio-button>
          </el-radio-group>
        </div>
      </div>

      <div>
        <div class="text-xs font-medium text-slate-600 mb-1">Rust 正则表达式列表（每行 1 条）</div>
        <el-input
          :model-value="linesToText(current.patterns)"
          type="textarea"
          :rows="4"
          class="font-mono text-xs"
          placeholder="(?i)\bexample\b"
          @update:model-value="setCustomRegexPatterns"
        />
      </div>
    </template>

    <!-- 8. Composite -->
    <template v-else-if="current.type === 'composite' && allowComposite">
      <div class="flex flex-wrap items-center justify-between gap-2 rounded-lg bg-indigo-50/60 border border-indigo-200 p-3">
        <div class="flex items-center gap-3">
          <span class="text-xs font-semibold text-indigo-900">复合逻辑：</span>
          <el-radio-group
            :model-value="current.logic"
            size="small"
            @change="setCompositeLogic"
          >
            <el-radio-button value="all">全部条件同时满足 (AND)</el-radio-button>
            <el-radio-button value="any">满足任一条件即可 (OR)</el-radio-button>
          </el-radio-group>
        </div>

        <el-dropdown @command="(cmd: RuleMatcher['type']) => addCompositeCondition(cmd)">
          <el-button size="small" type="primary">
            <Plus class="h-3.5 w-3.5 mr-1" />
            添加子条件
          </el-button>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item command="keyword_group">关键词词组</el-dropdown-item>
              <el-dropdown-item command="extension_rule">扩展名后缀集合</el-dropdown-item>
              <el-dropdown-item command="file_constraint">文件体积/长度约束</el-dropdown-item>
              <el-dropdown-item command="title_pattern_preset">标题结构特征</el-dropdown-item>
              <el-dropdown-item command="substantive_content">有效实质内容</el-dropdown-item>
              <el-dropdown-item command="size_ratio">类型体积占比</el-dropdown-item>
              <el-dropdown-item command="custom_regex">专家自定义正则</el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
      </div>

      <div v-if="current.conditions.length === 0" class="text-xs text-slate-400 text-center py-6 border border-dashed border-slate-200 rounded-lg">
        暂无子条件，请点击右上角「添加子条件」组合成复合判断（最适合配置精准白名单豁免）
      </div>

      <div class="space-y-3">
        <div
          v-for="(sub, idx) in current.conditions"
          :key="idx"
          class="rounded-lg border border-slate-200 bg-slate-50/40 p-3 space-y-2"
        >
          <div class="flex items-center justify-between border-b border-slate-200 pb-2">
            <div class="flex items-center gap-2">
              <span class="text-xs font-semibold text-slate-700">子条件 #{{ idx + 1 }}</span>
              <el-tag size="small" type="info">{{ matcherLabel(sub.type) }}</el-tag>
            </div>
            <div class="flex items-center gap-1">
              <el-button
                size="small"
                link
                :disabled="idx === 0"
                @click="moveCompositeChild(idx, -1)"
              >
                <ArrowUp class="h-3.5 w-3.5" />
              </el-button>
              <el-button
                size="small"
                link
                :disabled="idx === current.conditions.length - 1"
                @click="moveCompositeChild(idx, 1)"
              >
                <ArrowDown class="h-3.5 w-3.5" />
              </el-button>
              <el-button size="small" type="danger" link @click="removeCompositeChild(idx)">
                <Trash2 class="h-3.5 w-3.5" />
              </el-button>
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
