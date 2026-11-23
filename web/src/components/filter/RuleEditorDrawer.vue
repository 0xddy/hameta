<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { ElMessage } from "element-plus";
import { ShieldCheck, ShieldOff, X } from "lucide-vue-next";
import type {
  ExtensionPackInfo,
  KeywordPackInfo,
  PolicyAction,
  PolicyRule,
} from "@/api/filter";
import MatcherEditor from "@/components/filter/MatcherEditor.vue";
import ObsDrawer from "@/components/ui/ObsDrawer.vue";

const props = withDefaults(
  defineProps<{
    modelValue: boolean;
    rule: PolicyRule | null;
    mode?: "create" | "edit";
    extensionPacks?: ExtensionPackInfo[];
    keywordPacks?: KeywordPackInfo[];
  }>(),
  {
    mode: "edit",
    extensionPacks: () => [],
    keywordPacks: () => [],
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", v: boolean): void;
  (e: "save", rule: PolicyRule): void;
}>();

const open = computed({
  get: () => props.modelValue,
  set: (v: boolean) => emit("update:modelValue", v),
});

function blankRule(): PolicyRule {
  return {
    id: `rule_${Date.now().toString(36)}`,
    name: "新建自定义过滤规则",
    enabled: true,
    action: "reject",
    tags: [],
    note: "",
    matcher: {
      type: "keyword_group",
      target: "raw_name",
      match_mode: "contains",
      case_sensitive: false,
      words: [],
    },
  };
}

function deepClone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

const form = ref<PolicyRule>(blankRule());
const tagInput = ref("");

watch(
  () => [props.modelValue, props.rule] as const,
  ([visible, r]) => {
    if (!visible) return;
    form.value = r ? deepClone(r) : blankRule();
    tagInput.value = "";
  },
  { immediate: true },
);

function addTag() {
  const t = tagInput.value.trim();
  if (!t) return;
  if (!form.value.tags.includes(t)) {
    form.value.tags = [...form.value.tags, t];
  }
  tagInput.value = "";
}

function removeTag(tag: string) {
  form.value.tags = form.value.tags.filter((x) => x !== tag);
}

function handleSave() {
  if (!form.value.id.trim()) {
    ElMessage.warning("请填写规则唯一 ID");
    return;
  }
  if (!form.value.name.trim()) {
    ElMessage.warning("请填写规则名称");
    return;
  }
  emit("save", {
    ...deepClone(form.value),
    id: form.value.id.trim(),
    name: form.value.name.trim(),
    note: form.value.note.trim(),
  });
  open.value = false;
}
</script>

<template>
  <ObsDrawer v-model="open" max-width="780px">
    <template #header>
      <div>
        <div class="text-[11px] font-mono text-emerald-400">
          {{ mode === "create" ? "CREATE POLICY RULE" : "EDIT POLICY RULE" }}
        </div>
        <h3 class="mt-0.5 text-sm font-semibold text-slate-100">
          {{ mode === "create" ? "创建过滤 / 白名单规则" : `编辑规则 · ${form.name}` }}
        </h3>
      </div>
    </template>

    <div class="space-y-4 text-xs">
      <!-- Action Type Switch -->
      <div class="grid grid-cols-2 gap-2.5">
        <button
          type="button"
          class="p-3 rounded-lg border text-left transition flex items-start gap-2.5"
          :class="
            form.action === 'allow'
              ? 'bg-emerald-500/12 border-emerald-500/40 text-emerald-200'
              : 'obs-inset text-slate-400 hover:text-slate-200'
          "
          @click="form.action = 'allow' as PolicyAction"
        >
          <ShieldCheck class="w-4 h-4 text-emerald-400 shrink-0 mt-0.5" />
          <div>
            <div class="font-semibold">白名单豁免 (ALLOW)</div>
            <div class="text-[11px] text-slate-400 mt-0.5">命中后直接豁免后续拦截规则（仍需通过基础闸门）</div>
          </div>
        </button>

        <button
          type="button"
          class="p-3 rounded-lg border text-left transition flex items-start gap-2.5"
          :class="
            form.action === 'reject'
              ? 'bg-rose-500/12 border-rose-500/40 text-rose-200'
              : 'obs-inset text-slate-400 hover:text-slate-200'
          "
          @click="form.action = 'reject' as PolicyAction"
        >
          <ShieldOff class="w-4 h-4 text-rose-400 shrink-0 mt-0.5" />
          <div>
            <div class="font-semibold">黑名单拦截 (REJECT)</div>
            <div class="text-[11px] text-slate-400 mt-0.5">命中后直接丢弃该种子，不写入 Manticore</div>
          </div>
        </button>
      </div>

      <!-- Meta Fields -->
      <div class="obs-inset p-4 rounded-lg space-y-3">
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">规则唯一 ID *</label>
            <input v-model="form.id" type="text" class="obs-input px-3 py-1.5 text-xs font-mono" />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">规则显示名称 *</label>
            <input v-model="form.name" type="text" class="obs-input px-3 py-1.5 text-xs" />
          </div>
        </div>

        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3 items-center">
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">备注说明</label>
            <input v-model="form.note" type="text" placeholder="规则目的 / 误伤边界说明" class="obs-input px-3 py-1.5 text-xs" />
          </div>
          <label class="inline-flex items-center gap-2 text-xs text-slate-200 cursor-pointer pt-5">
            <input v-model="form.enabled" type="checkbox" class="obs-checkbox" />
            启用该规则参与流水线判定
          </label>
        </div>

        <div>
          <label class="block text-[11px] text-slate-400 mb-1">标签分类</label>
          <div class="flex items-center gap-2">
            <input
              v-model="tagInput"
              type="text"
              placeholder="输入标签后回车追加"
              class="obs-input px-3 py-1.5 text-xs flex-1"
              @keyup.enter="addTag"
            />
            <button type="button" class="obs-btn obs-btn-ghost px-3 py-1.5 text-xs" @click="addTag">追加</button>
          </div>
          <div v-if="form.tags.length > 0" class="mt-2 flex flex-wrap gap-1.5">
            <span
              v-for="tag in form.tags"
              :key="tag"
              class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-slate-800 border border-slate-700 text-[11px] text-slate-200"
            >
              {{ tag }}
              <button type="button" class="text-slate-400 hover:text-rose-300" @click="removeTag(tag)">
                <X class="w-3 h-3" />
              </button>
            </span>
          </div>
        </div>
      </div>

      <!-- Nested Matcher Editor -->
      <div class="obs-card p-4 space-y-2">
        <div class="text-xs font-semibold text-slate-100 border-b border-[#1e293b] pb-2">
          规则匹配条件配置
        </div>
        <MatcherEditor
          v-model="form.matcher"
          :extension-packs="extensionPacks"
          :keyword-packs="keywordPacks"
        />
      </div>
    </div>

    <template #footer>
      <button type="button" class="obs-btn obs-btn-ghost px-3.5 py-1.5 text-xs" @click="open = false">
        取消
      </button>
      <button type="button" class="obs-btn obs-btn-primary px-4 py-1.5 text-xs" @click="handleSave">
        保存至草稿规则集
      </button>
    </template>
  </ObsDrawer>
</template>
