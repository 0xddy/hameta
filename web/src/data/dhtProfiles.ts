export interface StaticDhtProfile {
  name: string;
  level: string;
  qps: number;
  burst: number;
  inFlight: number;
  peerLookup: number;
  peerBurst: number;
  peerActive: number;
  poolCap: number;
  respCap: number;
  snapSize: number;
  maxNewDest: number;
  respRate: number;
  respBytes: number;
  respSrc: number;
  replPerMin: number;
  subInFlight: number;
  recFd: number;
}

export const DHT_PROFILES: StaticDhtProfile[] = [
  { name: "P1",  level: "轻量入门", qps: 100,   burst: 200,   inFlight: 64,    peerLookup: 8,    peerBurst: 16,   peerActive: 32,   poolCap: 2048,   respCap: 512,   snapSize: 64,   maxNewDest: 600,    respRate: 400,    respBytes: 524288,    respSrc: 40,  replPerMin: 200,   subInFlight: 8,   recFd: 4096 },
  { name: "P2",  level: "入门增强", qps: 160,   burst: 320,   inFlight: 96,    peerLookup: 12,   peerBurst: 24,   peerActive: 48,   poolCap: 3072,   respCap: 768,   snapSize: 96,   maxNewDest: 900,    respRate: 640,    respBytes: 786432,    respSrc: 48,  replPerMin: 320,   subInFlight: 10,  recFd: 8192 },
  { name: "P3",  level: "低负载",   qps: 250,   burst: 500,   inFlight: 128,   peerLookup: 16,   peerBurst: 32,   peerActive: 64,   poolCap: 4096,   respCap: 1024,  snapSize: 128,  maxNewDest: 1200,   respRate: 1000,   respBytes: 1048576,   respSrc: 60,  replPerMin: 480,   subInFlight: 12,  recFd: 8192 },
  { name: "P4",  level: "温和探索", qps: 350,   burst: 700,   inFlight: 192,   peerLookup: 24,   peerBurst: 48,   peerActive: 96,   poolCap: 6144,   respCap: 1536,  snapSize: 160,  maxNewDest: 1800,   respRate: 1400,   respBytes: 1572864,   respSrc: 72,  replPerMin: 720,   subInFlight: 16,  recFd: 16384 },
  { name: "P5",  level: "轻量平衡", qps: 500,   burst: 1000,  inFlight: 256,   peerLookup: 32,   peerBurst: 64,   peerActive: 128,  poolCap: 8192,   respCap: 2048,  snapSize: 192,  maxNewDest: 2400,   respRate: 2000,   respBytes: 2097152,   respSrc: 80,  replPerMin: 960,   subInFlight: 20,  recFd: 16384 },
  { name: "P6",  level: "标准采集", qps: 750,   burst: 1500,  inFlight: 384,   peerLookup: 48,   peerBurst: 96,   peerActive: 192,  poolCap: 10240,  respCap: 2560,  snapSize: 224,  maxNewDest: 3600,   respRate: 3000,   respBytes: 3145728,   respSrc: 96,  replPerMin: 1440,  subInFlight: 24,  recFd: 16384 },
  { name: "P7",  level: "进阶标准", qps: 1000,  burst: 2000,  inFlight: 512,   peerLookup: 64,   peerBurst: 128,  peerActive: 256,  poolCap: 12288,  respCap: 3072,  snapSize: 256,  maxNewDest: 4800,   respRate: 4000,   respBytes: 4194304,   respSrc: 110, replPerMin: 1920,  subInFlight: 28,  recFd: 32768 },
  { name: "P8",  level: "中配主力", qps: 1400,  burst: 2800,  inFlight: 768,   peerLookup: 80,   peerBurst: 160,  peerActive: 320,  poolCap: 14336,  respCap: 3584,  snapSize: 320,  maxNewDest: 6400,   respRate: 5600,   respBytes: 6291456,   respSrc: 128, replPerMin: 2560,  subInFlight: 32,  recFd: 32768 },
  { name: "P9",  level: "均衡主力", qps: 2000,  burst: 4000,  inFlight: 1024,  peerLookup: 96,   peerBurst: 192,  peerActive: 384,  poolCap: 16384,  respCap: 4096,  snapSize: 384,  maxNewDest: 8000,   respRate: 8000,   respBytes: 8388608,   respSrc: 160, replPerMin: 3600,  subInFlight: 40,  recFd: 32768 },
  { name: "P10", level: "性能优选", qps: 2500,  burst: 5000,  inFlight: 1280,  peerLookup: 128,  peerBurst: 256,  peerActive: 512,  poolCap: 20480,  respCap: 5120,  snapSize: 448,  maxNewDest: 10000,  respRate: 10000,  respBytes: 10485760,  respSrc: 192, replPerMin: 4800,  subInFlight: 48,  recFd: 65536 },
  { name: "P11", level: "高并发",   qps: 3000,  burst: 6000,  inFlight: 1536,  peerLookup: 160,  peerBurst: 320,  peerActive: 640,  poolCap: 24576,  respCap: 6144,  snapSize: 512,  maxNewDest: 12000,  respRate: 12000,  respBytes: 12582912,  respSrc: 224, replPerMin: 6000,  subInFlight: 56,  recFd: 65536 },
  { name: "P12", level: "高速采集", qps: 4500,  burst: 9000,  inFlight: 2048,  peerLookup: 224,  peerBurst: 448,  peerActive: 896,  poolCap: 32768,  respCap: 8192,  snapSize: 640,  maxNewDest: 18000,  respRate: 18000,  respBytes: 16777216,  respSrc: 256, replPerMin: 9000,  subInFlight: 64,  recFd: 131072 },
  { name: "P13", level: "强力引擎", qps: 6500,  burst: 13000, inFlight: 3072,  peerLookup: 320,  peerBurst: 640,  peerActive: 1280, poolCap: 49152,  respCap: 12288, snapSize: 768,  maxNewDest: 26000,  respRate: 26000,  respBytes: 25165824,  respSrc: 320, replPerMin: 13000, subInFlight: 80,  recFd: 131072 },
  { name: "P14", level: "高频穿透", qps: 9000,  burst: 18000, inFlight: 4096,  peerLookup: 448,  peerBurst: 896,  peerActive: 1792, poolCap: 65536,  respCap: 16384, snapSize: 896,  maxNewDest: 36000,  respRate: 36000,  respBytes: 33554432,  respSrc: 384, replPerMin: 18000, subInFlight: 96,  recFd: 262144 },
  { name: "P15", level: "万级吞吐", qps: 12000, burst: 24000, inFlight: 6144,  peerLookup: 640,  peerBurst: 1280, peerActive: 2560, poolCap: 98304,  respCap: 24576, snapSize: 1024, maxNewDest: 48000,  respRate: 48000,  respBytes: 50331648,  respSrc: 512, replPerMin: 24000, subInFlight: 128, recFd: 262144 },
  { name: "P16", level: "超大规模", qps: 18000, burst: 36000, inFlight: 8192,  peerLookup: 896,  peerBurst: 1792, peerActive: 3584, poolCap: 131072, respCap: 32768, snapSize: 1280, maxNewDest: 72000,  respRate: 72000,  respBytes: 67108864,  respSrc: 640, replPerMin: 36000, subInFlight: 160, recFd: 524288 },
  { name: "P17", level: "极限吞吐", qps: 25000, burst: 50000, inFlight: 12288, peerLookup: 1280, peerBurst: 2560, peerActive: 5120, poolCap: 196608, respCap: 49152, snapSize: 1536, maxNewDest: 100000, respRate: 100000, respBytes: 100663296, respSrc: 800, replPerMin: 50000, subInFlight: 200, recFd: 524288 },
];

export const DHT_PROFILE_SELECT_OPTIONS = DHT_PROFILES.map((p) => ({
  value: p.name,
  label: `${p.name} · ${p.level}`,
}));

const DHT_PROFILE_LEVEL_MAP = new Map<string, string>(
  DHT_PROFILES.map((p) => [p.name, p.level]),
);

export function getDhtProfileLevelLabel(name: string): string {
  return DHT_PROFILE_LEVEL_MAP.get(name.toUpperCase()) ?? "";
}
