export interface CountryGeoInfo {
  isoNum: string;
  code: string;
  name: string;
  lat: number;
  lon: number;
}

export interface HomeHubInfo {
  id: string;
  isoNum: string;
  name: string;
  short: string;
  code: string;
  lat: number;
  lon: number;
}

const COUNTRY_LIST: CountryGeoInfo[] = [
  { isoNum: "156", code: "CN", name: "中国", lat: 35.86, lon: 104.19 },
  { isoNum: "276", code: "DE", name: "德国", lat: 51.16, lon: 10.45 },
  { isoNum: "528", code: "NL", name: "荷兰", lat: 52.13, lon: 5.29 },
  { isoNum: "702", code: "SG", name: "新加坡", lat: 1.35, lon: 103.82 },
  { isoNum: "840", code: "US", name: "美国", lat: 39.83, lon: -98.58 },
  { isoNum: "392", code: "JP", name: "日本", lat: 36.2, lon: 138.25 },
  { isoNum: "410", code: "KR", name: "韩国", lat: 35.91, lon: 127.77 },
  { isoNum: "826", code: "GB", name: "英国", lat: 54.0, lon: -2.0 },
  { isoNum: "250", code: "FR", name: "法国", lat: 46.23, lon: 2.21 },
  { isoNum: "643", code: "RU", name: "俄罗斯", lat: 61.52, lon: 105.32 },
  { isoNum: "752", code: "SE", name: "瑞典", lat: 60.13, lon: 18.64 },
  { isoNum: "124", code: "CA", name: "加拿大", lat: 56.13, lon: -106.35 },
  { isoNum: "036", code: "AU", name: "澳大利亚", lat: -25.27, lon: 133.78 },
  { isoNum: "076", code: "BR", name: "巴西", lat: -14.24, lon: -51.93 },
  { isoNum: "356", code: "IN", name: "印度", lat: 20.59, lon: 78.96 },
  { isoNum: "380", code: "IT", name: "意大利", lat: 41.87, lon: 12.57 },
  { isoNum: "724", code: "ES", name: "西班牙", lat: 40.46, lon: -3.75 },
  { isoNum: "616", code: "PL", name: "波兰", lat: 51.92, lon: 19.15 },
  { isoNum: "804", code: "UA", name: "乌克兰", lat: 48.38, lon: 31.17 },
  { isoNum: "704", code: "VN", name: "越南", lat: 14.06, lon: 108.28 },
  { isoNum: "764", code: "TH", name: "泰国", lat: 15.87, lon: 100.99 },
  { isoNum: "360", code: "ID", name: "印度尼西亚", lat: -0.79, lon: 113.92 },
  { isoNum: "792", code: "TR", name: "土耳其", lat: 38.96, lon: 35.24 },
  { isoNum: "484", code: "MX", name: "墨西哥", lat: 23.63, lon: -102.55 },
  { isoNum: "710", code: "ZA", name: "南非", lat: -30.56, lon: 22.94 },
  { isoNum: "032", code: "AR", name: "阿根廷", lat: -38.42, lon: -63.62 },
  { isoNum: "682", code: "SA", name: "沙特阿拉伯", lat: 23.89, lon: 45.08 },
  { isoNum: "756", code: "CH", name: "瑞士", lat: 46.82, lon: 8.23 },
  { isoNum: "578", code: "NO", name: "挪威", lat: 60.47, lon: 8.47 },
  { isoNum: "246", code: "FI", name: "芬兰", lat: 61.92, lon: 25.75 },
  { isoNum: "344", code: "HK", name: "香港", lat: 22.32, lon: 114.17 },
  { isoNum: "158", code: "TW", name: "台湾", lat: 23.7, lon: 120.96 },
  { isoNum: "040", code: "AT", name: "奥地利", lat: 47.52, lon: 14.55 },
  { isoNum: "056", code: "BE", name: "比利时", lat: 50.5, lon: 4.47 },
  { isoNum: "203", code: "CZ", name: "捷克", lat: 49.82, lon: 15.47 },
  { isoNum: "208", code: "DK", name: "丹麦", lat: 56.26, lon: 9.5 },
  { isoNum: "372", code: "IE", name: "爱尔兰", lat: 53.14, lon: -7.69 },
  { isoNum: "376", code: "IL", name: "以色列", lat: 31.05, lon: 34.85 },
  { isoNum: "458", code: "MY", name: "马来西亚", lat: 4.21, lon: 101.98 },
  { isoNum: "554", code: "NZ", name: "新西兰", lat: -40.9, lon: 174.89 },
  { isoNum: "608", code: "PH", name: "菲律宾", lat: 12.88, lon: 121.77 },
  { isoNum: "620", code: "PT", name: "葡萄牙", lat: 39.4, lon: -8.22 },
  { isoNum: "642", code: "RO", name: "罗马尼亚", lat: 45.94, lon: 24.97 },
  { isoNum: "152", code: "CL", name: "智利", lat: -35.68, lon: -71.54 },
  { isoNum: "170", code: "CO", name: "哥伦比亚", lat: 4.57, lon: -74.3 },
  { isoNum: "818", code: "EG", name: "埃及", lat: 26.82, lon: 30.8 },
  { isoNum: "300", code: "GR", name: "希腊", lat: 39.07, lon: 21.82 },
  { isoNum: "348", code: "HU", name: "匈牙利", lat: 47.16, lon: 19.5 },
  { isoNum: "398", code: "KZ", name: "哈萨克斯坦", lat: 48.02, lon: 66.92 },
  { isoNum: "586", code: "PK", name: "巴基斯坦", lat: 30.38, lon: 69.35 },
  { isoNum: "784", code: "AE", name: "阿联酋", lat: 23.42, lon: 53.85 },
  { isoNum: "100", code: "BG", name: "保加利亚", lat: 42.73, lon: 25.49 },
  { isoNum: "191", code: "HR", name: "克罗地亚", lat: 45.1, lon: 15.2 },
  { isoNum: "688", code: "RS", name: "塞尔维亚", lat: 44.02, lon: 21.01 },
  { isoNum: "703", code: "SK", name: "斯洛伐克", lat: 48.67, lon: 19.7 },
  { isoNum: "440", code: "LT", name: "立陶宛", lat: 55.17, lon: 23.88 },
  { isoNum: "428", code: "LV", name: "拉脱维亚", lat: 56.88, lon: 24.6 },
  { isoNum: "233", code: "EE", name: "爱沙尼亚", lat: 58.6, lon: 25.01 },
  { isoNum: "112", code: "BY", name: "白俄罗斯", lat: 53.71, lon: 27.95 },
  { isoNum: "498", code: "MD", name: "摩尔多瓦", lat: 47.41, lon: 28.37 },
  { isoNum: "504", code: "MA", name: "摩洛哥", lat: 31.79, lon: -7.09 },
  { isoNum: "566", code: "NG", name: "尼日利亚", lat: 9.08, lon: 8.68 },
  { isoNum: "404", code: "KE", name: "肯尼亚", lat: -0.02, lon: 37.91 },
  { isoNum: "604", code: "PE", name: "秘鲁", lat: -9.19, lon: -75.02 },
];

export const COUNTRY_GEO_BY_CODE: Record<string, CountryGeoInfo> = Object.fromEntries(
  COUNTRY_LIST.map((c) => [c.code, c]),
);

export const COUNTRY_META_BY_ISO_NUM: Record<string, { code: string; name: string }> = Object.fromEntries(
  COUNTRY_LIST.map((c) => [c.isoNum, { code: c.code, name: c.name }]),
);

export const GLOBE_HOME_HUBS: Record<string, HomeHubInfo> = {
  CN: { id: "CN", isoNum: "156", name: "中国 (CN)", short: "中国", code: "CN", lat: 31.23, lon: 121.47 },
  SG: { id: "SG", isoNum: "702", name: "新加坡 (SG)", short: "新加坡", code: "SG", lat: 1.35, lon: 103.82 },
  DE: { id: "DE", isoNum: "276", name: "德国 (DE)", short: "德国", code: "DE", lat: 50.11, lon: 8.68 },
  US: { id: "US", isoNum: "840", name: "美国 (US)", short: "美国", code: "US", lat: 37.39, lon: -122.08 },
  JP: { id: "JP", isoNum: "392", name: "日本 (JP)", short: "日本", code: "JP", lat: 35.68, lon: 139.69 },
  HK: { id: "HK", isoNum: "344", name: "香港 (HK)", short: "香港", code: "HK", lat: 22.32, lon: 114.17 },
  NL: { id: "NL", isoNum: "528", name: "荷兰 (NL)", short: "荷兰", code: "NL", lat: 52.37, lon: 4.9 },
  GB: { id: "GB", isoNum: "826", name: "英国 (GB)", short: "英国", code: "GB", lat: 51.51, lon: -0.13 },
};

export function resolveHomeHub(code?: string | null): HomeHubInfo {
  const norm = (code || "CN").trim().toUpperCase();
  if (GLOBE_HOME_HUBS[norm]) return GLOBE_HOME_HUBS[norm];
  const geo = COUNTRY_GEO_BY_CODE[norm];
  if (geo) {
    return {
      id: geo.code,
      isoNum: geo.isoNum,
      name: `${geo.name} (${geo.code})`,
      short: geo.name,
      code: geo.code,
      lat: geo.lat,
      lon: geo.lon,
    };
  }
  return GLOBE_HOME_HUBS.CN;
}

export function resolveCountryGeo(isoCode: string): CountryGeoInfo | null {
  const norm = (isoCode || "").trim().toUpperCase();
  return COUNTRY_GEO_BY_CODE[norm] ?? null;
}

export type GlobeLayerKey = "all" | "get_peers" | "announce_peer" | "response" | "find_node_ping";

export const GLOBE_LAYER_STYLE: Record<
  Exclude<GlobeLayerKey, "all">,
  { label: string; fullLabel: string; color: string; glow: string; badgeCls: string }
> = {
  get_peers: {
    label: "GET_PEERS",
    fullLabel: "get_peers 查寻",
    color: "#06b6d4",
    glow: "rgba(6, 182, 212, 0.9)",
    badgeCls: "bg-cyan-500/15 text-cyan-300 border-cyan-500/30",
  },
  announce_peer: {
    label: "ANNOUNCE",
    fullLabel: "announce_peer 宣告",
    color: "#10b981",
    glow: "rgba(16, 185, 129, 0.9)",
    badgeCls: "bg-emerald-500/15 text-emerald-300 border-emerald-500/30",
  },
  response: {
    label: "RESPONSE",
    fullLabel: "response 回包",
    color: "#f59e0b",
    glow: "rgba(245, 158, 11, 0.9)",
    badgeCls: "bg-amber-500/15 text-amber-300 border-amber-500/30",
  },
  find_node_ping: {
    label: "FIND/PING",
    fullLabel: "find_node / ping",
    color: "#818cf8",
    glow: "rgba(129, 140, 248, 0.9)",
    badgeCls: "bg-indigo-500/15 text-indigo-300 border-indigo-500/30",
  },
};
