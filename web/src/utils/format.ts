const COMPACT_UNITS = ["", "k", "M", "B", "T"] as const;
const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB", "PB", "EB"] as const;

function scale1000(absVal: number): { scaled: number; idx: number; decimals: number } {
  let idx = 0;
  let scaled = absVal;
  while (scaled >= 1000 && idx < COMPACT_UNITS.length - 1) {
    scaled /= 1000;
    idx += 1;
  }
  let decimals = idx === 0 ? 0 : scaled >= 100 ? 1 : 2;
  if (idx > 0 && Number(scaled.toFixed(decimals)) >= 1000 && idx < COMPACT_UNITS.length - 1) {
    scaled /= 1000;
    idx += 1;
    decimals = scaled >= 100 ? 1 : 2;
  }
  return { scaled, idx, decimals };
}

export function fmtExact(val: number | null | undefined, suffix?: string): string {
  if (val === null || val === undefined || !Number.isFinite(val)) return "-";
  const s = Math.round(val).toLocaleString("zh-CN");
  return suffix ? `${s} ${suffix}` : s;
}

export function fmtCount(val: number | null | undefined): string {
  if (val === null || val === undefined || !Number.isFinite(val)) return "-";
  const sign = val < 0 ? "-" : "";
  const abs = Math.abs(val);
  if (abs < 1000) {
    const r = Math.round(abs);
    if (r < 1000) return `${sign}${r}`;
  }
  const { scaled, idx, decimals } = scale1000(abs);
  return `${sign}${scaled.toFixed(decimals)}${COMPACT_UNITS[idx]}`;
}

export function fmtRate(val: number | null | undefined): string {
  if (val === null || val === undefined || !Number.isFinite(val) || val === 0) return "0";
  const sign = val < 0 ? "-" : "";
  const abs = Math.abs(val);
  if (abs < 1000) {
    if (abs >= 100) {
      const r = Math.round(abs);
      if (r < 1000) return `${sign}${r}`;
    } else {
      return `${sign}${abs.toFixed(abs >= 10 ? 1 : 2)}`;
    }
  }
  const { scaled, idx, decimals } = scale1000(abs);
  return `${sign}${scaled.toFixed(decimals)}${COMPACT_UNITS[idx]}`;
}

export function fmtBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes)) return "-";
  const sign = bytes < 0 ? "-" : "";
  const abs = Math.abs(bytes);
  if (abs < 1024) {
    const r = Math.round(abs);
    return r < 1024 ? `${sign}${r} B` : `${sign}1.00 KB`;
  }
  let idx = 0;
  let scaled = abs;
  while (scaled >= 1024 && idx < BYTE_UNITS.length - 1) {
    scaled /= 1024;
    idx += 1;
  }
  let decimals = scaled >= 100 ? 1 : 2;
  if (Number(scaled.toFixed(decimals)) >= 1024 && idx < BYTE_UNITS.length - 1) {
    scaled /= 1024;
    idx += 1;
    decimals = scaled >= 100 ? 1 : 2;
  }
  return `${sign}${scaled.toFixed(decimals)} ${BYTE_UNITS[idx]}`;
}

export function fmtBps(bps: number | null | undefined): string {
  if (!bps || !Number.isFinite(bps) || bps <= 0) return "0 B/s";
  return `${fmtBytes(bps)}/s`;
}

export function fmtDurSecs(secs: number | null | undefined): string {
  if (secs === null || secs === undefined || !Number.isFinite(secs)) return "-";
  const t = Math.max(0, Math.round(secs));
  const d = Math.floor(t / 86400);
  const h = Math.floor((t % 86400) / 3600);
  const m = Math.floor((t % 3600) / 60);
  const s = t % 60;
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

export function fmtDurMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "-";
  if (ms < 1000) {
    const r = Math.round(ms);
    return r < 1000 ? `${r}ms` : "1.00s";
  }
  if (ms < 60000) {
    const s = ms / 1000;
    const d = s >= 10 ? 1 : 2;
    return Number(s.toFixed(d)) >= 60 ? fmtDurSecs(60) : `${s.toFixed(d)}s`;
  }
  return fmtDurSecs(Math.round(ms / 1000));
}

export function fmtAxis(val: number): string {
  if (!Number.isFinite(val) || val === 0) return "0";
  const { scaled, idx } = scale1000(Math.abs(val));
  const d = scaled >= 100 ? 0 : 1;
  return `${val < 0 ? "-" : ""}${Number(scaled.toFixed(d))}${COMPACT_UNITS[idx]}`;
}

export function formatDateTime(unixSecs: number | null | undefined): string {
  if (!unixSecs) return "-";
  const d = new Date(unixSecs * 1000);
  const pad2 = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
}

export function formatTime(unixSecs: number | null | undefined): string {
  if (!unixSecs) return "-";
  return new Date(unixSecs * 1000).toLocaleTimeString("zh-CN", { hour12: false });
}

export function formatTimeMs(dateOrMs: Date | number): string {
  const d = dateOrMs instanceof Date ? dateOrMs : new Date(dateOrMs);
  const pad2 = (n: number) => String(n).padStart(2, "0");
  const pad3 = (n: number) => String(n).padStart(3, "0");
  return `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}.${pad3(d.getMilliseconds())}`;
}

export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}
