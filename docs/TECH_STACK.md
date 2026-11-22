# hameta 技术选型与架构决策记录（Tech Stack & ADRs）

- **关联需求文档**：[PRD.md](file:///g:/Development/Project/hameta/docs/PRD.md)（`v1.3`）
- **适用范围**：`hameta` 后端（Rust Workspace）与 Web 前端（Vue 3 SPA）

---

## 1. 选型目标与环境基线

围绕“单二进制多角色、单机多进程故障隔离、无停机规则热更新、兼容 Manticore 存量与 Redis 去重”的核心目标，工程环境基线设定如下：

| 维度 | 最低 / 推荐基线 | 说明 |
|---|---|---|
| **Rust 工具链** | `rustc 1.91+` / Rust `Edition 2024` | 与 [dht-crawler](file:///G:/Development/Project/Rust%E7%A3%81%E5%8A%9B%E7%88%AC%E8%99%AB/dht-crawler/Cargo.toml) 保持一致，避免跨 edition 语法与异步特性差异 |
| **前端构建环境** | `Node.js 24+` + `pnpm` | 仅在构建或前端热更新开发阶段需要；生产运行态前端已嵌入 Rust 二进制 |
| **DHT 内核依赖** | 本地 path 依赖 `../Rust磁力爬虫/dht-crawler` (`v0.2.1`) | 启用 `metrics` feature 对接统一可观测体系 |
| **Redis** | `>= 5.0`（强制，支持 Streams）；**推荐 `>= 6.2`**（支持 `XAUTOCLAIM`） | 保留线上 `metadata:indexed:<info_hash>` 去重键，新队列与锁使用 `hameta:*` |
| **Manticore Search** | HTTP JSON/SQL API（默认 `:9308`） | 严格零 DDL 改动接管存量 `torrents` 表 |
| **运行平台** | Windows（本地开发调试）/ Linux `x86_64`（生产部署推荐） | 支持跨平台进程管理与文件描述符限制检测 |

---

## 2. 后端技术选型矩阵（Rust Crate 清单）

所有后端模块围绕 Tokio 异步生态统一，尽量减少重型或强绑定系统动态库（如 OpenSSL）的依赖，以保证交叉编译与单二进制交付的简洁性。

| 领域 | 选型 Crate / 版本 | 核心用途 | 选型理由 |
|---|---|---|---|
| **异步运行时** | `tokio` (`1.x`) + `tokio-util` (`0.7`) + `async-channel` (`2.x`) | 多核异步调度、取消令牌 `CancellationToken`、多消费者有界通道 | 与 `dht-crawler` 完全同源；`async-channel` 在 MPMC 本地投递队列下实现无锁出队 |
| **CLI 多角色入口** | `clap` (`4.x`, `derive`) | 单二进制拆分子命令：`panel` / `worker crawler` / `worker indexer` / `admin reset` | 类型安全子命令解析，兼顾主面板、独立 Worker 及管理员账密重置维护命令 |
| **Web 与 SSE 服务** | `axum` (`0.8`) + `tower` + `tower-http` | REST API、鉴权中间件、实时日志/状态 SSE、内部 IPC HTTP Server | 零宏路由、与 `tokio`/`hyper` 深度集成；`tower-http` 提供压缩、超时、限流、请求体大小保护与 Trace |
| **静态资源嵌入** | `rust-embed` (`8.x`) + `mime_guess` (`2.x`) | 将 `web/dist` 编译期嵌入 `hameta` 可执行文件 | 实现单二进制开箱即用，生产环境零 Node/Nginx 强依赖；配合 SPA fallback 处理前端路由 |
| **本地配置与状态库** | `sqlx` (`0.8`, `runtime-tokio`, `sqlite`, `migrate`) | 管理员会话、服务连接配置、实例配置版本、过滤规则与词库、降采样历史指标、审计日志 | 内嵌 `libsqlite3` 静态链接（零外部安装）、内置编译期 SQL 迁移、开启 WAL 模式满足读多写少场景 |
| **认证与基础工具** | `sha2` (`0.10`)、`rand`、`hex` | 轻量加盐 SHA-256 管理员密码与会话 Token 摘要 | 编译快、零额外加密算法库负担；Redis/Manticore 配置直接明文存于本地 SQLite，免去主密钥管理 |
| **Redis 客户端** | `redis` (`0.26+`, `tokio-comp`) + `deadpool-redis` (`0.17+`) | 连接池管理、原子 Lua 去重脚本执行、Redis Streams (`XADD`/`XREADGROUP`/`XACK`/`XAUTOCLAIM`) | 沿用现有成熟组合，原生支持 Lua 脚本与任意 Streams 命令组装 |
| **Manticore HTTP** | `reqwest` (`0.12`, `rustls-tls`, `json`, `gzip`) | 调用 Manticore `/sql`、`/search`、`/bulk` 与 HTTP Basic Auth | 全纯 Rust TLS 栈（`rustls`），不依赖宿主机 OpenSSL，极大简化 Linux 交叉编译 |
| **序列化与校验** | `serde` + `serde_json` + `thiserror` + `anyhow` + `chrono` | 配置契约（`deny_unknown_fields`）、结构化错误分级、UTC 时间戳处理 | 领域库使用 `thiserror` 明确错误枚举，二进制/应用编排使用 `anyhow` 保留上下文 |
| **采集、缓存与热更新** | `dht-crawler` + `moka` (`0.12`) + `arc-swap` (`1.7`) + `regex` (`1.10`) + `unicode-normalization` (`0.1`) | DHT 爬虫驱动、进程内 LRU+TTL 去重、过滤规则与清洗词库无锁热切换、NFKC 归一化 | `moka` 承担高并发前置拦截减轻 Redis 压力；`arc-swap` 实现百万级并发读取零锁切换过滤规则 |
| **观测、日志与系统监控** | `tracing` + `tracing-subscriber` + `tracing-appender`；`metrics` + `metrics-exporter-prometheus`；`sysinfo` | 分角色按天滚动日志文件、广播 SSE 实时日志流、Prometheus `/metrics`、进程 CPU/RSS/Host 负载 | 与 `dht-crawler` 的 `metrics` facade 无缝对接；Linux 下补充读取 `/proc` 与 `libc::getrlimit` 获取精确 UDP 丢包与 FD 软硬限 |
| **内存与跨平台进程控制** | `mimalloc`（可选 feature）、Linux `libc`、Windows `windows-sys` | 降低高并发碎片；防孤儿进程保护（Linux `PR_SET_PDEATHSIG` / Windows Job Object） | 解决 `panel` 异常退出后子进程残留占用 UDP 端口的问题 |

---

## 3. 前端技术选型矩阵（Vue 3 SPA）

前端沿用并升级原 `torrent-browser` 的 Vue 3 + TypeScript 技术栈，兼顾后台运维密集表单效率、图表呈现力与最终单二进制体积控制。

| 领域 | 选型包 / 工具 | 核心用途 | 选型理由 |
|---|---|---|---|
| **核心框架** | `vue` (`^3.5`) + `typescript` (`^5.9`) + `vite` (`^6.x`) | SPA 组件化、严格静态类型检查、快速构建与本地 HMR 代理 | 与前代 `torrent-browser` 一致，组件模型清晰、构建速度快 |
| **状态与路由** | `pinia` (`^2.3`) + `vue-router` (`^4.6`) | 登录态与系统引导守卫、服务/实例配置状态、路由鉴权拦截 | 轻量规范，天然配合全局路由守卫实现“未初始化跳 `/setup`、未登录跳 `/login`” |
| **UI 组件库与样式** | `element-plus` + `tailwindcss` + `lucide-vue-next` | 复杂配置表单、分页表格、详情抽屉、Tabs、确认弹窗与现代图标 | 后台管理类高频场景（日期范围选择、树形文件、抽屉、表单验证）开箱即用，开发效率远高于手写基础组件 |
| **图表可视化** | `echarts` (`^5.x`)（按需引入） | 总览历史趋势折线图、全链路漏斗图、P50/P95 延迟与超时期分布柱状图 | 对接后端秒/分钟/小时降采样数据，支持缩放联动、多轴与暗色主题 |
| **代码 / 查询编辑器** | `codemirror` (`6.x`, JSON / SQL 语言包) | L3 专家模式 `crawl.*` JSON 配置编辑器、只读 SQL 控制台编辑器 | 相比动辄数十 MB 的 Monaco Editor，`CodeMirror 6` 模块化按需打包极轻量，能显著控制 `hameta` 最终二进制体积 |
| **网络与实时流** | 原生 `fetch` 封装 + `EventSource` | 统一 REST 请求（带 401 自动登出与错误提示）、实时日志流 & 监控快照 SSE | 零多余运行时依赖，原生支持 Cookie 会话鉴权 |
| **包管理与静态检查** | `pnpm` + `vue-tsc` + `eslint` | 确定性锁文件安装与严格 TS 编译检查 | 构建产出纯静态 `web/dist/` 目录供 `rust-embed` 打包 |

---

## 4. 核心架构决策记录（ADR）

### ADR-001：单二进制多角色 + 单机多进程 Supervisor 模式
- **背景**：用户要求抛弃旧 CLI 并采取单机多进程架构，避免 Worker 崩溃拖垮面板；同时底层 `dht-crawler::DHTServer::shutdown()` 停止后不可重用，修改端口或档位需彻底重建实例。
- **决策**：单一 `hameta` 二进制内置 `panel`、`worker crawler`、`worker indexer` 三个子命令；`panel` 作为 Supervisor 主进程统一托管本地子进程生命周期，通过系统调用（Linux `PR_SET_PDEATHSIG` / Windows Job Object）绑定父子生死。
- **权衡**：相比单进程多线程模型，多进程多了本地 IPC 与序列化开销（毫秒级心跳与批量推送，吞吐影响可忽略），但换来了 **panic/OOM/FD 耗尽的绝对故障隔离** 与 **配置变更平滑重建的零状态残留**，且单文件分发部署体验依然简单。

### ADR-002：Stdin 一次性引导注入 + Loopback IPC 控制面
- **背景**：如果 `panel` 通过命令行参数传递大段配置 JSON 或服务密码，不仅命令行长度受限且存在信息暴露；同时若控制指令完全依赖 Redis Pub/Sub，一旦 Redis 故障将导致面板无法查看日志或优雅关闭进程。
- **决策**：
  1. `panel` 拉起子进程时，通过子进程标准输入（`stdin`）一次性写入 JSON 格式的 `WorkerBootstrap`（包含实例配置、当前 Redis/Manticore 服务连接配置、当前规则包及一次性随机 IPC Token）并立即关闭 `stdin`（EOF）。
  2. 子进程启动后绑定 `127.0.0.1:0`（系统分配空闲端口），通过回调 `panel` 的本地注册端口完成双向建连，后续心跳、指标采集、规则热推送、优雅停机指令全部走带 Bearer Token 的 Loopback HTTP IPC。
  3. 单独本地调试时保留 `--standalone --config <file>` 模式直接从本地配置文件启动。
- **权衡**：无需维护跨进程配置文件同步，启动零临时文件且控制面不依赖 Redis 存活。

### ADR-003：仅兼容 `metadata:indexed:*` 去重键，全面启用 `hameta:*` Redis Streams
- **背景**：旧版 Redis 使用 `LPUSH/LTRIM + BRPOP` List 队列，消费者一旦在写入 Manticore 前崩溃就会永久丢失弹出批次内的种子；而生产环境已有 Redis 中存有大量 `metadata:indexed:<info_hash>` 去重键必须复用。
- **决策**：
  1. 严格沿用 `metadata:indexed:<info_hash>` 作为全局已索引去重标记（`crawler` 查询跳过、`indexer` 写库成功后置位）。
  2. 其余新队列与抓取锁全部隔离到 `hameta:*` 命名空间：使用 Redis Streams（`hameta:stream:torrent:new`、`hameta:stream:torrent:hot`）配合 Consumer Group (`XREADGROUP` + `XACK` + `XAUTOCLAIM`) 实现“至少一次交付 + 崩溃自动接管”，并用 `MAXLEN ~` 控制内存上限。
- **权衡**：`indexer` 在 Manticore 写入路径天然按 `info_hash` 确定性 `id` 幂等（`replace`），配合 `XAUTOCLAIM` 即使极少数重复投递也不会产生脏数据；缺点是 `XAUTOCLAIM` 要求 Redis `>= 6.2`（若检测到 Redis `5.0–6.0`，自动降级为 `XPENDING + XCLAIM` 兼容路径）。

### ADR-004：SQLite 单写者架构（仅 `panel` 读写本地数据库）
- **背景**：多个子进程如果同时直连打开同一个 SQLite 文件进行写入，极易在高并发刷新指标或配置保存时出现 `SQLITE_BUSY` 锁竞争。
- **决策**：规定运行期 **仅有 `panel` 主进程打开并读写 `hameta.db`**（开启 `WAL` 模式）。`crawler` 和 `indexer` 子进程不碰 SQLite，所有配置、规则与词库在启动时通过 `stdin` 一次性注入，运行期由 `panel` 通过 Loopback IPC 推送更新，指标与拦截样本由 `panel` 定时拉取后汇总写入 SQLite。唯一运维例外是人工执行 `hameta admin reset` 命令时，利用 WAL 与 `busy_timeout` 执行一次短事务更新 `admin_users` 并清空 `admin_sessions`。
- **权衡**：彻底消灭跨进程数据库锁冲突与迁移竞争，SQLite 模型极简可靠。

### ADR-005：Manticore 存量零 DDL 变更与重查询防护
- **背景**：线上 Manticore `torrents` 表存有海量数据，无条件执行 `SELECT COUNT(*)`、`SUM(file_count)` 或高频无索引排序极易拖垮入库吞吐。
- **决策**：
  1. 绝不执行 `ALTER TABLE` / `DROP TABLE`；连接已有库只做 `DESCRIBE torrents` 校验字段。
  2. 总览页总文档数优先读取 `SHOW TABLE torrents STATUS`（O(1) 元数据统计）并结合 30–60 秒服务端 TTL 缓存；“今日新增”与“拦截统计”由 `panel` 基于 `indexer` 上报与 SQLite 增量台账统计，绝不在每次刷新界面时对 `torrents` 扫全表。
  3. 检索列表强制上限 `pageSize <= 100`，只读 SQL 控制台强制单语句白名单校验、默认注入 `LIMIT 200` 及请求超时中断。

### ADR-006：`ArcSwap` 跨进程无停机热更新与草稿沙盒架构
- **背景**：运营需要频繁调整广告关键词、标题正则和清洗词库，重启 `indexer` 会导致批处理缓冲中断并触发大量 PEL 转移。
- **决策**：将 6 大类过滤规则与动态清洗词库（`ad-phrases` / `ad-regexes`）抽象为独立 Crate `hameta-filter`，同时为 `panel`（沙盒试跑）和 `indexer`（线上过滤清洗）复用；规则变更保存在 SQLite 的草稿/发布版本表中，发布前服务端预编译验证所有正则，发布后经 IPC 推送 JSON 载荷到 `indexer`，子进程在后台编译完成后通过 `ArcSwap::store` 原子替换生效，无需重启。
