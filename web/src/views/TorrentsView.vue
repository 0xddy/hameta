<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import {
  Check,
  Copy,
  Edit3,
  Eye,
  File as FileIcon,
  Folder,
  FolderTree,
  List,
  Magnet,
  Plus,
  RefreshCw,
  Search,
  Trash2,
} from "lucide-vue-next";
import { ElMessage } from "element-plus";
import {
  batchDeleteTorrents,
  createTorrent,
  deleteTorrent,
  getTorrentDetail,
  listTorrents,
  updateTorrent,
  type ListTorrentsQuery,
  type TorrentDetail,
  type TorrentFileInfo,
  type TorrentSummary,
} from "@/api/torrents";

interface FileTreeNode {
  id: string;
  label: string;
  isDir: boolean;
  size: number;
  fileCount: number;
  fullPath: string;
  children?: FileTreeNode[];
}

const loading = ref(false);
const items = ref<TorrentSummary[]>([]);
const total = ref(0);
const totalRelation = ref("eq");
const tookMs = ref(0);
const selectedRows = ref<TorrentSummary[]>([]);

const filters = reactive<{
  q: string;
  sort_by: "updated_at" | "created_at" | "hot" | "file_count";
  sort_dir: "desc" | "asc";
  min_hot: number | undefined;
  max_hot: number | undefined;
  min_files: number | undefined;
  empty_refine_only: boolean;
  page: number;
  page_size: number;
}>({
  q: "",
  sort_by: "updated_at",
  sort_dir: "desc",
  min_hot: undefined,
  max_hot: undefined,
  min_files: undefined,
  empty_refine_only: false,
  page: 1,
  page_size: 20,
});

// 新增种子对话框
const createVisible = ref(false);
const createSubmitting = ref(false);
const createForm = reactive<{
  info_hash: string;
  raw_name: string;
  refine_name: string;
  hot: number;
  sync_redis_indexed: boolean;
  files: TorrentFileInfo[];
}>({
  info_hash: "",
  raw_name: "",
  refine_name: "",
  hot: 1,
  sync_redis_indexed: true,
  files: [{ path: "", size: 1048576 }],
});

// 详情与编辑抽屉
const detailVisible = ref(false);
const detailLoading = ref(false);
const detailTab = ref<"files" | "edit">("files");
const detailData = ref<TorrentDetail | null>(null);
const fileViewMode = ref<"tree" | "flat">("tree");
const fileSearchKeyword = ref("");
const fileSortMode = ref<"size_desc" | "size_asc" | "path_asc">("size_desc");
const fileLocalPage = ref(1);
const fileLocalPageSize = 50;

const editSubmitting = ref(false);
const editForm = reactive<{
  raw_name: string;
  refine_name: string;
  hot: number;
  files: TorrentFileInfo[];
}>({
  raw_name: "",
  refine_name: "",
  hot: 1,
  files: [],
});

// 删除确认弹窗（支持单条与批量 + keep_redis_indexed 默认勾选）
const deleteVisible = ref(false);
const deleteSubmitting = ref(false);
const deleteMode = ref<"single" | "batch">("single");
const deleteTarget = ref<TorrentSummary | null>(null);
const keepRedisIndexed = ref(true);

const copiedHash = ref("");

const filteredSortedFiles = computed<TorrentFileInfo[]>(() => {
  if (!detailData.value) return [];
  const kw = fileSearchKeyword.value.trim().toLowerCase();
  const list = detailData.value.files.filter((f) =>
    kw ? f.path.toLowerCase().includes(kw) : true,
  );
  const sorted = [...list];
  if (fileSortMode.value === "size_desc") {
    sorted.sort((a, b) => b.size - a.size || a.path.localeCompare(b.path));
  } else if (fileSortMode.value === "size_asc") {
    sorted.sort((a, b) => a.size - b.size || a.path.localeCompare(b.path));
  } else {
    sorted.sort((a, b) => a.path.localeCompare(b.path));
  }
  return sorted;
});

const pagedFlatFiles = computed<TorrentFileInfo[]>(() => {
  const start = (fileLocalPage.value - 1) * fileLocalPageSize;
  return filteredSortedFiles.value.slice(start, start + fileLocalPageSize);
});

const fileTreeNodes = computed<FileTreeNode[]>(() => {
  const files = filteredSortedFiles.value;
  interface MutableDir {
    name: string;
    fullPath: string;
    dirs: Map<string, MutableDir>;
    files: TorrentFileInfo[];
  }
  const root: MutableDir = { name: "", fullPath: "", dirs: new Map(), files: [] };

  for (const f of files) {
    const parts = f.path.split(/[/\\]+/).filter(Boolean);
    if (parts.length <= 1) {
      root.files.push(f);
      continue;
    }
    let cur = root;
    for (let i = 0; i < parts.length - 1; i += 1) {
      const seg = parts[i];
      let next = cur.dirs.get(seg);
      if (!next) {
        next = {
          name: seg,
          fullPath: cur.fullPath ? `${cur.fullPath}/${seg}` : seg,
          dirs: new Map(),
          files: [],
        };
        cur.dirs.set(seg, next);
      }
      cur = next;
    }
    cur.files.push(f);
  }

  function convert(node: MutableDir): { nodes: FileTreeNode[]; totalSize: number; count: number } {
    const dirNodes: FileTreeNode[] = [];
    let totalSize = 0;
    let count = 0;

    for (const sub of node.dirs.values()) {
      const res = convert(sub);
      totalSize += res.totalSize;
      count += res.count;
      dirNodes.push({
        id: `dir:${sub.fullPath}`,
        label: sub.name,
        isDir: true,
        size: res.totalSize,
        fileCount: res.count,
        fullPath: sub.fullPath,
        children: res.nodes,
      });
    }

    const leafNodes: FileTreeNode[] = node.files.map((f, idx) => {
      const segs = f.path.split(/[/\\]+/).filter(Boolean);
      const leafLabel = segs[segs.length - 1] || f.path;
      totalSize += f.size;
      count += 1;
      return {
        id: `file:${f.path}:${idx}`,
        label: leafLabel,
        isDir: false,
        size: f.size,
        fileCount: 1,
        fullPath: f.path,
      };
    });

    if (fileSortMode.value === "size_desc") {
      dirNodes.sort((a, b) => b.size - a.size);
      leafNodes.sort((a, b) => b.size - a.size);
    } else if (fileSortMode.value === "size_asc") {
      dirNodes.sort((a, b) => a.size - b.size);
      leafNodes.sort((a, b) => a.size - b.size);
    } else {
      dirNodes.sort((a, b) => a.label.localeCompare(b.label));
      leafNodes.sort((a, b) => a.label.localeCompare(b.label));
    }

    return {
      nodes: [...dirNodes, ...leafNodes],
      totalSize,
      count,
    };
  }

  return convert(root).nodes;
});

function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || Number.isNaN(bytes)) return "-";
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

async function copyText(text: string, label: string, marker?: string) {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
    } else {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.focus();
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
    }
    if (marker) {
      copiedHash.value = marker;
      window.setTimeout(() => {
        if (copiedHash.value === marker) copiedHash.value = "";
      }, 1500);
    }
    ElMessage.success(`${label}已复制到剪贴板`);
  } catch {
    ElMessage.error("复制失败，请手动选中复制");
  }
}

async function fetchTorrents() {
  loading.value = true;
  try {
    const query: ListTorrentsQuery = {
      q: filters.q.trim() || undefined,
      sort_by: filters.sort_by,
      sort_dir: filters.sort_dir,
      min_hot: filters.min_hot,
      max_hot: filters.max_hot,
      min_files: filters.min_files,
      empty_refine_only: filters.empty_refine_only || undefined,
      page: filters.page,
      page_size: filters.page_size,
    };
    const resp = await listTorrents(query);
    items.value = resp.items;
    total.value = resp.total;
    totalRelation.value = resp.total_relation;
    tookMs.value = resp.took_ms;
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "检索 Manticore 种子失败");
  } finally {
    loading.value = false;
  }
}

function handleSearch() {
  filters.page = 1;
  void fetchTorrents();
}

function handleResetFilters() {
  filters.q = "";
  filters.sort_by = "updated_at";
  filters.sort_dir = "desc";
  filters.min_hot = undefined;
  filters.max_hot = undefined;
  filters.min_files = undefined;
  filters.empty_refine_only = false;
  filters.page = 1;
  void fetchTorrents();
}

function handleSelectionChange(rows: TorrentSummary[]) {
  selectedRows.value = rows;
}

function generateRandomHexHash() {
  const bytes = new Uint8Array(20);
  crypto.getRandomValues(bytes);
  createForm.info_hash = Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

function openCreateDialog() {
  createForm.info_hash = "";
  createForm.raw_name = "";
  createForm.refine_name = "";
  createForm.hot = 1;
  createForm.sync_redis_indexed = true;
  createForm.files = [{ path: "sample-video.mp4", size: 104857600 }];
  createVisible.value = true;
}

async function submitCreate() {
  const hash = createForm.info_hash.trim();
  if (!/^[0-9a-fA-F]{40}$/.test(hash)) {
    ElMessage.warning("info_hash 必须是 40 位十六进制字符串");
    return;
  }
  if (!createForm.raw_name.trim()) {
    ElMessage.warning("raw_name 不能为空");
    return;
  }
  const cleanFiles = createForm.files
    .map((f) => ({ path: f.path.trim(), size: Number(f.size) || 0 }))
    .filter((f) => f.path.length > 0);

  createSubmitting.value = true;
  try {
    await createTorrent({
      info_hash: hash,
      raw_name: createForm.raw_name.trim(),
      refine_name: createForm.refine_name.trim() || undefined,
      hot: Math.max(1, Number(createForm.hot) || 1),
      files: cleanFiles,
      sync_redis_indexed: createForm.sync_redis_indexed,
    });
    ElMessage.success("种子已写入 Manticore");
    createVisible.value = false;
    await fetchTorrents();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "新增种子失败");
  } finally {
    createSubmitting.value = false;
  }
}

async function openDetailDrawer(row: TorrentSummary, initialTab: "files" | "edit" = "files") {
  detailVisible.value = true;
  detailTab.value = initialTab;
  detailLoading.value = true;
  fileSearchKeyword.value = "";
  fileLocalPage.value = 1;

  try {
    const detail = await getTorrentDetail(row.id);
    detailData.value = detail;
    editForm.raw_name = detail.raw_name;
    editForm.refine_name = detail.refine_name;
    editForm.hot = detail.hot;
    editForm.files = detail.files.map((f) => ({ path: f.path, size: f.size }));
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "加载种子详情失败");
    detailVisible.value = false;
  } finally {
    detailLoading.value = false;
  }
}

async function submitEdit() {
  if (!detailData.value) return;
  if (!editForm.raw_name.trim()) {
    ElMessage.warning("raw_name 不能为空");
    return;
  }
  const cleanFiles = editForm.files
    .map((f) => ({ path: f.path.trim(), size: Math.max(0, Number(f.size) || 0) }))
    .filter((f) => f.path.length > 0);

  editSubmitting.value = true;
  try {
    const updated = await updateTorrent(detailData.value.id, {
      raw_name: editForm.raw_name.trim(),
      refine_name: editForm.refine_name.trim(),
      hot: Number(editForm.hot) || 1,
      files: cleanFiles,
    });
    detailData.value = updated;
    editForm.raw_name = updated.raw_name;
    editForm.refine_name = updated.refine_name;
    editForm.hot = updated.hot;
    editForm.files = updated.files.map((f) => ({ path: f.path, size: f.size }));
    ElMessage.success("Manticore 记录已更新（整行 Replace 落盘）");
    await fetchTorrents();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "更新失败");
  } finally {
    editSubmitting.value = false;
  }
}

function openSingleDelete(row: TorrentSummary) {
  deleteMode.value = "single";
  deleteTarget.value = row;
  keepRedisIndexed.value = true;
  deleteVisible.value = true;
}

function openBatchDelete() {
  if (selectedRows.value.length === 0) return;
  deleteMode.value = "batch";
  deleteTarget.value = null;
  keepRedisIndexed.value = true;
  deleteVisible.value = true;
}

async function confirmDelete() {
  deleteSubmitting.value = true;
  try {
    if (deleteMode.value === "single" && deleteTarget.value) {
      await deleteTorrent(deleteTarget.value.id, keepRedisIndexed.value);
      ElMessage.success(
        keepRedisIndexed.value
          ? "已从 Manticore 删除（已保留 Redis 去重标记）"
          : "已从 Manticore 删除并清除 Redis 去重标记",
      );
      if (detailData.value?.id === deleteTarget.value.id) {
        detailVisible.value = false;
      }
    } else if (deleteMode.value === "batch") {
      const ids = selectedRows.value.map((r) => r.id);
      const res = await batchDeleteTorrents(ids, keepRedisIndexed.value);
      ElMessage.success(`批量删除完成：共删除 ${res.deleted} 条，同步 Redis ${res.redis_synced} 个标记`);
      selectedRows.value = [];
    }
    deleteVisible.value = false;
    await fetchTorrents();
  } catch (err) {
    ElMessage.error(err instanceof Error ? err.message : "删除失败");
  } finally {
    deleteSubmitting.value = false;
  }
}

onMounted(() => {
  void fetchTorrents();
});
</script>

<template>
  <div class="max-w-7xl mx-auto space-y-5">
    <!-- 检索与筛选工具栏 -->
    <div class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 class="text-base font-semibold text-slate-900">磁力资源检索与浏览管理</h2>
          <p class="text-xs text-slate-500 mt-0.5">
            直连 Manticore `torrents` 存量表，支持关键字全文检索、40 位 InfoHash 精准查、整行修改与 Redis 去重标记同步删除
          </p>
        </div>
        <div class="flex items-center gap-2">
          <el-button
            v-if="selectedRows.length > 0"
            size="small"
            type="danger"
            @click="openBatchDelete"
          >
            <Trash2 class="h-3.5 w-3.5 mr-1" />
            批量删除 ({{ selectedRows.length }})
          </el-button>
          <el-button size="small" type="primary" @click="openCreateDialog">
            <Plus class="h-3.5 w-3.5 mr-1" />
            手动新增种子
          </el-button>
        </div>
      </div>

      <div class="flex flex-wrap items-center gap-2.5">
        <el-input
          v-model="filters.q"
          size="small"
          clearable
          placeholder="输入关键字或 40 位 info_hash 检索..."
          class="flex-1 min-w-[260px]"
          @keyup.enter="handleSearch"
        >
          <template #prefix>
            <Search class="h-3.5 w-3.5 text-slate-400" />
          </template>
        </el-input>

        <el-select v-model="filters.sort_by" size="small" style="width: 130px" @change="handleSearch">
          <el-option label="按更新时间" value="updated_at" />
          <el-option label="按入库时间" value="created_at" />
          <el-option label="按热度 (hot)" value="hot" />
          <el-option label="按文件数量" value="file_count" />
        </el-select>

        <el-select v-model="filters.sort_dir" size="small" style="width: 90px" @change="handleSearch">
          <el-option label="降序" value="desc" />
          <el-option label="升序" value="asc" />
        </el-select>

        <el-button size="small" type="primary" :loading="loading" @click="handleSearch">
          检索
        </el-button>
        <el-button size="small" @click="handleResetFilters">
          <RefreshCw class="h-3.5 w-3.5 mr-1" />
          重置
        </el-button>
      </div>

      <!-- 高级过滤条 -->
      <div class="flex flex-wrap items-center gap-3 text-xs text-slate-600 pt-1 border-t border-slate-100">
        <div class="flex items-center gap-1.5">
          <span>热度范围:</span>
          <el-input-number
            v-model="filters.min_hot"
            size="small"
            :min="0"
            :value-on-clear="undefined"
            placeholder="最小"
            controls-position="right"
            style="width: 90px"
          />
          <span>~</span>
          <el-input-number
            v-model="filters.max_hot"
            size="small"
            :min="0"
            :value-on-clear="undefined"
            placeholder="最大"
            controls-position="right"
            style="width: 90px"
          />
        </div>
        <div class="flex items-center gap-1.5">
          <span>最少文件数:</span>
          <el-input-number
            v-model="filters.min_files"
            size="small"
            :min="0"
            :value-on-clear="undefined"
            placeholder="不限"
            controls-position="right"
            style="width: 90px"
          />
        </div>
        <el-checkbox v-model="filters.empty_refine_only" size="small" @change="handleSearch">
          仅显示 refine_name 为空的记录
        </el-checkbox>
        <span class="ml-auto text-[11px] text-slate-400 font-mono">
          命中 {{ totalRelation === "gte" ? "≥ " : "" }}{{ total.toLocaleString("zh-CN") }} 条 · 耗时 {{ tookMs }} ms
        </span>
      </div>
    </div>

    <!-- 结果列表 -->
    <div class="rounded-xl border border-slate-200 bg-white p-5 shadow-sm space-y-4" v-loading="loading">
      <el-table
        :data="items"
        border
        stripe
        size="small"
        @selection-change="handleSelectionChange"
      >
        <el-table-column type="selection" width="42" />

        <el-table-column label="资源标题 / InfoHash" min-width="320">
          <template #default="{ row }">
            <div class="space-y-1 py-0.5">
              <div
                class="font-medium text-slate-900 hover:text-indigo-600 cursor-pointer break-words"
                @click="openDetailDrawer(row, 'files')"
              >
                {{ row.refine_name || row.raw_name || "(无标题)" }}
              </div>
              <div
                v-if="row.refine_name && row.raw_name && row.refine_name !== row.raw_name"
                class="text-[11px] text-slate-400 break-words"
              >
                原始: {{ row.raw_name }}
              </div>
              <div class="flex items-center gap-2 text-[11px] font-mono text-slate-500">
                <span>{{ row.info_hash }}</span>
                <button
                  type="button"
                  class="inline-flex items-center gap-0.5 text-indigo-600 hover:text-indigo-800"
                  @click="copyText(row.magnet_link, '磁力链接', row.info_hash)"
                >
                  <Check v-if="copiedHash === row.info_hash" class="h-3 w-3 text-emerald-600" />
                  <Magnet v-else class="h-3 w-3" />
                  复制磁力
                </button>
              </div>
            </div>
          </template>
        </el-table-column>

        <el-table-column label="总大小" width="105">
          <template #default="{ row }">
            <span class="font-mono text-xs text-slate-700">{{ formatBytes(row.total_size) }}</span>
          </template>
        </el-table-column>

        <el-table-column prop="file_count" label="文件数" width="80">
          <template #default="{ row }">
            <span class="font-mono text-xs">{{ row.file_count }}</span>
          </template>
        </el-table-column>

        <el-table-column prop="hot" label="热度" width="80">
          <template #default="{ row }">
            <el-tag size="small" :type="row.hot >= 10 ? 'danger' : row.hot >= 3 ? 'warning' : 'info'" effect="plain">
              {{ row.hot }}
            </el-tag>
          </template>
        </el-table-column>

        <el-table-column label="更新时间" width="160">
          <template #default="{ row }">
            <span class="font-mono text-[11px] text-slate-600">{{ formatTs(row.updated_at) }}</span>
          </template>
        </el-table-column>

        <el-table-column label="操作" width="175" fixed="right">
          <template #default="{ row }">
            <div class="flex items-center gap-1.5">
              <el-button size="small" plain @click="openDetailDrawer(row, 'files')">
                <Eye class="h-3 w-3 mr-0.5" />
                详情
              </el-button>
              <el-button size="small" type="primary" plain @click="openDetailDrawer(row, 'edit')">
                <Edit3 class="h-3 w-3" />
              </el-button>
              <el-button size="small" type="danger" plain @click="openSingleDelete(row)">
                <Trash2 class="h-3 w-3" />
              </el-button>
            </div>
          </template>
        </el-table-column>
      </el-table>

      <div class="flex flex-wrap items-center justify-between gap-3 pt-1">
        <span class="text-xs text-slate-500">
          受深翻页保护限制，单页上限 100 条，最多浏览前 1000 页。
        </span>
        <el-pagination
          v-model:current-page="filters.page"
          v-model:page-size="filters.page_size"
          :page-sizes="[10, 20, 50, 100]"
          :total="total"
          layout="sizes, prev, pager, next"
          size="small"
          @current-change="fetchTorrents"
          @size-change="handleSearch"
        />
      </div>
    </div>

    <!-- 手动新增种子对话框 -->
    <el-dialog v-model="createVisible" title="手动新增种子记录" width="660px">
      <el-form label-width="120px" size="small">
        <el-form-item label="InfoHash (40Hex)">
          <div class="flex items-center gap-2 w-full">
            <el-input
              v-model="createForm.info_hash"
              placeholder="40 位十六进制字符串"
              class="font-mono flex-1"
            />
            <el-button @click="generateRandomHexHash">随机生成</el-button>
          </div>
        </el-form-item>
        <el-form-item label="原始标题 raw_name">
          <el-input v-model="createForm.raw_name" placeholder="例如 Ubuntu.24.04.LTS.Desktop.amd64" />
        </el-form-item>
        <el-form-item label="清洗标题 refine_name">
          <el-input v-model="createForm.refine_name" placeholder="留空则默认等于 raw_name" />
        </el-form-item>
        <el-form-item label="初始热度 hot">
          <el-input-number v-model="createForm.hot" :min="1" />
        </el-form-item>
        <el-form-item label="包含文件列表">
          <div class="w-full space-y-2">
            <div
              v-for="(f, idx) in createForm.files"
              :key="idx"
              class="flex items-center gap-2"
            >
              <el-input v-model="f.path" placeholder="相对路径，如 dir/file.mkv" class="flex-1" />
              <el-input-number v-model="f.size" :min="0" controls-position="right" style="width: 150px" />
              <el-button
                type="danger"
                plain
                :disabled="createForm.files.length <= 1"
                @click="createForm.files.splice(idx, 1)"
              >
                删
              </el-button>
            </div>
            <el-button size="small" plain @click="createForm.files.push({ path: '', size: 0 })">
              <Plus class="h-3 w-3 mr-1" />
              添加文件行
            </el-button>
          </div>
        </el-form-item>
        <el-form-item label="Redis 去重同步">
          <el-checkbox v-model="createForm.sync_redis_indexed">
            同步写入 Redis metadata:indexed:* 已入库标记
          </el-checkbox>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button size="small" @click="createVisible = false">取消</el-button>
        <el-button size="small" type="primary" :loading="createSubmitting" @click="submitCreate">
          写入 Manticore
        </el-button>
      </template>
    </el-dialog>

    <!-- 详情与编辑抽屉 -->
    <el-drawer
      v-model="detailVisible"
      :title="detailData ? detailData.refine_name || detailData.raw_name : '种子详情'"
      size="820px"
      destroy-on-close
    >
      <div class="space-y-4" v-loading="detailLoading">
        <div v-if="detailData" class="rounded-lg border border-slate-200 bg-slate-50/70 p-4 space-y-2 text-xs">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <div class="font-mono text-slate-700 break-all">
              <span class="text-slate-400">ID:</span> {{ detailData.id }} ·
              <span class="text-slate-400">Hash:</span> {{ detailData.info_hash }}
            </div>
            <div class="flex items-center gap-2">
              <el-button
                size="small"
                type="primary"
                plain
                @click="copyText(detailData.magnet_link, '磁力链接')"
              >
                <Magnet class="h-3.5 w-3.5 mr-1" />
                复制磁力链接
              </el-button>
              <el-button
                size="small"
                plain
                @click="copyText(detailData.info_hash, 'InfoHash')"
              >
                <Copy class="h-3.5 w-3.5 mr-1" />
                复制 Hash
              </el-button>
            </div>
          </div>
          <div class="flex flex-wrap gap-x-5 gap-y-1 text-slate-600">
            <span>原始标题: <strong class="text-slate-900">{{ detailData.raw_name }}</strong></span>
            <span>总大小: <strong class="font-mono text-slate-900">{{ formatBytes(detailData.total_size) }}</strong></span>
            <span>文件数: <strong class="font-mono text-slate-900">{{ detailData.file_count }}</strong></span>
            <span>热度: <strong class="font-mono text-rose-600">{{ detailData.hot }}</strong></span>
            <span>创建: {{ formatTs(detailData.created_at) }}</span>
            <span>更新: {{ formatTs(detailData.updated_at) }}</span>
          </div>
        </div>

        <el-tabs v-if="detailData" v-model="detailTab" type="border-card">
          <el-tab-pane label="文件清单浏览" name="files">
            <div class="space-y-3">
              <div class="flex flex-wrap items-center justify-between gap-2">
                <div class="flex items-center gap-2 flex-1 min-w-[220px]">
                  <el-input
                    v-model="fileSearchKeyword"
                    size="small"
                    clearable
                    placeholder="过滤抽屉内文件路径关键字..."
                    @input="fileLocalPage = 1"
                  />
                  <el-select v-model="fileSortMode" size="small" style="width: 120px">
                    <el-option label="按大小降序" value="size_desc" />
                    <el-option label="按大小升序" value="size_asc" />
                    <el-option label="按路径排序" value="path_asc" />
                  </el-select>
                </div>
                <el-radio-group v-model="fileViewMode" size="small">
                  <el-radio-button value="tree">
                    <span class="inline-flex items-center gap-1">
                      <FolderTree class="h-3.5 w-3.5" />
                      目录树
                    </span>
                  </el-radio-button>
                  <el-radio-button value="flat">
                    <span class="inline-flex items-center gap-1">
                      <List class="h-3.5 w-3.5" />
                      平铺列表
                    </span>
                  </el-radio-button>
                </el-radio-group>
              </div>

              <!-- 目录树视图 -->
              <div
                v-if="fileViewMode === 'tree'"
                class="rounded-lg border border-slate-200 p-2 max-h-[460px] overflow-auto"
              >
                <el-tree
                  v-if="fileTreeNodes.length > 0"
                  :data="fileTreeNodes"
                  node-key="id"
                  :default-expand-all="detailData.files.length <= 80"
                >
                  <template #default="{ data }">
                    <div class="flex items-center justify-between w-full pr-2 text-xs">
                      <span class="inline-flex items-center gap-1.5 truncate">
                        <Folder v-if="data.isDir" class="h-3.5 w-3.5 text-amber-500 shrink-0" />
                        <FileIcon v-else class="h-3.5 w-3.5 text-slate-400 shrink-0" />
                        <span :class="data.isDir ? 'font-medium text-slate-800' : 'text-slate-700'">
                          {{ data.label }}
                        </span>
                        <span v-if="data.isDir" class="text-[11px] text-slate-400">
                          ({{ data.fileCount }} 项)
                        </span>
                      </span>
                      <span class="font-mono text-[11px] text-slate-500 shrink-0 ml-2">
                        {{ formatBytes(data.size) }}
                      </span>
                    </div>
                  </template>
                </el-tree>
                <div v-else class="text-center py-8 text-xs text-slate-400">无匹配文件</div>
              </div>

              <!-- 平铺分页视图 -->
              <div v-else class="space-y-2.5">
                <el-table :data="pagedFlatFiles" size="small" border stripe max-height="420">
                  <el-table-column prop="path" label="文件路径" min-width="320" show-overflow-tooltip>
                    <template #default="{ row }">
                      <span class="font-mono text-xs">{{ row.path }}</span>
                    </template>
                  </el-table-column>
                  <el-table-column label="大小" width="120">
                    <template #default="{ row }">
                      <span class="font-mono text-xs">{{ formatBytes(row.size) }}</span>
                    </template>
                  </el-table-column>
                </el-table>
                <div class="flex items-center justify-between text-xs text-slate-500">
                  <span>共 {{ filteredSortedFiles.length }} 个文件</span>
                  <el-pagination
                    v-model:current-page="fileLocalPage"
                    :page-size="fileLocalPageSize"
                    :total="filteredSortedFiles.length"
                    layout="prev, pager, next"
                    size="small"
                  />
                </div>
              </div>
            </div>
          </el-tab-pane>

          <el-tab-pane label="编辑标题 / 热度 / 文件" name="edit">
            <el-form label-width="115px" size="small">
              <el-form-item label="raw_name">
                <el-input v-model="editForm.raw_name" />
              </el-form-item>
              <el-form-item label="refine_name">
                <el-input v-model="editForm.refine_name" />
              </el-form-item>
              <el-form-item label="热度 hot">
                <el-input-number v-model="editForm.hot" :min="0" />
              </el-form-item>
              <el-form-item label="文件列表编辑">
                <div class="w-full space-y-2 max-h-72 overflow-auto pr-1">
                  <div
                    v-for="(f, idx) in editForm.files"
                    :key="idx"
                    class="flex items-center gap-2"
                  >
                    <el-input v-model="f.path" class="flex-1" />
                    <el-input-number
                      v-model="f.size"
                      :min="0"
                      controls-position="right"
                      style="width: 140px"
                    />
                    <el-button
                      type="danger"
                      plain
                      @click="editForm.files.splice(idx, 1)"
                    >
                      删
                    </el-button>
                  </div>
                </div>
                <el-button class="mt-2" size="small" plain @click="editForm.files.push({ path: '', size: 0 })">
                  <Plus class="h-3 w-3 mr-1" />
                  新增文件项
                </el-button>
              </el-form-item>
              <el-form-item>
                <el-button type="primary" :loading="editSubmitting" @click="submitEdit">
                  保存修改（Manticore 整行 Replace）
                </el-button>
              </el-form-item>
            </el-form>
          </el-tab-pane>
        </el-tabs>
      </div>
    </el-drawer>

    <!-- 删除 / 批量删除确认对话框 -->
    <el-dialog
      v-model="deleteVisible"
      :title="deleteMode === 'single' ? '确认删除种子记录' : `确认批量删除 ${selectedRows.length} 条记录`"
      width="480px"
    >
      <div class="space-y-3 text-xs text-slate-700">
        <p v-if="deleteMode === 'single' && deleteTarget">
          即将从 Manticore `torrents` 表永久删除：
          <strong class="block mt-1 text-slate-900 break-words">
            {{ deleteTarget.refine_name || deleteTarget.raw_name }}
          </strong>
          <span class="block mt-0.5 font-mono text-slate-500">{{ deleteTarget.info_hash }}</span>
        </p>
        <p v-else>
          即将从 Manticore `torrents` 表永久删除选中的 <strong>{{ selectedRows.length }}</strong> 条记录。
        </p>

        <div class="rounded-lg border border-amber-200 bg-amber-50 p-3 space-y-1.5">
          <el-checkbox v-model="keepRedisIndexed">
            <span class="font-medium text-slate-900">
              保留 Redis metadata:indexed:* 去重标记（防再次抓取）
            </span>
          </el-checkbox>
          <p class="text-[11px] text-amber-800 pl-6">
            {{
              keepRedisIndexed
                ? "勾选（默认）：删除检索记录但保留去重标记，Crawler 后续再遇到相同 Hash 时将直接跳过。"
                : "未勾选：同时删除 Redis 去重标记与 Pending 占坑锁，Crawler 再次发现该 Hash 时可重新下载入库。"
            }}
          </p>
        </div>
      </div>

      <template #footer>
        <el-button size="small" @click="deleteVisible = false">取消</el-button>
        <el-button size="small" type="danger" :loading="deleteSubmitting" @click="confirmDelete">
          确认删除
        </el-button>
      </template>
    </el-dialog>
  </div>
</template>
