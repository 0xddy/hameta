<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { RouterLink } from "vue-router";
import {
  getGlobeSnapshot,
  GLOBE_STREAM_URL,
  type GlobeCountryTraffic,
  type GlobeTrafficSnapshot,
} from "@/api/globe";
import ObsSelect from "@/components/ui/ObsSelect.vue";
import {
  GLOBE_HOME_HUBS,
  GLOBE_LAYER_STYLE,
  resolveCountryGeo,
  resolveHomeHub,
  type GlobeLayerKey,
} from "@/data/globeGeo";
import { WORLD_TOPO } from "@/data/worldTopo";
import { useTelemetryStore } from "@/stores/telemetry";
import { fmtBps, fmtBytes, fmtCount, fmtExact, fmtRate, formatTime } from "@/utils/format";

interface LonLatRing extends Array<[number, number]> {}
interface DecodedCountryGeom {
  isoNum: string;
  rings: LonLatRing[];
}

interface ProjectedArc {
  isoCode: string;
  countryName: string;
  pps: number;
  bps: number;
  sharePct: number;
  fromLat: number;
  fromLon: number;
  layer: Exclude<GlobeLayerKey, "all">;
  color: string;
  glow: string;
  weight: number;
}

interface PulseLogEntry {
  id: number;
  ts: number;
  isoCode: string;
  name: string;
  channel: Exclude<GlobeLayerKey, "all">;
  pps: number;
  bps: number;
}

// Pre-decode WORLD_TOPO once on module load
const DECODED_WORLD: DecodedCountryGeom[] = (() => {
  const { scale, translate } = WORLD_TOPO.t;
  const decodedArcs: LonLatRing[] = WORLD_TOPO.a.map((rawArc) => {
    const pts: LonLatRing = [];
    let x = 0;
    let y = 0;
    for (let i = 0; i < rawArc.length; i += 1) {
      x += rawArc[i][0];
      y += rawArc[i][1];
      pts.push([x * scale[0] + translate[0], y * scale[1] + translate[1]]);
    }
    return pts;
  });

  function stitchRing(arcIndices: number[]): LonLatRing {
    const ring: LonLatRing = [];
    for (const rawIdx of arcIndices) {
      const rev = rawIdx < 0;
      const idx = rev ? ~rawIdx : rawIdx;
      const src = decodedArcs[idx];
      if (!src) continue;
      if (rev) {
        for (let k = src.length - 1; k >= 0; k -= 1) {
          if (ring.length === 0 || k < src.length - 1) ring.push(src[k]);
        }
      } else {
        for (let k = 0; k < src.length; k += 1) {
          if (ring.length === 0 || k > 0) ring.push(src[k]);
        }
      }
    }
    return ring;
  }

  const out: DecodedCountryGeom[] = [];
  for (const g of WORLD_TOPO.g) {
    const isoNum = String(g[0]);
    const kind = g[1];
    const rawData = g[2];
    const rings: LonLatRing[] = [];
    if (kind === 0 && Array.isArray(rawData)) {
      const polyRings = rawData as number[][];
      if (polyRings[0]) rings.push(stitchRing(polyRings[0]));
    } else if (kind === 1 && Array.isArray(rawData)) {
      const polys = rawData as number[][][];
      for (const poly of polys) {
        if (poly[0]) rings.push(stitchRing(poly[0]));
      }
    }
    if (rings.length > 0) out.push({ isoNum, rings });
  }
  return out;
})();

const STARS = Array.from({ length: 110 }, (_, i) => {
  const s1 = Math.sin(i * 12.9898) * 43758.5453;
  const s2 = Math.sin((i + 77) * 78.233) * 19341.123;
  const rx = s1 - Math.floor(s1);
  const ry = s2 - Math.floor(s2);
  return { x: rx, y: ry, r: 0.5 + (i % 3) * 0.45, a: 0.16 + ((i * 37) % 10) * 0.03 };
});

const telemetry = useTelemetryStore();

const snap = ref<GlobeTrafficSnapshot | null>(null);
const sseConnected = ref(false);
const activeLayer = ref<GlobeLayerKey>("all");
const manualHubCode = ref<string | null>(null);
const autoRotate = ref(true);
const focusedCountry = ref<string | null>(null);
const pulseLogs = ref<PulseLogEntry[]>([]);
const hoveredCountry = ref<{
  isoCode: string;
  name: string;
  pps: number;
  bps: number;
  sharePct: number;
  x: number;
  y: number;
} | null>(null);

// Camera orientation (deg & scale)
const camLon = ref(26);
const camLat = ref(20);
const camZoom = ref(1.0);
let targetLon: number | null = null;
let targetLat: number | null = null;
let velLon = 0;
let velLat = 0;
let dragging = false;
let lastMouseX = 0;
let lastMouseY = 0;
let dragMoved = false;

const canvasWrapRef = ref<HTMLDivElement | null>(null);
const canvasRef = ref<HTMLCanvasElement | null>(null);
let rafId: number | null = null;
let resizeObs: ResizeObserver | null = null;
let es: EventSource | null = null;
let reconnectTimer: number | null = null;
let reconnectAttempt = 0;
let pulseSeq = 1;
let hitZones: Array<{ isoCode: string; name: string; pps: number; bps: number; sharePct: number; x: number; y: number; r: number }> = [];

const hubOptions = computed(() =>
  Object.values(GLOBE_HOME_HUBS).map((h) => ({
    label: h.name,
    value: h.code,
  })),
);

const effectiveHubCode = computed({
  get: () => manualHubCode.value || snap.value?.geoip.home_country || "CN",
  set: (v: string) => {
    manualHubCode.value = v;
    const hub = resolveHomeHub(v);
    focusLatLon(hub.lat, hub.lon);
  },
});

const homeHub = computed(() => resolveHomeHub(effectiveHubCode.value));

const geoState = computed<"loading" | "active" | "disabled" | "idle_ready" | "missing">(() => {
  const s = snap.value;
  if (!s) return "loading";
  if (s.geoip.mmdb_loaded) return "active";
  if (!s.geoip.enabled) return "disabled";
  if (s.running_crawlers === 0 && (s.geoip.mmdb_file_ready || s.geoip.mmdb_size_bytes > 0)) {
    return "idle_ready";
  }
  return "missing";
});

const topCountries = computed(() => (snap.value?.countries ?? []).slice(0, 8));

const ipv4Share = computed(() => {
  const s = snap.value;
  if (!s) return 0;
  const total = s.ipv4_pps + s.ipv6_pps;
  return total > 0 ? (s.ipv4_pps / total) * 100 : 100;
});

const validPacketPct = computed(() => {
  const s = snap.value;
  if (!s) return null;
  if (s.udp_rx_pps <= 0) return 100;
  const bad = s.channels.parse_error_pps + s.channels.invalid_udp_pps + s.channels.queue_full_pps;
  return Math.max(0, Math.min(100, ((s.udp_rx_pps - bad) / s.udp_rx_pps) * 100));
});

const packetMix = computed(() => {
  const c = snap.value?.channels;
  if (!c) {
    return { getPeers: 0, announce: 0, response: 0, findPing: 0 };
  }
  const g = Math.max(0, c.get_peers_pps);
  const a = Math.max(0, c.announce_peer_pps);
  const r = Math.max(0, c.response_pps);
  const f = Math.max(0, c.find_node_ping_pps);
  const total = g + a + r + f;
  if (total <= 0) {
    return { getPeers: 0, announce: 0, response: 0, findPing: 0 };
  }
  const pg = Math.round((g / total) * 100);
  const pa = Math.round((a / total) * 100);
  const pr = Math.round((r / total) * 100);
  const pf = Math.max(0, 100 - pg - pa - pr);
  return { getPeers: pg, announce: pa, response: pr, findPing: pf };
});

const LAYER_FILTER_PILLS: Array<{ key: GlobeLayerKey; label: string; dotCls?: string }> = [
  { key: "all", label: "全部" },
  { key: "get_peers", label: "get_peers", dotCls: "bg-cyan-400" },
  { key: "announce_peer", label: "announce_peer", dotCls: "bg-emerald-400" },
  { key: "response", label: "response", dotCls: "bg-amber-400" },
  { key: "find_node_ping", label: "find / ping", dotCls: "bg-indigo-400" },
];

const countryHeatByIsoNum = computed(() => {
  const map = new Map<string, { country: GlobeCountryTraffic; intensity: number }>();
  const list = snap.value?.countries ?? [];
  const maxPps = Math.max(1, ...list.map((c) => layerCountryPps(c, activeLayer.value)));
  for (const c of list) {
    const geo = resolveCountryGeo(c.iso_code);
    if (!geo) continue;
    const val = layerCountryPps(c, activeLayer.value);
    map.set(geo.isoNum, { country: c, intensity: Math.min(1, val / maxPps) });
  }
  return map;
});

const activeArcs = computed<ProjectedArc[]>(() => {
  const list = snap.value?.countries ?? [];
  const hub = homeHub.value;
  const filtered = list
    .map((c) => ({ c, rate: layerCountryPps(c, activeLayer.value) }))
    .filter((x) => x.rate > 0)
    .slice(0, 24);
  const maxRate = Math.max(1, ...filtered.map((x) => x.rate));

  const arcs: ProjectedArc[] = [];
  for (const { c, rate } of filtered) {
    const geo = resolveCountryGeo(c.iso_code);
    if (!geo) continue;
    if (geo.code === hub.code && Math.abs(geo.lat - hub.lat) < 5) continue;
    const dom = dominantChannel(c, activeLayer.value);
    const style = GLOBE_LAYER_STYLE[dom];
    arcs.push({
      isoCode: geo.code,
      countryName: geo.name,
      pps: rate,
      bps: c.bps,
      sharePct: c.share_pct,
      fromLat: geo.lat,
      fromLon: geo.lon,
      layer: dom,
      color: style.color,
      glow: style.glow,
      weight: 0.35 + (rate / maxRate) * 0.65,
    });
  }
  return arcs;
});

function layerCountryPps(c: GlobeCountryTraffic, layer: GlobeLayerKey): number {
  switch (layer) {
    case "get_peers":
      return c.get_peers_pps;
    case "announce_peer":
      return c.announce_peer_pps;
    case "response":
      return c.response_pps;
    case "find_node_ping":
      return c.find_node_ping_pps;
    default:
      return c.pps;
  }
}

function dominantChannel(
  c: GlobeCountryTraffic,
  layer: GlobeLayerKey,
): Exclude<GlobeLayerKey, "all"> {
  if (layer !== "all") return layer;
  const pairs: Array<[Exclude<GlobeLayerKey, "all">, number]> = [
    ["get_peers", c.get_peers_pps],
    ["announce_peer", c.announce_peer_pps],
    ["response", c.response_pps],
    ["find_node_ping", c.find_node_ping_pps],
  ];
  pairs.sort((a, b) => b[1] - a[1]);
  return pairs[0][0];
}

function recordPulseFromSnapshot(next: GlobeTrafficSnapshot) {
  const nonZero = next.countries.filter((c) => c.pps > 0).slice(0, 3);
  if (nonZero.length === 0) return;
  const entries: PulseLogEntry[] = nonZero.map((c) => {
    const geo = resolveCountryGeo(c.iso_code);
    return {
      id: pulseSeq++,
      ts: next.captured_at,
      isoCode: c.iso_code,
      name: geo ? `${geo.code} ${geo.name}` : c.iso_code,
      channel: dominantChannel(c, "all"),
      pps: c.pps,
      bps: c.bps,
    };
  });
  pulseLogs.value = [...entries, ...pulseLogs.value].slice(0, 18);
}

function focusLatLon(lat: number, lon: number) {
  targetLat = Math.max(-55, Math.min(68, lat));
  targetLon = lon;
}

function resetCamera() {
  const hub = homeHub.value;
  focusedCountry.value = null;
  camZoom.value = 1.0;
  autoRotate.value = true;
  focusLatLon(Math.max(12, Math.min(48, hub.lat)), hub.lon);
}

function adjustZoom(delta: number) {
  camZoom.value = Math.max(0.7, Math.min(2.4, Number((camZoom.value + delta).toFixed(2))));
}

function focusCountry(isoCode: string) {
  const geo = resolveCountryGeo(isoCode);
  if (!geo) return;
  focusedCountry.value = geo.code;
  autoRotate.value = false;
  focusLatLon(geo.lat, geo.lon);
}

function clearFocus() {
  focusedCountry.value = null;
  resetCamera();
}

// 3D math helpers
const DEG2RAD = Math.PI / 180;

function llToVec3(lat: number, lon: number): [number, number, number] {
  const phi = lat * DEG2RAD;
  const lam = lon * DEG2RAD;
  const cosPhi = Math.cos(phi);
  return [cosPhi * Math.sin(lam), Math.sin(phi), cosPhi * Math.cos(lam)];
}

function slerpVec3(
  a: [number, number, number],
  b: [number, number, number],
  t: number,
): [number, number, number] {
  let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  dot = Math.max(-1, Math.min(1, dot));
  const omega = Math.acos(dot);
  if (omega < 0.001) {
    return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
  }
  const sinOmega = Math.sin(omega);
  const s0 = Math.sin((1 - t) * omega) / sinOmega;
  const s1 = Math.sin(t * omega) / sinOmega;
  return [a[0] * s0 + b[0] * s1, a[1] * s0 + b[1] * s1, a[2] * s0 + b[2] * s1];
}

function rotatePoint(
  v: [number, number, number],
  cLon: number,
  cLat: number,
): [number, number, number] {
  // Rotate around Y axis by -camLon, then X axis by camLat
  const lam = -cLon * DEG2RAD;
  const cosL = Math.cos(lam);
  const sinL = Math.sin(lam);
  const x1 = v[0] * cosL + v[2] * sinL;
  const y1 = v[1];
  const z1 = -v[0] * sinL + v[2] * cosL;

  const phi = cLat * DEG2RAD;
  const cosP = Math.cos(phi);
  const sinP = Math.sin(phi);
  const y2 = y1 * cosP - z1 * sinP;
  const z2 = y1 * sinP + z1 * cosP;
  return [x1, y2, z2];
}

function projectVec(
  v: [number, number, number],
  cx: number,
  cy: number,
  radius: number,
  altitude = 1,
): { x: number; y: number; z: number } {
  const rv = rotatePoint(v, camLon.value, camLat.value);
  return {
    x: cx + rv[0] * radius * altitude,
    y: cy - rv[1] * radius * altitude,
    z: rv[2],
  };
}

function renderGlobeFrame(nowMs: number) {
  const canvas = canvasRef.value;
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;

  const dpr = Math.min(2, window.devicePixelRatio || 1);
  const w = canvas.width / dpr;
  const h = canvas.height / dpr;
  if (w < 40 || h < 40) return;

  // Camera inertia & smooth easing
  if (!dragging) {
    if (targetLon !== null && targetLat !== null) {
      let dLon = targetLon - camLon.value;
      while (dLon > 180) dLon -= 360;
      while (dLon < -180) dLon += 360;
      const dLat = targetLat - camLat.value;
      camLon.value += dLon * 0.12;
      camLat.value += dLat * 0.12;
      if (Math.abs(dLon) < 0.15 && Math.abs(dLat) < 0.15) {
        camLon.value = targetLon;
        camLat.value = targetLat;
        targetLon = null;
        targetLat = null;
      }
    } else {
      camLon.value += velLon;
      camLat.value = Math.max(-55, Math.min(68, camLat.value + velLat));
      velLon *= 0.92;
      velLat *= 0.92;
      if (autoRotate.value && Math.abs(velLon) < 0.02) {
        camLon.value += 0.11;
      }
    }
    while (camLon.value > 180) camLon.value -= 360;
    while (camLon.value < -180) camLon.value += 360;
  }

  ctx.save();
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, w, h);

  const cx = w * 0.5;
  const cy = h * 0.52;
  const R = Math.min(w, h) * 0.365 * camZoom.value;

  // Layer 1: Deep space stars + outer Fresnel glow
  for (const st of STARS) {
    ctx.fillStyle = `rgba(148, 163, 184, ${st.a})`;
    ctx.beginPath();
    ctx.arc(st.x * w, st.y * h, st.r, 0, Math.PI * 2);
    ctx.fill();
  }

  const haloGrad = ctx.createRadialGradient(cx, cy, R * 0.88, cx, cy, R * 1.28);
  haloGrad.addColorStop(0, "rgba(16, 185, 129, 0.16)");
  haloGrad.addColorStop(0.55, "rgba(6, 182, 212, 0.08)");
  haloGrad.addColorStop(1, "rgba(6, 182, 212, 0)");
  ctx.fillStyle = haloGrad;
  ctx.beginPath();
  ctx.arc(cx, cy, R * 1.28, 0, Math.PI * 2);
  ctx.fill();

  // Layer 2 (Back): Satellite orbit ring back half
  ctx.save();
  ctx.translate(cx, cy);
  ctx.rotate(-0.42);
  ctx.beginPath();
  ctx.ellipse(0, 0, R * 1.32, R * 0.34, 0, Math.PI, Math.PI * 2);
  ctx.strokeStyle = "rgba(16, 185, 129, 0.18)";
  ctx.lineWidth = 1;
  ctx.stroke();
  ctx.restore();

  // Layer 3: Ocean sphere + graticule clipped inside globe circle
  ctx.save();
  ctx.beginPath();
  ctx.arc(cx, cy, R, 0, Math.PI * 2);
  ctx.clip();

  const oceanGrad = ctx.createRadialGradient(cx - R * 0.3, cy - R * 0.35, R * 0.1, cx, cy, R * 1.05);
  oceanGrad.addColorStop(0, "#10213a");
  oceanGrad.addColorStop(0.65, "#071020");
  oceanGrad.addColorStop(1, "#030712");
  ctx.fillStyle = oceanGrad;
  ctx.fillRect(cx - R, cy - R, R * 2, R * 2);

  // Graticule lines every 30 deg
  ctx.strokeStyle = "rgba(51, 65, 85, 0.28)";
  ctx.lineWidth = 0.7;
  for (let lat = -60; lat <= 60; lat += 30) {
    ctx.beginPath();
    let pen = false;
    for (let lon = -180; lon <= 180; lon += 6) {
      const p = projectVec(llToVec3(lat, lon), cx, cy, R);
      if (p.z > -0.02) {
        if (!pen) {
          ctx.moveTo(p.x, p.y);
          pen = true;
        } else {
          ctx.lineTo(p.x, p.y);
        }
      } else {
        pen = false;
      }
    }
    ctx.stroke();
  }
  for (let lon = -180; lon < 180; lon += 30) {
    ctx.beginPath();
    let pen = false;
    for (let lat = -84; lat <= 84; lat += 6) {
      const p = projectVec(llToVec3(lat, lon), cx, cy, R);
      if (p.z > -0.02) {
        if (!pen) {
          ctx.moveTo(p.x, p.y);
          pen = true;
        } else {
          ctx.lineTo(p.x, p.y);
        }
      } else {
        pen = false;
      }
    }
    ctx.stroke();
  }

  // Layer 4: World TopoJSON polygons & country traffic heat
  const heatMap = countryHeatByIsoNum.value;
  const hubIsoNum = homeHub.value.isoNum;

  for (const country of DECODED_WORLD) {
    const heat = heatMap.get(country.isoNum);
    const isHome = country.isoNum === hubIsoNum;
    let fill = "#0e1a2e";
    let stroke = "rgba(56, 82, 118, 0.55)";

    if (isHome) {
      fill = "rgba(16, 185, 129, 0.26)";
      stroke = "rgba(52, 211, 153, 0.75)";
    } else if (heat && heat.intensity > 0) {
      const alpha = (0.14 + heat.intensity * 0.44).toFixed(2);
      fill = `rgba(6, 182, 212, ${alpha})`;
      stroke = "rgba(34, 211, 238, 0.72)";
    }

    for (const ring of country.rings) {
      if (ring.length < 3) continue;
      let anyFront = false;
      const projected: Array<{ x: number; y: number; z: number }> = [];
      for (let i = 0; i < ring.length; i += 1) {
        const pt = projectVec(llToVec3(ring[i][1], ring[i][0]), cx, cy, R);
        if (pt.z > 0) anyFront = true;
        projected.push(pt);
      }
      if (!anyFront) continue;

      ctx.beginPath();
      for (let i = 0; i < projected.length; i += 1) {
        const pt = projected[i];
        // Clamp back-hemisphere vertices lightly onto limb for clean sphere silhouettes
        let px = pt.x;
        let py = pt.y;
        if (pt.z < 0) {
          const dx = px - cx;
          const dy = py - cy;
          const len = Math.hypot(dx, dy) || 1;
          px = cx + (dx / len) * R;
          py = cy + (dy / len) * R;
        }
        if (i === 0) ctx.moveTo(px, py);
        else ctx.lineTo(px, py);
      }
      ctx.closePath();
      ctx.fillStyle = fill;
      ctx.fill();
      ctx.strokeStyle = stroke;
      ctx.lineWidth = isHome || heat ? 0.95 : 0.65;
      ctx.stroke();
    }
  }

  ctx.restore(); // unclip sphere

  // Rim highlight around sphere
  ctx.beginPath();
  ctx.arc(cx, cy, R, 0, Math.PI * 2);
  ctx.strokeStyle = "rgba(34, 211, 238, 0.25)";
  ctx.lineWidth = 1.25;
  ctx.stroke();

  // Layer 5: Inbound SLERP traffic arcs + comets + source/target ripples
  const hub = homeHub.value;
  const hubVec = llToVec3(hub.lat, hub.lon);
  const arcs = activeArcs.value;
  const secFloat = nowMs / 1000;
  const visibleMarkers: Array<{
    isoCode: string;
    name: string;
    pps: number;
    bps: number;
    sharePct: number;
    x: number;
    y: number;
    color: string;
    weight: number;
  }> = [];

  hitZones = [];

  for (let i = 0; i < arcs.length; i += 1) {
    const arc = arcs[i];
    const fromVec = llToVec3(arc.fromLat, arc.fromLon);
    const steps = 28;
    const dot = Math.max(-1, Math.min(1, fromVec[0] * hubVec[0] + fromVec[1] * hubVec[1] + fromVec[2] * hubVec[2]));
    const ang = Math.acos(dot);
    const peakAlt = 1 + Math.min(0.42, 0.1 + (ang / Math.PI) * 0.32);

    ctx.beginPath();
    let started = false;
    for (let s = 0; s <= steps; s += 1) {
      const t = s / steps;
      const v = slerpVec3(fromVec, hubVec, t);
      const alt = 1 + Math.sin(t * Math.PI) * (peakAlt - 1);
      const pt = projectVec(v, cx, cy, R, alt);
      if (pt.z > -0.08) {
        if (!started) {
          ctx.moveTo(pt.x, pt.y);
          started = true;
        } else {
          ctx.lineTo(pt.x, pt.y);
        }
      } else {
        started = false;
      }
    }
    ctx.strokeStyle = arc.color;
    ctx.globalAlpha = 0.28 + arc.weight * 0.48;
    ctx.lineWidth = 0.9 + arc.weight * 1.7;
    ctx.stroke();
    ctx.globalAlpha = 1;

    // Animated inbound comet travelling source -> hub
    const speed = 0.35 + arc.weight * 0.45;
    const cometT = (secFloat * speed + i * 0.17) % 1;
    const cVec = slerpVec3(fromVec, hubVec, cometT);
    const cAlt = 1 + Math.sin(cometT * Math.PI) * (peakAlt - 1);
    const cPt = projectVec(cVec, cx, cy, R, cAlt);
    if (cPt.z > 0) {
      ctx.fillStyle = arc.color;
      ctx.shadowColor = arc.glow;
      ctx.shadowBlur = 8;
      ctx.beginPath();
      ctx.arc(cPt.x, cPt.y, 2 + arc.weight * 1.4, 0, Math.PI * 2);
      ctx.fill();
      ctx.shadowBlur = 0;
    }

    // Source hotspot dot
    const srcPt = projectVec(fromVec, cx, cy, R);
    if (srcPt.z > 0.08) {
      const rDot = 2.6 + arc.weight * 2.8;
      ctx.fillStyle = arc.color;
      ctx.beginPath();
      ctx.arc(srcPt.x, srcPt.y, rDot, 0, Math.PI * 2);
      ctx.fill();

      // Ripple pulse
      const ripplePhase = (secFloat * 0.9 + i * 0.31) % 1;
      ctx.strokeStyle = arc.color;
      ctx.globalAlpha = (1 - ripplePhase) * 0.45;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(srcPt.x, srcPt.y, rDot + ripplePhase * 9, 0, Math.PI * 2);
      ctx.stroke();
      ctx.globalAlpha = 1;

      visibleMarkers.push({
        isoCode: arc.isoCode,
        name: arc.countryName,
        pps: arc.pps,
        bps: arc.bps,
        sharePct: arc.sharePct,
        x: srcPt.x,
        y: srcPt.y,
        color: arc.color,
        weight: arc.weight,
      });
      hitZones.push({
        isoCode: arc.isoCode,
        name: arc.countryName,
        pps: arc.pps,
        bps: arc.bps,
        sharePct: arc.sharePct,
        x: srcPt.x,
        y: srcPt.y,
        r: Math.max(10, rDot + 6),
      });
    }
  }

  // Layer 6: Front orbital ring + Home Hub 3D elevated beacon spire
  ctx.save();
  ctx.translate(cx, cy);
  ctx.rotate(-0.42);
  ctx.beginPath();
  ctx.ellipse(0, 0, R * 1.32, R * 0.34, 0, 0, Math.PI);
  ctx.strokeStyle = "rgba(16, 185, 129, 0.24)";
  ctx.lineWidth = 1.1;
  ctx.stroke();
  ctx.restore();

  const hubBase = projectVec(hubVec, cx, cy, R, 1.0);
  const hubTip = projectVec(hubVec, cx, cy, R, 1.18);
  if (hubBase.z > 0) {
    const grad = ctx.createLinearGradient(hubBase.x, hubBase.y, hubTip.x, hubTip.y);
    grad.addColorStop(0, "rgba(16, 185, 129, 0.95)");
    grad.addColorStop(1, "rgba(52, 211, 153, 0.15)");
    ctx.strokeStyle = grad;
    ctx.lineWidth = 2.5;
    ctx.beginPath();
    ctx.moveTo(hubBase.x, hubBase.y);
    ctx.lineTo(hubTip.x, hubTip.y);
    ctx.stroke();

    const pPhase = (secFloat * 1.1) % 1;
    ctx.strokeStyle = "#34d399";
    ctx.globalAlpha = (1 - pPhase) * 0.65;
    ctx.lineWidth = 1.3;
    ctx.beginPath();
    ctx.arc(hubBase.x, hubBase.y, 4 + pPhase * 15, 0, Math.PI * 2);
    ctx.stroke();
    ctx.globalAlpha = 1;

    ctx.fillStyle = "#34d399";
    ctx.shadowColor = "rgba(52, 211, 153, 0.9)";
    ctx.shadowBlur = 10;
    ctx.beginPath();
    ctx.arc(hubTip.x, hubTip.y, 3.2, 0, Math.PI * 2);
    ctx.fill();
    ctx.shadowBlur = 0;
  }

  // Layer 7: Collision-free 2D labels
  const drawCtx: CanvasRenderingContext2D = ctx;
  const placedBoxes: Array<{ x1: number; y1: number; x2: number; y2: number }> = [];
  drawCtx.font = "500 10.5px 'JetBrains Mono', ui-monospace, monospace";

  function tryDrawTag(text: string, x: number, y: number, borderColor: string, textColor: string) {
    const tw = drawCtx.measureText(text).width;
    const bw = tw + 10;
    const bh = 18;
    const bx = x + 7;
    const by = y - bh * 0.5;
    if (bx < 8 || bx + bw > w - 8 || by < 8 || by + bh > h - 8) return;
    for (const box of placedBoxes) {
      if (!(bx + bw + 4 < box.x1 || bx - 4 > box.x2 || by + bh + 4 < box.y1 || by - 4 > box.y2)) {
        return;
      }
    }
    placedBoxes.push({ x1: bx, y1: by, x2: bx + bw, y2: by + bh });
    drawCtx.fillStyle = "rgba(10, 15, 28, 0.86)";
    drawCtx.strokeStyle = borderColor;
    drawCtx.lineWidth = 1;
    drawCtx.beginPath();
    drawCtx.roundRect(bx, by, bw, bh, 4);
    drawCtx.fill();
    drawCtx.stroke();
    drawCtx.fillStyle = textColor;
    drawCtx.textBaseline = "middle";
    drawCtx.fillText(text, bx + 5, by + bh * 0.5 + 0.5);
  }

  if (hubBase.z > 0.1) {
    tryDrawTag(`HOME · ${hub.code} ${hub.short}`, hubTip.x, hubTip.y, "rgba(52, 211, 153, 0.55)", "#6ee7b7");
  }

  visibleMarkers.sort((a, b) => b.weight - a.weight);
  for (const m of visibleMarkers.slice(0, 12)) {
    tryDrawTag(`${m.isoCode} ${m.name}`, m.x, m.y, "rgba(51, 65, 85, 0.7)", "#cbd5e1");
  }

  ctx.restore();
  rafId = window.requestAnimationFrame(renderGlobeFrame);
}

function resizeCanvas() {
  const wrap = canvasWrapRef.value;
  const canvas = canvasRef.value;
  if (!wrap || !canvas) return;
  const rect = wrap.getBoundingClientRect();
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  canvas.width = Math.max(300, Math.round(rect.width * dpr));
  canvas.height = Math.max(300, Math.round(rect.height * dpr));
  canvas.style.width = `${Math.max(300, Math.round(rect.width))}px`;
  canvas.style.height = `${Math.max(300, Math.round(rect.height))}px`;
}

function onPointerDown(ev: PointerEvent) {
  dragging = true;
  dragMoved = false;
  targetLon = null;
  targetLat = null;
  velLon = 0;
  velLat = 0;
  lastMouseX = ev.clientX;
  lastMouseY = ev.clientY;
  (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
}

function onPointerMove(ev: PointerEvent) {
  const canvas = canvasRef.value;
  if (dragging) {
    const dx = ev.clientX - lastMouseX;
    const dy = ev.clientY - lastMouseY;
    if (Math.abs(dx) > 2 || Math.abs(dy) > 2) dragMoved = true;
    const sens = 0.28 / camZoom.value;
    velLon = -dx * sens;
    velLat = dy * sens;
    camLon.value += velLon;
    camLat.value = Math.max(-55, Math.min(68, camLat.value + velLat));
    lastMouseX = ev.clientX;
    lastMouseY = ev.clientY;
    hoveredCountry.value = null;
    return;
  }

  if (!canvas) return;
  const rect = canvas.getBoundingClientRect();
  const mx = ev.clientX - rect.left;
  const my = ev.clientY - rect.top;
  const hit = hitZones.find((z) => Math.hypot(mx - z.x, my - z.y) <= z.r);
  if (hit) {
    hoveredCountry.value = {
      isoCode: hit.isoCode,
      name: hit.name,
      pps: hit.pps,
      bps: hit.bps,
      sharePct: hit.sharePct,
      x: mx,
      y: my,
    };
  } else {
    hoveredCountry.value = null;
  }
}

function onPointerUp(ev: PointerEvent) {
  if (!dragging) return;
  dragging = false;
  if (!dragMoved && hoveredCountry.value) {
    focusCountry(hoveredCountry.value.isoCode);
  }
  try {
    (ev.currentTarget as HTMLElement).releasePointerCapture(ev.pointerId);
  } catch {
    // ignore
  }
}

function onWheel(ev: WheelEvent) {
  ev.preventDefault();
  adjustZoom(ev.deltaY < 0 ? 0.12 : -0.12);
}

function closeStream() {
  if (reconnectTimer !== null) {
    window.clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  if (es) {
    es.close();
    es = null;
  }
}

function openStream() {
  closeStream();
  const conn = new EventSource(GLOBE_STREAM_URL, { withCredentials: true });
  es = conn;

  conn.addEventListener("globe", (msg: MessageEvent<string>) => {
    try {
      const payload = JSON.parse(msg.data) as GlobeTrafficSnapshot;
      snap.value = payload;
      sseConnected.value = true;
      reconnectAttempt = 0;
      recordPulseFromSnapshot(payload);
    } catch {
      // ignore invalid frame
    }
  });

  conn.onerror = () => {
    sseConnected.value = false;
    conn.close();
    if (es === conn) es = null;
    const delayMs = Math.min(12_000, 1500 * Math.pow(2, Math.min(reconnectAttempt, 3)));
    reconnectAttempt += 1;
    reconnectTimer = window.setTimeout(() => {
      openStream();
    }, delayMs);
  };
}

onMounted(async () => {
  telemetry.connect();
  try {
    const initial = await getGlobeSnapshot();
    snap.value = initial;
    const hub = resolveHomeHub(initial.geoip.home_country);
    camLon.value = hub.lon;
    camLat.value = Math.max(12, Math.min(48, hub.lat));
  } catch {
    // SSE stream will hydrate
  }
  openStream();

  resizeCanvas();
  if (typeof ResizeObserver !== "undefined" && canvasWrapRef.value) {
    resizeObs = new ResizeObserver(() => resizeCanvas());
    resizeObs.observe(canvasWrapRef.value);
  }
  rafId = window.requestAnimationFrame(renderGlobeFrame);
});

onUnmounted(() => {
  telemetry.disconnect();
  closeStream();
  resizeObs?.disconnect();
  resizeObs = null;
  if (rafId !== null) {
    window.cancelAnimationFrame(rafId);
    rafId = null;
  }
});
</script>

<template>
  <div class="space-y-4">
    <!-- Top 4 Adaptive-Unit Inbound DHT KPI Cards -->
    <div class="grid grid-cols-2 xl:grid-cols-4 gap-3.5">
      <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
        <div class="flex items-center justify-between gap-2 text-xs text-slate-400">
          <span>DHT 入站包速 (UDP RX)</span>
          <span
            class="px-1.5 py-0.5 rounded font-mono-num text-[10px]"
            :class="sseConnected ? 'bg-cyan-500/15 text-cyan-300' : 'bg-amber-500/15 text-amber-300'"
          >
            {{ sseConnected ? "LIVE" : "SYNC" }}
          </span>
        </div>
        <div
          class="text-xl font-bold font-mono-num text-cyan-300 truncate"
          :title="snap ? fmtExact(snap.udp_rx_pps, 'pkt/s') : undefined"
        >
          {{ snap ? fmtRate(snap.udp_rx_pps) : "—" }}
          <span class="text-xs font-normal text-slate-400">pkt/s</span>
        </div>
        <div class="text-[11px] font-mono-num text-slate-500 truncate">
          入站带宽 {{ snap ? fmtBps(snap.udp_rx_bps) : "—" }} · {{ snap?.running_crawlers ?? 0 }} 个 Crawler 聚合
        </div>
      </div>

      <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
        <div class="flex items-center justify-between gap-2 text-xs text-slate-400">
          <span>入站 InfoHash 捕获</span>
          <span class="px-1.5 py-0.5 rounded bg-emerald-500/15 text-emerald-300 font-mono-num text-[10px]">
            UDP
          </span>
        </div>
        <div
          class="text-xl font-bold font-mono-num text-emerald-300 truncate"
          :title="telemetry.snapshot ? fmtExact(telemetry.snapshot.funnel.hashes_per_min, '/min') : undefined"
        >
          {{ telemetry.snapshot ? fmtRate(telemetry.snapshot.funnel.hashes_per_min) : "—" }}
          <span class="text-xs font-normal text-slate-400">/min</span>
        </div>
        <div class="text-[11px] font-mono-num text-slate-500 truncate">
          get_peers + announce_peer 有效触发
        </div>
      </div>

      <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
        <div class="flex items-center justify-between gap-2 text-xs text-slate-400">
          <span>活跃来源国家</span>
          <span
            class="px-1.5 py-0.5 rounded font-mono-num text-[10px]"
            :class="
              geoState === 'active'
                ? 'bg-cyan-500/15 text-cyan-300'
                : geoState === 'idle_ready'
                  ? 'bg-emerald-500/15 text-emerald-300'
                  : geoState === 'disabled'
                    ? 'bg-slate-800 text-slate-400'
                    : 'bg-rose-500/15 text-rose-300'
            "
          >
            {{
              geoState === "active"
                ? `${snap?.active_country_count ?? 0} 国家/地区`
                : geoState === "idle_ready"
                  ? "MMDB 已就绪"
                  : geoState === "disabled"
                    ? "已停用"
                    : "未归因"
            }}
          </span>
        </div>
        <div class="text-xl font-bold font-mono-num text-slate-50 truncate">
          {{ snap ? snap.active_country_count : "—" }}
          <span class="text-xs font-normal text-slate-400">countries</span>
        </div>
        <div class="text-[11px] font-mono-num text-slate-500 truncate">
          双栈包占比 · IPv4 {{ ipv4Share.toFixed(0) }}% / IPv6 {{ (100 - ipv4Share).toFixed(0) }}%
        </div>
      </div>

      <div class="obs-card rounded-xl p-4 space-y-1.5 min-w-0">
        <div class="flex items-center justify-between gap-2 text-xs text-slate-400">
          <span>入站有效率 &amp; 丢包保护</span>
          <span class="font-mono-num text-[10px] text-amber-300">RateGuard</span>
        </div>
        <div class="text-xl font-bold font-mono-num text-slate-100 truncate">
          {{ validPacketPct !== null ? `${validPacketPct.toFixed(2)}%` : "—" }}
        </div>
        <div class="text-[11px] font-mono-num text-slate-500 truncate">
          队列满 {{ snap ? fmtCount(snap.channels.queue_full_pps) : "—" }}/s · 非法/错 {{ snap ? fmtCount(snap.channels.parse_error_pps + snap.channels.invalid_udp_pps) : "—" }}/s
        </div>
      </div>
    </div>

    <!-- Main 3D Globe Command Stage + Regional Telemetry Sidebar -->
    <div class="grid grid-cols-1 xl:grid-cols-12 gap-4 items-stretch">
      <!-- Left 8 cols: Full-Bleed 3D Stage with Floating Glass HUD -->
      <div
        ref="canvasWrapRef"
        class="xl:col-span-8 obs-card globe-stage-bg rounded-2xl overflow-hidden relative h-[620px] min-w-0 select-none"
      >
        <canvas
          ref="canvasRef"
          class="w-full h-full block cursor-grab active:cursor-grabbing"
          @pointerdown="onPointerDown"
          @pointermove="onPointerMove"
          @pointerup="onPointerUp"
          @wheel="onWheel"
        />

        <!-- Unified Top Floating HUD Row (Zero-Overlap) -->
        <div class="absolute top-3 left-3 right-3 flex flex-wrap items-center justify-between gap-2 pointer-events-none z-10">
          <div class="globe-hud-glass rounded-xl px-2.5 py-1.5 flex items-center gap-2 pointer-events-auto min-w-0">
            <span class="inline-flex items-center gap-1.5 text-xs font-semibold text-slate-100 shrink-0">
              <span
                class="h-2 w-2 rounded-full"
                :class="
                  geoState === 'active'
                    ? 'bg-emerald-400 text-emerald-400 pulse-ring'
                    : geoState === 'idle_ready'
                      ? 'bg-emerald-400'
                      : geoState === 'disabled'
                        ? 'bg-slate-500'
                        : 'bg-rose-500'
                "
              ></span>
              DHT 入站态势
            </span>

            <ObsSelect
              v-model="effectiveHubCode"
              :options="hubOptions"
              class="w-28"
              trigger-class="px-2! py-0.5! text-[11px]!"
            />

            <RouterLink
              v-if="geoState === 'active' && (snap?.geoip.sample_rate_divisor ?? 1) > 1"
              to="/settings/services?tab=geoip"
              :title="`GeoLite2-Country.mmdb · 自适应采样 1:${snap?.geoip.sample_rate_divisor ?? 1} · 点击配置`"
              class="inline-flex items-center px-2 py-0.5 rounded-md bg-amber-500/15 text-amber-300 border border-amber-500/35 font-mono-num text-[10px] cursor-pointer hover:bg-amber-500/25 transition shrink-0"
            >
              MMDB · 1:{{ snap?.geoip.sample_rate_divisor }}
            </RouterLink>
            <RouterLink
              v-else-if="geoState === 'active'"
              to="/settings/services?tab=geoip"
              :title="`GeoLite2-Country.mmdb · mmap ${fmtBytes(snap?.geoip.mmdb_size_bytes ?? 0)} · 全量统计 · 点击配置`"
              class="inline-flex items-center px-2 py-0.5 rounded-md bg-emerald-500/15 text-emerald-300 border border-emerald-500/30 font-mono-num text-[10px] cursor-pointer hover:bg-emerald-500/25 transition shrink-0"
            >
              MMDB · 全量
            </RouterLink>
            <RouterLink
              v-else-if="geoState === 'idle_ready'"
              to="/settings/services?tab=geoip"
              title="GeoLite2-Country.mmdb 已就绪 · 启动 Crawler 进程后自动加载"
              class="inline-flex items-center px-2 py-0.5 rounded-md bg-emerald-500/15 text-emerald-300 border border-emerald-500/30 font-mono-num text-[10px] cursor-pointer hover:bg-emerald-500/25 transition shrink-0"
            >
              MMDB · 已就绪
            </RouterLink>
            <RouterLink
              v-else-if="geoState === 'disabled'"
              to="/settings/services?tab=geoip"
              title="GeoIP 国家归因已在设置中停用 · 点击开启"
              class="inline-flex items-center px-2 py-0.5 rounded-md bg-slate-800 text-slate-300 border border-slate-700 font-mono-num text-[10px] cursor-pointer hover:bg-slate-700 transition shrink-0"
            >
              GeoIP 已停用
            </RouterLink>
            <RouterLink
              v-else
              to="/settings/services?tab=geoip"
              title="GeoLite2-Country.mmdb 未加载 · 点击配置或一键下载"
              class="inline-flex items-center px-2 py-0.5 rounded-md bg-rose-500/15 text-rose-300 border border-rose-500/35 font-mono-num text-[10px] cursor-pointer hover:bg-rose-500/25 transition shrink-0"
            >
              MMDB 未加载
            </RouterLink>
          </div>

          <div class="globe-hud-glass rounded-xl p-1 flex items-center gap-1 pointer-events-auto shrink-0">
            <button
              type="button"
              class="px-2 py-1 rounded-lg text-[11px] border cursor-pointer transition"
              :class="
                autoRotate
                  ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/35'
                  : 'text-slate-400 border-transparent hover:text-slate-200'
              "
              @click="autoRotate = !autoRotate"
            >
              自动自转
            </button>
            <span class="h-3.5 w-px bg-slate-800 mx-0.5"></span>
            <button
              type="button"
              class="obs-btn-ghost px-2 py-1 rounded-lg text-xs font-mono-num cursor-pointer"
              title="放大"
              @click="adjustZoom(0.16)"
            >
              +
            </button>
            <button
              type="button"
              class="obs-btn-ghost px-2 py-1 rounded-lg text-xs font-mono-num cursor-pointer"
              title="缩小"
              @click="adjustZoom(-0.16)"
            >
              -
            </button>
            <button
              type="button"
              class="obs-btn-ghost px-2 py-1 rounded-lg text-[11px] cursor-pointer"
              @click="resetCamera"
            >
              复位
            </button>
          </div>
        </div>

        <!-- Centered Multi-State GeoIP Glass Notice -->
        <div
          v-if="geoState === 'idle_ready'"
          class="absolute inset-0 flex items-center justify-center p-6 z-15 pointer-events-none"
        >
          <div class="globe-hud-glass rounded-2xl p-5 max-w-sm text-center space-y-3 pointer-events-auto border-emerald-500/30">
            <div class="inline-flex items-center gap-2 px-2.5 py-1 rounded-full bg-emerald-500/15 border border-emerald-500/30 text-emerald-300 font-mono-num text-[11px]">
              GeoLite2-Country.mmdb 已就绪 · {{ fmtBytes(snap?.geoip.mmdb_size_bytes ?? 0) }}
            </div>
            <h4 class="text-sm font-semibold text-slate-100">等待启动 Crawler 采集进程</h4>
            <p class="text-xs text-slate-400 leading-relaxed">
              离线 IP 数据库已校验有效，当前暂无运行中的 Crawler 进程；启动采集进程后将自动加载并实时绘制全球 DHT 入站流量弧线。
            </p>
            <div class="flex flex-wrap justify-center gap-2">
              <RouterLink
                to="/workers"
                class="obs-btn-primary px-3.5 py-1.5 rounded-lg text-xs inline-block cursor-pointer"
              >
                前往启动 Crawler 进程 →
              </RouterLink>
              <RouterLink
                to="/settings/services?tab=geoip"
                class="obs-btn-ghost px-3 py-1.5 rounded-lg text-xs inline-block cursor-pointer"
              >
                GeoIP 配置
              </RouterLink>
            </div>
          </div>
        </div>

        <div
          v-else-if="geoState === 'disabled'"
          class="absolute inset-0 flex items-center justify-center p-6 z-15 pointer-events-none"
        >
          <div class="globe-hud-glass rounded-2xl p-5 max-w-sm text-center space-y-3 pointer-events-auto border-amber-500/35">
            <div class="inline-flex items-center gap-2 px-2.5 py-1 rounded-full bg-amber-500/15 border border-amber-500/30 text-amber-300 font-mono-num text-[11px]">
              GeoIP 国家归因已停用
            </div>
            <h4 class="text-sm font-semibold text-slate-100">国家来源归因未开启</h4>
            <p class="text-xs text-slate-400 leading-relaxed">
              全球 UDP pkt/s 与带宽统计继续运行，不影响 DHT 采集主链路；开启 GeoIP 归因并重载后即可恢复全球国家热点与实时弧线。
            </p>
            <div class="flex justify-center gap-2">
              <RouterLink
                to="/settings/services?tab=geoip"
                class="obs-btn-primary px-3.5 py-1.5 rounded-lg text-xs inline-block cursor-pointer"
              >
                前往开启 GeoIP →
              </RouterLink>
            </div>
          </div>
        </div>

        <div
          v-else-if="geoState === 'missing'"
          class="absolute inset-0 flex items-center justify-center p-6 z-15 pointer-events-none"
        >
          <div class="globe-hud-glass rounded-2xl p-5 max-w-sm text-center space-y-3 pointer-events-auto border-rose-500/35">
            <div class="inline-flex items-center gap-2 px-2.5 py-1 rounded-full bg-rose-500/15 border border-rose-500/30 text-rose-300 font-mono-num text-[11px]">
              GeoLite2-Country.mmdb 未加载
            </div>
            <h4 class="text-sm font-semibold text-slate-100">国家来源归因已降级停用</h4>
            <p class="text-xs text-slate-400 leading-relaxed">
              {{ snap?.geoip.error || "全局 UDP pkt/s 与带宽统计继续运行，不影响 DHT 采集主链路；配置并加载 GeoLite2-Country.mmdb 后即可恢复全球国家热点与实时弧线。" }}
            </p>
            <div class="flex justify-center gap-2">
              <RouterLink
                to="/settings/services?tab=geoip"
                class="obs-btn-primary px-3.5 py-1.5 rounded-lg text-xs inline-block cursor-pointer"
              >
                前往配置 / 一键下载 GeoIP 离线库 →
              </RouterLink>
            </div>
          </div>
        </div>

        <!-- Waiting for packets notice when MMDB is loaded -->
        <div
          v-else-if="geoState === 'active' && (snap?.countries.length ?? 0) === 0"
          class="pointer-events-none absolute top-16 left-1/2 -translate-x-1/2 z-10 px-3.5 py-1.5 rounded-full globe-hud-glass text-[11px] font-mono-num text-slate-300"
        >
          MMDB 已就绪，正在等待爬虫每秒上报国家级 DHT UDP 采样…
        </div>

        <!-- Unified Bottom Interactive Filter Legend + Minimal Camera Pill -->
        <div class="absolute bottom-3 left-3 right-3 flex flex-wrap items-center justify-between gap-2 pointer-events-none z-10">
          <div class="globe-hud-glass rounded-xl p-1 flex flex-wrap items-center gap-1 pointer-events-auto">
            <button
              v-for="pill in LAYER_FILTER_PILLS"
              :key="pill.key"
              type="button"
              class="inline-flex items-center gap-1.5 px-2 py-1 rounded-lg border text-[11px] cursor-pointer transition"
              :class="
                activeLayer === pill.key
                  ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/35 font-semibold'
                  : 'border-transparent text-slate-400 hover:text-slate-200 hover:bg-slate-800/40'
              "
              @click="activeLayer = pill.key"
            >
              <span v-if="pill.dotCls" class="h-1.5 w-1.5 rounded-full" :class="pill.dotCls"></span>
              {{ pill.label }}
            </button>
          </div>
          <div class="globe-hud-glass rounded-lg px-2.5 py-1 font-mono-num text-[10px] text-slate-400 pointer-events-none hidden sm:block">
            {{ Math.abs(Math.round(camLat)) }}°{{ camLat >= 0 ? "N" : "S" }} · {{ Math.abs(Math.round(camLon)) }}°{{ camLon >= 0 ? "E" : "W" }} · {{ camZoom.toFixed(2) }}x
          </div>
        </div>

        <!-- Hover Tooltip -->
        <div
          v-if="hoveredCountry"
          class="pointer-events-none absolute z-20 globe-hud-glass rounded-xl p-2.5 w-56 shadow-2xl text-[11px] font-mono-num"
          :style="{
            left: `${Math.min(Math.max(12, hoveredCountry.x + 14), 560)}px`,
            top: `${Math.max(12, hoveredCountry.y - 24)}px`,
          }"
        >
          <div class="font-semibold text-slate-100">
            {{ hoveredCountry.isoCode }} · {{ hoveredCountry.name }}
          </div>
          <div class="mt-1 text-cyan-300">
            {{ fmtRate(hoveredCountry.pps) }} pkt/s · {{ fmtBps(hoveredCountry.bps) }}
          </div>
          <div class="text-slate-400 mt-0.5">占比 {{ hoveredCountry.sharePct.toFixed(1) }}% · 点击聚焦视角</div>
        </div>
      </div>

      <!-- Right Sidebar: Country Ranking + 1s Aggregated DHT Pulse + Packet Mix -->
      <aside class="xl:col-span-4 space-y-4 min-w-0 flex flex-col">
        <!-- Top 8 Source Countries -->
        <div class="obs-card rounded-2xl p-4 space-y-3 flex-1 min-w-0">
          <div class="flex items-center justify-between gap-2">
            <div>
              <h3 class="text-sm font-semibold text-slate-100">DHT 入站来源国家 Top 8</h3>
              <p class="text-[11px] text-slate-500">基于 GeoLite2-Country 识别 · 点击聚焦国家版图</p>
            </div>
            <button
              v-if="focusedCountry"
              type="button"
              class="text-[11px] text-cyan-400 hover:underline cursor-pointer shrink-0"
              @click="clearFocus"
            >
              取消聚焦
            </button>
            <span v-else class="font-mono-num text-[10px] text-slate-500 shrink-0">按 pkt/s 排序</span>
          </div>

          <div v-if="topCountries.length > 0" class="space-y-2 max-h-[246px] overflow-y-auto obs-scroll pr-0.5">
            <button
              v-for="(c, idx) in topCountries"
              :key="c.iso_code"
              type="button"
              class="w-full text-left obs-inset p-2.5 rounded-lg transition group cursor-pointer"
              :class="focusedCountry === c.iso_code ? 'border-cyan-500/45 bg-cyan-950/20' : 'hover:border-emerald-500/35'"
              @click="focusCountry(c.iso_code)"
            >
              <div class="flex items-center justify-between text-xs gap-2">
                <div class="flex items-center gap-2 min-w-0">
                  <span class="text-[10px] font-mono-num text-slate-500 w-4">{{ idx + 1 }}.</span>
                  <span class="font-mono-num font-semibold text-slate-200">{{ c.iso_code }}</span>
                  <span class="text-slate-300 truncate group-hover:text-emerald-300 transition">
                    {{ resolveCountryGeo(c.iso_code)?.name || c.iso_code }}
                  </span>
                  <span
                    v-if="c.is_home"
                    class="px-1.5 py-0.2 rounded text-[9px] font-mono-num bg-emerald-500/15 text-emerald-300 border border-emerald-500/30 shrink-0"
                  >
                    HOME
                  </span>
                </div>
                <span class="font-mono-num text-cyan-300 font-semibold shrink-0">
                  {{ fmtRate(c.pps) }} pkt/s
                </span>
              </div>
              <div class="mt-1.5 flex items-center gap-2">
                <div class="flex-1 h-1.5 rounded-full bg-slate-900 overflow-hidden">
                  <div
                    class="h-full rounded-full bg-gradient-to-r from-cyan-500 to-emerald-400"
                    :style="{ width: `${Math.min(100, Math.max(2, c.share_pct))}%` }"
                  ></div>
                </div>
                <span class="text-[10px] font-mono-num text-slate-400 w-12 text-right">
                  {{ c.share_pct.toFixed(1) }}%
                </span>
              </div>
            </button>
          </div>
          <div v-else class="py-8 text-center text-xs text-slate-500 font-mono-num">
            暂无国家级流量排行数据
          </div>
        </div>

        <!-- Per-second National Ingest Pulses -->
        <div class="obs-card rounded-2xl p-4 space-y-2.5 min-w-0">
          <div class="flex items-center justify-between gap-2">
            <h3 class="text-sm font-semibold text-slate-100">DHT 秒级国家级入站脉冲</h3>
            <RouterLink to="/console" class="text-[11px] text-emerald-400 hover:underline cursor-pointer shrink-0">
              查看实时入库 Console →
            </RouterLink>
          </div>

          <div class="space-y-2 max-h-[210px] overflow-y-auto obs-scroll pr-0.5 font-mono-num text-[11px]">
            <div
              v-for="p in pulseLogs"
              :key="p.id"
              class="obs-inset px-2.5 py-2 rounded-lg flex items-center justify-between gap-2"
            >
              <div class="flex items-center gap-1.5 min-w-0">
                <span class="text-slate-500 text-[10px] shrink-0">{{ formatTime(p.ts) }}</span>
                <span class="text-slate-200 truncate">{{ p.name }}</span>
                <span
                  class="px-1 rounded text-[9px] border shrink-0"
                  :class="GLOBE_LAYER_STYLE[p.channel].badgeCls"
                >
                  {{ GLOBE_LAYER_STYLE[p.channel].label }}
                </span>
              </div>
              <span class="text-cyan-300 shrink-0">{{ fmtRate(p.pps) }} pkt/s</span>
            </div>
            <div v-if="pulseLogs.length === 0" class="py-8 text-center text-slate-500 obs-inset rounded-lg">
              等待爬虫 1s 脉冲采样…
            </div>
          </div>
        </div>

        <!-- Compact Stacked Packet Mix & Dual-Stack Summary -->
        <div class="obs-card rounded-xl p-3.5 space-y-2 text-[11px]">
          <div class="flex items-center justify-between text-slate-400">
            <span>DHT 入站报文分布与双栈结构</span>
            <span class="font-mono-num text-cyan-300">
              IPv4 {{ ipv4Share.toFixed(0) }}% · IPv6 {{ (100 - ipv4Share).toFixed(0) }}%
            </span>
          </div>
          <div class="h-2 w-full rounded-full bg-slate-800 overflow-hidden flex">
            <div
              class="h-full bg-cyan-400 transition-all duration-300"
              :style="{ width: `${packetMix.getPeers}%` }"
              :title="`get_peers ${packetMix.getPeers}%`"
            ></div>
            <div
              class="h-full bg-emerald-400 transition-all duration-300"
              :style="{ width: `${packetMix.announce}%` }"
              :title="`announce_peer ${packetMix.announce}%`"
            ></div>
            <div
              class="h-full bg-amber-400 transition-all duration-300"
              :style="{ width: `${packetMix.response}%` }"
              :title="`response ${packetMix.response}%`"
            ></div>
            <div
              class="h-full bg-indigo-400 transition-all duration-300"
              :style="{ width: `${packetMix.findPing}%` }"
              :title="`find_node / ping ${packetMix.findPing}%`"
            ></div>
          </div>
          <div class="flex flex-wrap items-center justify-between gap-x-2 font-mono-num text-[10px] text-slate-500">
            <span>get_peers {{ packetMix.getPeers }}%</span>
            <span>announce {{ packetMix.announce }}%</span>
            <span>response {{ packetMix.response }}%</span>
            <span>find/ping {{ packetMix.findPing }}%</span>
          </div>
        </div>
      </aside>
    </div>
  </div>
</template>
