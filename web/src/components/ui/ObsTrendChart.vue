<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { fmtAxis } from "@/utils/format";

export interface TrendChartSeries {
  name: string;
  color: string;
  values: number[];
  raw?: number[];
  area?: boolean;
  dash?: boolean;
  fmt: (v: number) => string;
}

const props = withDefaults(
  defineProps<{
    labels: string[];
    series: TrendChartSeries[];
    maxVal?: number;
    heightClass?: string;
    emptyText?: string;
  }>(),
  {
    maxVal: undefined,
    heightClass: "h-56",
    emptyText: "暂无历史趋势采样数据",
  },
);

const uid = `otc_${Math.random().toString(36).slice(2, 9)}`;
const wrapRef = ref<HTMLDivElement | null>(null);
const width = ref(680);
const height = ref(224);
const hoverIdx = ref<number | null>(null);

let resizeObs: ResizeObserver | null = null;

function measureWrap() {
  const el = wrapRef.value;
  if (!el) return;
  const rect = el.getBoundingClientRect();
  if (rect.width > 0) width.value = Math.max(320, Math.round(rect.width));
  if (rect.height > 0) height.value = Math.max(180, Math.round(rect.height));
}

onMounted(() => {
  measureWrap();
  if (typeof ResizeObserver !== "undefined" && wrapRef.value) {
    resizeObs = new ResizeObserver(() => measureWrap());
    resizeObs.observe(wrapRef.value);
  }
  window.addEventListener("resize", measureWrap);
});

onUnmounted(() => {
  resizeObs?.disconnect();
  resizeObs = null;
  window.removeEventListener("resize", measureWrap);
});

const padL = 44;
const padR = 14;
const padT = 14;
const padB = 26;

const plotW = computed(() => Math.max(120, width.value - padL - padR));
const plotH = computed(() => Math.max(80, height.value - padT - padB));

const effectiveMax = computed(() => {
  if (props.maxVal && props.maxVal > 0) return props.maxVal;
  let peak = 0;
  for (const s of props.series) {
    for (const v of s.values) {
      if (Number.isFinite(v) && v > peak) peak = v;
    }
  }
  if (peak <= 0) return 10;
  return peak * 1.15;
});

const yTicks = computed(() => {
  const maxV = effectiveMax.value;
  return [0, 1, 2, 3, 4].map((i) => ({
    y: padT + (plotH.value / 4) * i,
    label: fmtAxis(maxV * (1 - i / 4)),
    isZero: i === 4,
  }));
});

const stepX = computed(() => {
  const n = props.labels.length;
  return n > 1 ? plotW.value / (n - 1) : plotW.value;
});

function xForIndex(i: number): number {
  return props.labels.length > 1 ? padL + stepX.value * i : padL + plotW.value / 2;
}

const xTicks = computed(() => {
  const n = props.labels.length;
  if (n === 0) return [];
  const maxTicks = Math.max(4, Math.floor(plotW.value / 76));
  const tickStep = n > 1 ? Math.max(1, Math.ceil((n - 1) / Math.max(1, maxTicks - 1))) : 1;
  const out: Array<{ idx: number; x: number; label: string; anchor: "start" | "middle" | "end" }> = [];
  for (let i = 0; i < n; i++) {
    const show = i === 0 || i === n - 1 || (i % tickStep === 0 && n - 1 - i >= Math.ceil(tickStep * 0.5));
    if (!show) continue;
    out.push({
      idx: i,
      x: xForIndex(i),
      label: props.labels[i],
      anchor: i === 0 ? "start" : i === n - 1 ? "end" : "middle",
    });
  }
  return out;
});

function buildSmoothPath(pts: Array<[number, number]>): string {
  if (!pts.length) return "";
  if (pts.length === 1) return `M ${pts[0][0]} ${pts[0][1]}`;
  let d = `M ${pts[0][0]} ${pts[0][1]}`;
  for (let i = 1; i < pts.length; i++) {
    const xc = (pts[i - 1][0] + pts[i][0]) / 2;
    const yc = (pts[i - 1][1] + pts[i][1]) / 2;
    d += ` Q ${pts[i - 1][0]} ${pts[i - 1][1]}, ${xc} ${yc}`;
  }
  const last = pts[pts.length - 1];
  return `${d} T ${last[0]} ${last[1]}`;
}

const computedSeries = computed(() => {
  const maxV = effectiveMax.value;
  return props.series.map((s, idx) => {
    const pts: Array<[number, number]> = s.values.map((v, i) => [
      xForIndex(i),
      padT + plotH.value - Math.min(1, Math.max(0, v / maxV)) * plotH.value,
    ]);
    const path = buildSmoothPath(pts);
    const areaPath =
      s.area && pts.length > 0
        ? `${path} L ${pts[pts.length - 1][0]} ${padT + plotH.value} L ${pts[0][0]} ${padT + plotH.value} Z`
        : "";
    return {
      ...s,
      gradId: `${uid}_g${idx}`,
      pts,
      path,
      areaPath,
    };
  });
});

const hitRects = computed(() => {
  const n = props.labels.length;
  return props.labels.map((_, i) => {
    const x = xForIndex(i);
    const leftEdge = i === 0 ? padL : Math.max(padL, x - stepX.value / 2);
    const rightEdge = i === n - 1 ? padL + plotW.value : Math.min(padL + plotW.value, x + stepX.value / 2);
    return {
      idx: i,
      x: leftEdge,
      w: Math.max(1, rightEdge - leftEdge),
    };
  });
});

const hoverX = computed(() => (hoverIdx.value !== null ? xForIndex(hoverIdx.value) : 0));

const tooltipStyle = computed(() => {
  if (hoverIdx.value === null) return {};
  const x = hoverX.value;
  if (x > width.value * 0.58) {
    return { left: "auto", right: `${Math.max(12, Math.round(width.value - x + 14))}px` };
  }
  return { right: "auto", left: `${Math.max(padL + 8, Math.round(x + 14))}px` };
});
</script>

<template>
  <div ref="wrapRef" class="relative w-full" :class="heightClass">
    <div
      v-if="labels.length === 0"
      class="h-full w-full flex flex-col items-center justify-center text-xs text-slate-500 font-mono-num"
    >
      <span>{{ emptyText }}</span>
    </div>

    <svg
      v-else
      :viewBox="`0 0 ${width} ${height}`"
      preserveAspectRatio="xMidYMid meet"
      class="block w-full h-full overflow-visible"
      @mouseleave="hoverIdx = null"
    >
      <defs>
        <template v-for="s in computedSeries" :key="s.gradId">
          <linearGradient v-if="s.area" :id="s.gradId" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" :stop-color="s.color" stop-opacity="0.24" />
            <stop offset="100%" :stop-color="s.color" stop-opacity="0.01" />
          </linearGradient>
        </template>
      </defs>

      <!-- Y 轴网格与刻度 -->
      <g v-for="(tick, idx) in yTicks" :key="idx">
        <line
          :x1="padL"
          :y1="tick.y"
          :x2="width - padR"
          :y2="tick.y"
          :stroke="tick.isZero ? 'rgba(148,163,184,0.2)' : 'rgba(148,163,184,0.1)'"
          :stroke-dasharray="tick.isZero ? undefined : '3 3'"
        />
        <text
          :x="padL - 8"
          :y="tick.y + 3"
          text-anchor="end"
          fill="#64748b"
          font-size="10"
          font-family="monospace"
        >
          {{ tick.label }}
        </text>
      </g>

      <!-- X 轴时间刻度 -->
      <text
        v-for="xt in xTicks"
        :key="xt.idx"
        :x="xt.x"
        :y="height - 6"
        :text-anchor="xt.anchor"
        fill="#64748b"
        font-size="10"
        font-family="monospace"
      >
        {{ xt.label }}
      </text>

      <!-- 曲线与填充区域 -->
      <g v-for="s in computedSeries" :key="s.name">
        <path v-if="s.area && s.areaPath" :d="s.areaPath" :fill="`url(#${s.gradId})`" />
        <path
          :d="s.path"
          fill="none"
          :stroke="s.color"
          stroke-width="2"
          :stroke-dasharray="s.dash ? '4 4' : undefined"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
      </g>

      <!-- 悬停指示线与高亮点 -->
      <line
        v-if="hoverIdx !== null"
        :x1="hoverX"
        :y1="padT"
        :x2="hoverX"
        :y2="padT + plotH"
        stroke="rgba(226,232,240,0.35)"
        stroke-dasharray="2 2"
        pointer-events="none"
      />
      <template v-if="hoverIdx !== null">
        <circle
          v-for="s in computedSeries"
          :key="`dot-${s.name}`"
          :cx="s.pts[hoverIdx]?.[0] ?? 0"
          :cy="s.pts[hoverIdx]?.[1] ?? 0"
          r="4"
          :fill="s.color"
          stroke="#0a0e16"
          stroke-width="2"
          pointer-events="none"
        />
      </template>

      <!-- 透明拾取热区 -->
      <rect
        v-for="hr in hitRects"
        :key="`hit-${hr.idx}`"
        :x="hr.x"
        :y="padT"
        :width="hr.w"
        :height="plotH"
        fill="transparent"
        class="cursor-crosshair"
        @mouseenter="hoverIdx = hr.idx"
      />
    </svg>

    <!-- 悬停切片浮窗 -->
    <div
      v-if="hoverIdx !== null && labels[hoverIdx]"
      class="pointer-events-none absolute top-2 z-10 min-w-[152px] rounded-lg bg-[#0d121d]/95 border border-slate-700/90 px-3 py-2 text-[11px] font-mono-num shadow-xl backdrop-blur-xs space-y-1"
      :style="tooltipStyle"
    >
      <div class="text-slate-400 border-b border-slate-800 pb-1 mb-1">{{ labels[hoverIdx] }}</div>
      <div
        v-for="s in series"
        :key="`tip-${s.name}`"
        class="flex items-center justify-between gap-4"
      >
        <span :style="{ color: s.color }">{{ s.name }}</span>
        <span class="text-slate-100">
          {{ s.fmt((s.raw ? s.raw[hoverIdx] : s.values[hoverIdx]) ?? 0) }}
        </span>
      </div>
    </div>
  </div>
</template>
