<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { RouterLink } from "vue-router";
import { ElMessage } from "element-plus";
import {
  ChevronDown,
  ChevronRight,
  Copy,
  File as FileIcon,
  Folder,
  FolderOpen,
  FolderTree,
  List,
  Magnet,
  Pencil,
  Plus,
  Search,
  Sparkles,
  Trash2,
} from "lucide-vue-next";
import type { ConsoleIngestEvent } from "@/api/console";
import {
  getTorrentDetail,
  updateTorrent,
  type TorrentDetail,
  type TorrentFileInfo,
} from "@/api/torrents";
import ObsDrawer from "@/components/ui/ObsDrawer.vue";
import ObsNumberInput from "@/components/ui/ObsNumberInput.vue";
import ObsSelect from "@/components/ui/ObsSelect.vue";
import { copyToClipboard, fmtBytes, fmtCount, formatDateTime } from "@/utils/format";

interface TreeNode {
  id: string;
  label: string;
  isDir: boolean;
  size: number;
  fileCount: number;
  children: TreeNode[];
}

interface FlatTreeRow {
  id: string;
  label: string;
  isDir: boolean;
  size: number;
  fileCount: number;
  depth: number;
  expanded: boolean;
}

const props = withDefaults(
  defineProps<{
    modelValue: boolean;
    torrentId: string | number | null;
    fallbackEvent?: ConsoleIngestEvent | null;
    allowDelete?: boolean;
    initialTab?: "files" | "edit";
  }>(),
  {
    fallbackEvent: null,
    allowDelete: false,
    initialTab: "files",
  },
);

const emit = defineEmits<{
  (e: "update:modelValue", v: boolean): void;
  (e: "updated", detail: TorrentDetail): void;
  (e: "requestDelete", detail: TorrentDetail): void;
}>();

const open = computed({
  get: () => props.modelValue,
  set: (v: boolean) => emit("update:modelValue", v),
});

const loading = ref(false);
const loadError = ref("");
const usingFallback = ref(false);
const detail = ref<TorrentDetail | null>(null);
const activeTab = ref<"files" | "edit">("files");

const fileSearch = ref("");
const fileSort = ref<"size_desc" | "size_asc" | "path_asc">("size_desc");
const fileViewMode = ref<"tree" | "flat">("tree");
const expandedDirs = ref<Set<string>>(new Set());
const flatPage = ref(1);
const flatPageSize = 50;

const editSubmitting = ref(false);
const editForm = ref<{
  raw_name: string;
  refine_name: string;
  hot: number;
  files: TorrentFileInfo[];
}>({
  raw_name: "",
  refine_name: "",
  hot: 0,
  files: [],
});

const FILE_SORT_OPTIONS = [
  { label: "体积降序", value: "size_desc" },
  { label: "体积升序", value: "size_asc" },
  { label: "路径字典序", value: "path_asc" },
];

function syncEditForm(d: TorrentDetail) {
  editForm.value = {
    raw_name: d.raw_name,
    refine_name: d.refine_name,
    hot: d.hot,
    files: d.files.map((f) => ({ path: f.path, size: f.size })),
  };
}

function buildFallbackDetail(ev: ConsoleIngestEvent): TorrentDetail {
  const updatedSec = ev.updated_at ? Math.floor(ev.updated_at / 1000) || ev.updated_at : Math.floor(Date.now() / 1000);
  return {
    id: String(ev.manticore_id ?? "0"),
    info_hash: ev.info_hash,
    magnet_link: `magnet:?xt=urn:btih:${ev.info_hash}`,
    raw_name: ev.raw_name,
    refine_name: ev.refine_name,
    file_count: ev.file_count,
    total_size: ev.total_size,
    hot: ev.hot,
    created_at: updatedSec,
    updated_at: updatedSec,
    files: (ev.files_preview ?? []).map((f) => ({ path: f.path, size: f.size })),
  };
}

function hasValidTorrentId(val: string | number | null | undefined): boolean {
  if (val === null || val === undefined) return false;
  const s = String(val).trim();
  return s.length > 0 && s !== "0";
}

async function loadDetail() {
  loadError.value = "";
  usingFallback.value = false;
  activeTab.value = props.initialTab ?? "files";
  fileSearch.value = "";
  flatPage.value = 1;

  const rawId = props.torrentId;
  const queryKey = hasValidTorrentId(rawId) ? String(rawId).trim() : "";
  if (queryKey) {
    loading.value = true;
    try {
      const res = await getTorrentDetail(queryKey);
      detail.value = res;
      syncEditForm(res);
      initExpandedTree(res.files);
      loading.value = false;
      return;
    } catch (e) {
      if (props.fallbackEvent) {
        const fb = buildFallbackDetail(props.fallbackEvent);
        detail.value = fb;
        syncEditForm(fb);
        usingFallback.value = true;
        initExpandedTree(fb.files);
        loading.value = false;
        return;
      }
      loadError.value = e instanceof Error ? e.message : "加载种子详情失败";
    } finally {
      loading.value = false;
    }
    return;
  }

  if (props.fallbackEvent) {
    const fb = buildFallbackDetail(props.fallbackEvent);
    detail.value = fb;
    syncEditForm(fb);
    usingFallback.value = true;
    initExpandedTree(fb.files);
  } else {
    detail.value = null;
  }
}

watch(
  () => [props.modelValue, props.torrentId, props.fallbackEvent?.seq],
  ([visible]) => {
    if (visible) void loadDetail();
  },
  { immediate: true },
);

const filteredSortedFiles = computed<TorrentFileInfo[]>(() => {
  const files = detail.value?.files ?? [];
  const kw = fileSearch.value.trim().toLowerCase();
  const list = kw ? files.filter((f) => f.path.toLowerCase().includes(kw)) : [...files];
  if (fileSort.value === "size_desc") {
    list.sort((a, b) => b.size - a.size || a.path.localeCompare(b.path));
  } else if (fileSort.value === "size_asc") {
    list.sort((a, b) => a.size - b.size || a.path.localeCompare(b.path));
  } else {
    list.sort((a, b) => a.path.localeCompare(b.path));
  }
  return list;
});

watch([fileSearch, fileSort], () => {
  flatPage.value = 1;
});

const treeRoots = computed<TreeNode[]>(() => {
  const files = filteredSortedFiles.value;
  const rootMap = new Map<string, TreeNode>();

  function getOrCreateDir(parentMap: Map<string, TreeNode>, id: string, label: string): TreeNode {
    let node = parentMap.get(`dir:${id}`);
    if (!node) {
      node = { id, label, isDir: true, size: 0, fileCount: 0, children: [] };
      parentMap.set(`dir:${id}`, node);
    }
    return node;
  }

  const childMapById = new Map<string, Map<string, TreeNode>>();

  for (let i = 0; i < files.length; i += 1) {
    const file = files[i];
    const parts = file.path.split("/").filter((s) => s.length > 0);
    if (parts.length <= 1) {
      const fid = `file:${i}:${file.path}`;
      rootMap.set(fid, {
        id: fid,
        label: parts[0] ?? file.path,
        isDir: false,
        size: file.size,
        fileCount: 1,
        children: [],
      });
      continue;
    }

    let curMap = rootMap;
    let prefix = "";
    for (let d = 0; d < parts.length - 1; d += 1) {
      const seg = parts[d];
      prefix = prefix ? `${prefix}/${seg}` : seg;
      const dirNode = getOrCreateDir(curMap, prefix, seg);
      dirNode.size += file.size;
      dirNode.fileCount += 1;
      let nextMap = childMapById.get(prefix);
      if (!nextMap) {
        nextMap = new Map<string, TreeNode>();
        childMapById.set(prefix, nextMap);
      }
      curMap = nextMap;
    }

    const fileName = parts[parts.length - 1];
    curMap.set(`file:${i}:${file.path}`, {
      id: `file:${i}:${file.path}`,
      label: fileName,
      isDir: false,
      size: file.size,
      fileCount: 1,
      children: [],
    });
  }

  function materialize(map: Map<string, TreeNode>): TreeNode[] {
    const nodes = Array.from(map.values()).map((n) => {
      if (!n.isDir) return n;
      const cMap = childMapById.get(n.id);
      return {
        ...n,
        children: cMap ? materialize(cMap) : [],
      };
    });
    nodes.sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      if (fileSort.value === "size_desc") return b.size - a.size || a.label.localeCompare(b.label);
      if (fileSort.value === "size_asc") return a.size - b.size || a.label.localeCompare(b.label);
      return a.label.localeCompare(b.label);
    });
    return nodes;
  }

  return materialize(rootMap);
});

function collectAllDirIds(nodes: TreeNode[], out: Set<string>) {
  for (const n of nodes) {
    if (n.isDir) {
      out.add(n.id);
      collectAllDirIds(n.children, out);
    }
  }
}

function initExpandedTree(files: TorrentFileInfo[]) {
  const next = new Set<string>();
  if (files.length <= 80) {
    for (const f of files) {
      const parts = f.path.split("/").filter((s) => s.length > 0);
      let prefix = "";
      for (let d = 0; d < parts.length - 1; d += 1) {
        prefix = prefix ? `${prefix}/${parts[d]}` : parts[d];
        next.add(prefix);
      }
    }
  } else {
    for (const f of files) {
      const parts = f.path.split("/").filter((s) => s.length > 0);
      if (parts.length > 1) next.add(parts[0]);
    }
  }
  expandedDirs.value = next;
}

function expandAllDirs() {
  const next = new Set<string>();
  collectAllDirIds(treeRoots.value, next);
  expandedDirs.value = next;
}

function collapseAllDirs() {
  expandedDirs.value = new Set();
}

function toggleDir(id: string) {
  const next = new Set(expandedDirs.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  expandedDirs.value = next;
}

const visibleTreeRows = computed<FlatTreeRow[]>(() => {
  const rows: FlatTreeRow[] = [];
  const forceExpand = fileSearch.value.trim().length > 0;
  function walk(nodes: TreeNode[], depth: number) {
    for (const n of nodes) {
      const expanded = forceExpand || expandedDirs.value.has(n.id);
      rows.push({
        id: n.id,
        label: n.label,
        isDir: n.isDir,
        size: n.size,
        fileCount: n.fileCount,
        depth,
        expanded,
      });
      if (n.isDir && expanded) {
        walk(n.children, depth + 1);
      }
    }
  }
  walk(treeRoots.value, 0);
  return rows;
});

const pagedFlatFiles = computed(() => {
  const start = (flatPage.value - 1) * flatPageSize;
  return filteredSortedFiles.value.slice(start, start + flatPageSize);
});

const flatTotalPages = computed(() => Math.max(1, Math.ceil(filteredSortedFiles.value.length / flatPageSize)));

async function handleCopy(text: string, label: string) {
  const ok = await copyToClipboard(text);
  if (ok) ElMessage.success(`已复制 ${label}`);
  else ElMessage.warning("当前环境不允许剪贴板写入");
}

async function submitEdit() {
  if (!detail.value || !hasValidTorrentId(detail.value.id)) return;
  const cleanedFiles = editForm.value.files
    .map((f) => ({ path: f.path.trim(), size: Math.max(0, Number(f.size) || 0) }))
    .filter((f) => f.path.length > 0);

  editSubmitting.value = true;
  try {
    const updated = await updateTorrent(detail.value.info_hash || detail.value.id, {
      raw_name: editForm.value.raw_name.trim(),
      refine_name: editForm.value.refine_name.trim(),
      hot: Math.max(0, Math.round(editForm.value.hot || 0)),
      files: cleanedFiles,
    });
    detail.value = updated;
    syncEditForm(updated);
    initExpandedTree(updated.files);
    ElMessage.success("种子元数据已更新至 Manticore");
    emit("updated", updated);
    activeTab.value = "files";
  } catch (e) {
    ElMessage.error(e instanceof Error ? e.message : "保存修改失败");
  } finally {
    editSubmitting.value = false;
  }
}
</script>

<template>
  <ObsDrawer v-model="open" max-width="860px">
    <template #header>
      <div class="min-w-0 pr-4">
        <div class="flex flex-wrap items-center gap-2">
          <span class="px-2 py-0.5 rounded bg-emerald-500/15 border border-emerald-500/30 text-[11px] font-mono text-emerald-300">
            {{ detail && hasValidTorrentId(detail.id) ? `ID #${detail.id}` : "SSE PREVIEW" }}
          </span>
          <span v-if="usingFallback" class="px-2 py-0.5 rounded bg-amber-500/15 border border-amber-500/30 text-[11px] text-amber-300">
            实时事件前 12 条文件摘要
          </span>
        </div>
        <h3 class="mt-1.5 text-sm font-semibold text-slate-100 break-words">
          {{ detail ? detail.refine_name || detail.raw_name : "种子详情检视" }}
        </h3>
        <div v-if="detail" class="mt-1 text-[11px] font-mono text-slate-400 break-all">
          {{ detail.info_hash }}
        </div>
      </div>
    </template>

    <div v-if="loading" class="py-16 text-center text-xs font-mono text-slate-400">
      正在从 Manticore 读取完整文件目录…
    </div>

    <div v-else-if="loadError" class="obs-inset p-4 rounded-lg border-rose-500/30 text-xs text-rose-300">
      {{ loadError }}
    </div>

    <div v-else-if="detail" class="space-y-4">
      <!-- Action bar & quick jumps -->
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="flex flex-wrap items-center gap-2">
          <button
            type="button"
            class="obs-btn obs-btn-ghost px-2.5 py-1.5 text-xs inline-flex items-center gap-1.5"
            @click="handleCopy(detail.info_hash, 'InfoHash')"
          >
            <Copy class="w-3.5 h-3.5 text-slate-400" />
            复制 Hash
          </button>
          <button
            type="button"
            class="obs-btn obs-btn-ghost px-2.5 py-1.5 text-xs inline-flex items-center gap-1.5"
            @click="handleCopy(detail.magnet_link, 'Magnet 链接')"
          >
            <Magnet class="w-3.5 h-3.5 text-emerald-400" />
            复制磁力链
          </button>
          <RouterLink
            :to="`/redis?hash=${encodeURIComponent(detail.info_hash)}`"
            class="obs-btn obs-btn-ghost px-2.5 py-1.5 text-xs inline-flex items-center gap-1.5"
            @click="open = false"
          >
            <Search class="w-3.5 h-3.5 text-sky-400" />
            Redis 穿透诊断
          </RouterLink>
          <RouterLink
            :to="`/filter?tab=playground&title=${encodeURIComponent(detail.raw_name)}`"
            class="obs-btn obs-btn-ghost px-2.5 py-1.5 text-xs inline-flex items-center gap-1.5"
            @click="open = false"
          >
            <Sparkles class="w-3.5 h-3.5 text-amber-400" />
            清洗沙盒验算
          </RouterLink>
        </div>

        <button
          v-if="allowDelete && hasValidTorrentId(detail.id)"
          type="button"
          class="obs-btn obs-btn-danger px-2.5 py-1.5 text-xs inline-flex items-center gap-1.5"
          @click="emit('requestDelete', detail)"
        >
          <Trash2 class="w-3.5 h-3.5" />
          删除记录
        </button>
      </div>

      <!-- Summary grid -->
      <div class="obs-inset p-4 rounded-lg grid grid-cols-2 sm:grid-cols-4 gap-3 text-xs">
        <div class="col-span-2 sm:col-span-4 space-y-1 pb-2.5 border-b border-[#1e293b]">
          <div class="flex items-start gap-2">
            <span class="text-[11px] font-mono text-slate-500 w-20 shrink-0">raw_name</span>
            <span class="text-slate-300 break-all font-mono text-[11px]">{{ detail.raw_name }}</span>
          </div>
          <div class="flex items-start gap-2">
            <span class="text-[11px] font-mono text-slate-500 w-20 shrink-0">refine_name</span>
            <span class="text-emerald-300 break-all font-medium">{{ detail.refine_name || "（未清洗 / 与原标题一致）" }}</span>
          </div>
        </div>
        <div>
          <div class="text-[11px] text-slate-500">包含文件数</div>
          <div class="mt-0.5 font-mono font-semibold text-slate-100">{{ fmtCount(detail.file_count) }}</div>
        </div>
        <div>
          <div class="text-[11px] text-slate-500">总体积</div>
          <div class="mt-0.5 font-mono font-semibold text-sky-300">{{ fmtBytes(detail.total_size) }}</div>
        </div>
        <div>
          <div class="text-[11px] text-slate-500">热度 (hot)</div>
          <div class="mt-0.5 font-mono font-semibold text-amber-300">{{ fmtCount(detail.hot) }}</div>
        </div>
        <div>
          <div class="text-[11px] text-slate-500">更新时间</div>
          <div class="mt-0.5 font-mono text-[11px] text-slate-300">{{ formatDateTime(detail.updated_at) }}</div>
        </div>
      </div>

      <!-- Sub-tabs -->
      <div class="flex items-center justify-between border-b border-[#1e293b]">
        <div class="flex items-center gap-1">
          <button
            type="button"
            class="px-3 py-2 text-xs font-medium border-b-2 transition"
            :class="
              activeTab === 'files'
                ? 'border-emerald-400 text-emerald-300'
                : 'border-transparent text-slate-400 hover:text-slate-200'
            "
            @click="activeTab = 'files'"
          >
            文件结构 ({{ detail.files.length }})
          </button>
          <button
            v-if="!usingFallback && hasValidTorrentId(detail.id)"
            type="button"
            class="px-3 py-2 text-xs font-medium border-b-2 transition inline-flex items-center gap-1.5"
            :class="
              activeTab === 'edit'
                ? 'border-emerald-400 text-emerald-300'
                : 'border-transparent text-slate-400 hover:text-slate-200'
            "
            @click="activeTab = 'edit'"
          >
            <Pencil class="w-3 h-3" />
            编辑标题 / 热度 / 文件
          </button>
        </div>
      </div>

      <!-- Tab 1: File tree / flat view -->
      <div v-if="activeTab === 'files'" class="space-y-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div class="flex flex-wrap items-center gap-2 flex-1 min-w-[220px]">
            <div class="relative flex-1 max-w-xs">
              <Search class="w-3.5 h-3.5 text-slate-500 absolute left-2.5 top-1/2 -translate-y-1/2 pointer-events-none" />
              <input
                v-model="fileSearch"
                type="text"
                placeholder="筛选文件名或路径…"
                class="obs-input pl-8 pr-3 py-1.5 text-xs"
              />
            </div>
            <ObsSelect
              v-model="fileSort"
              :options="FILE_SORT_OPTIONS"
              trigger-class="text-xs py-1.5 min-w-[120px]"
            />
          </div>

          <div class="flex items-center gap-2">
            <template v-if="fileViewMode === 'tree'">
              <button type="button" class="obs-btn obs-btn-ghost px-2 py-1 text-[11px]" @click="expandAllDirs">
                展开全部
              </button>
              <button type="button" class="obs-btn obs-btn-ghost px-2 py-1 text-[11px]" @click="collapseAllDirs">
                折叠
              </button>
            </template>
            <div class="bg-[#090d16] border border-[#1e293b] p-0.5 rounded-lg inline-flex">
              <button
                type="button"
                class="px-2.5 py-1 rounded text-[11px] inline-flex items-center gap-1 transition"
                :class="fileViewMode === 'tree' ? 'bg-emerald-500/15 text-emerald-300' : 'text-slate-400 hover:text-slate-200'"
                @click="fileViewMode = 'tree'"
              >
                <FolderTree class="w-3 h-3" />
                目录树
              </button>
              <button
                type="button"
                class="px-2.5 py-1 rounded text-[11px] inline-flex items-center gap-1 transition"
                :class="fileViewMode === 'flat' ? 'bg-emerald-500/15 text-emerald-300' : 'text-slate-400 hover:text-slate-200'"
                @click="fileViewMode = 'flat'"
              >
                <List class="w-3 h-3" />
                平铺
              </button>
            </div>
          </div>
        </div>

        <!-- Tree view -->
        <div
          v-if="fileViewMode === 'tree'"
          class="obs-inset rounded-lg max-h-[440px] overflow-auto obs-scroll divide-y divide-[#1e293b]/40"
        >
          <div
            v-for="row in visibleTreeRows"
            :key="row.id"
            class="flex items-center justify-between gap-3 px-3 py-1.5 text-xs hover:bg-[#131b2c]/70 transition-colors"
            :style="{ paddingLeft: `${12 + row.depth * 18}px` }"
            :class="row.isDir ? 'cursor-pointer select-none' : ''"
            @click="row.isDir ? toggleDir(row.id) : undefined"
          >
            <div class="flex items-center gap-1.5 min-w-0">
              <template v-if="row.isDir">
                <ChevronDown v-if="row.expanded" class="w-3.5 h-3.5 text-slate-500 shrink-0" />
                <ChevronRight v-else class="w-3.5 h-3.5 text-slate-500 shrink-0" />
                <FolderOpen v-if="row.expanded" class="w-3.5 h-3.5 text-amber-400 shrink-0" />
                <Folder v-else class="w-3.5 h-3.5 text-amber-400 shrink-0" />
                <span class="font-medium text-slate-200 truncate">{{ row.label }}</span>
                <span class="text-[10px] font-mono text-slate-500 shrink-0">({{ row.fileCount }})</span>
              </template>
              <template v-else>
                <span class="w-3.5 shrink-0"></span>
                <FileIcon class="w-3.5 h-3.5 text-slate-500 shrink-0" />
                <span class="font-mono text-[11px] text-slate-300 truncate">{{ row.label }}</span>
              </template>
            </div>
            <span class="font-mono text-[11px] text-slate-400 shrink-0 tabular-nums">
              {{ fmtBytes(row.size) }}
            </span>
          </div>

          <div v-if="visibleTreeRows.length === 0" class="py-10 text-center text-xs text-slate-500">
            没有匹配筛选条件的文件
          </div>
        </div>

        <!-- Flat paginated table -->
        <div v-else class="space-y-2.5">
          <div class="obs-inset rounded-lg overflow-hidden max-h-[420px] overflow-auto obs-scroll">
            <table class="w-full text-left text-xs">
              <thead class="sticky top-0 bg-[#0b101b] border-b border-[#1e293b] text-[11px] text-slate-400">
                <tr>
                  <th class="px-3 py-2">文件路径</th>
                  <th class="px-3 py-2 text-right w-28">大小</th>
                </tr>
              </thead>
              <tbody class="divide-y divide-[#1e293b]/50">
                <tr v-for="(f, idx) in pagedFlatFiles" :key="`${idx}:${f.path}`" class="hover:bg-[#131b2c]/60">
                  <td class="px-3 py-2 font-mono text-[11px] text-slate-300 break-all">{{ f.path }}</td>
                  <td class="px-3 py-2 font-mono text-[11px] text-slate-400 text-right whitespace-nowrap">
                    {{ fmtBytes(f.size) }}
                  </td>
                </tr>
                <tr v-if="pagedFlatFiles.length === 0">
                  <td colspan="2" class="py-10 text-center text-xs text-slate-500">无匹配文件</td>
                </tr>
              </tbody>
            </table>
          </div>

          <div class="flex items-center justify-between text-[11px] font-mono text-slate-400">
            <span>共 {{ filteredSortedFiles.length }} 个文件</span>
            <div class="inline-flex items-center gap-1.5">
              <button
                type="button"
                class="obs-btn obs-btn-ghost px-2 py-1 text-[11px]"
                :disabled="flatPage <= 1"
                @click="flatPage -= 1"
              >
                上一页
              </button>
              <span class="px-2 text-slate-300">{{ flatPage }} / {{ flatTotalPages }}</span>
              <button
                type="button"
                class="obs-btn obs-btn-ghost px-2 py-1 text-[11px]"
                :disabled="flatPage >= flatTotalPages"
                @click="flatPage += 1"
              >
                下一页
              </button>
            </div>
          </div>
        </div>
      </div>

      <!-- Tab 2: Edit metadata & files -->
      <div v-else class="space-y-4">
        <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">原始标题 raw_name</label>
            <input v-model="editForm.raw_name" type="text" class="obs-input px-3 py-2 text-xs font-mono" />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">清洗标题 refine_name</label>
            <input v-model="editForm.refine_name" type="text" class="obs-input px-3 py-2 text-xs" />
          </div>
          <div>
            <label class="block text-[11px] text-slate-400 mb-1">热度 hot</label>
            <ObsNumberInput v-model="editForm.hot" :min="0" :step="1" />
          </div>
        </div>

        <div class="space-y-2">
          <div class="flex items-center justify-between">
            <span class="text-xs font-medium text-slate-300">文件列表 ({{ editForm.files.length }})</span>
            <button
              type="button"
              class="obs-btn obs-btn-ghost px-2.5 py-1 text-xs inline-flex items-center gap-1"
              @click="editForm.files.push({ path: '', size: 0 })"
            >
              <Plus class="w-3 h-3 text-emerald-400" />
              新增文件项
            </button>
          </div>
          <div class="max-h-64 overflow-auto obs-scroll space-y-2 pr-1">
            <div
              v-for="(f, idx) in editForm.files"
              :key="idx"
              class="flex items-center gap-2"
            >
              <input v-model="f.path" type="text" placeholder="dir/filename.ext" class="obs-input px-2.5 py-1.5 text-xs font-mono flex-1" />
              <div class="w-36 shrink-0">
                <ObsNumberInput v-model="f.size" :min="0" :step="1024" />
              </div>
              <button
                type="button"
                class="obs-btn obs-btn-danger px-2.5 py-1.5 text-xs shrink-0"
                @click="editForm.files.splice(idx, 1)"
              >
                删
              </button>
            </div>
          </div>
        </div>

        <div class="flex justify-end pt-2 border-t border-[#1e293b]">
          <button
            type="button"
            class="obs-btn obs-btn-primary px-4 py-2 text-xs"
            :disabled="editSubmitting"
            @click="submitEdit"
          >
            {{ editSubmitting ? "正在写入…" : "保存修改到 Manticore" }}
          </button>
        </div>
      </div>
    </div>
  </ObsDrawer>
</template>
