# hameta 开发指南、编码规范与实施路线（Dev Guide & Roadmap）

- **关联需求文档**：[PRD.md](file:///g:/Development/Project/hameta/docs/PRD.md)（`v1.3`）
- **关联选型文档**：[TECH_STACK.md](file:///g:/Development/Project/hameta/docs/TECH_STACK.md)
- **关联架构文档**：[ARCHITECTURE.md](file:///g:/Development/Project/hameta/docs/ARCHITECTURE.md)

---

## 1. 本地开发环境要求与目录约定

### 1.1 工具链与运行依赖

| 依赖项 | 版本要求 | 说明 |
|---|---|---|
| **Rust 工具链** | `1.91+`（Edition 2024） | 包含 `rustfmt` 与 `clippy` 组件 |
| **Node.js & 包管理器** | `Node.js 24+` + `pnpm` | 仅用于前端开发与打包静态资源 |
| **Redis** | `>= 5.0`，**推荐 `>= 6.2`** | 用于 Streams 消费测试（`>= 6.2` 支持 `XAUTOCLAIM`） |
| **Manticore Search** | HTTP API `:9308` | 集成测试必须指向独立测试库或测试表，严禁在测试中修改线上 `torrents` |

### 1.2 本地源码目录布局约定
`hameta` 根目录下的 Workspace 通过相对路径依赖本地 DHT 内核仓库：

```text
Development/Project/
├── Rust磁力爬虫/
│   └── dht-crawler/       # 核心库 (Cargo.toml 中通过 ../Rust磁力爬虫/dht-crawler 引用)
└── hameta/                # 本项目根 Workspace
```

在根 `Cargo.toml` 的 `[workspace.dependencies]` 中统一声明：
```toml
dht-crawler = { path = "../Rust磁力爬虫/dht-crawler", features = ["metrics"] }
```

---

## 2. 本地开发、调试与构建工作流

### 2.1 前后端联动开发模式（推荐）
在本地开发阶段，前端由 Vite Dev Server 提供秒级 HMR 热更新，后端 `hameta-panel` 开启 `--dev` 模式，将 API、SSE 与静态资源联调解耦：

1. **启动后端面板进程**（默认监听 `127.0.0.1:8080`，数据写至 `./data`）：
   ```bash
   cargo run -p hameta-cli -- panel --dev
   ```
   > 首次克隆仓库尚未执行前端打包时，`web/dist` 目录可能为空。在 `hameta-panel` 中，开发模式或占位目录需预置一个最小 `web/dist/.gitkeep` 或通过 `#[cfg(not(debug_assertions))]` / 条件嵌入，避免未构建前端时 `rust-embed` 编译报错。

2. **启动前端 Vite 开发服务器**（在 `web/` 下配置 `server.proxy` 将 `/api`、`/healthz`、`/metrics` 转发至 `http://127.0.0.1:8080`）：
   ```bash
   pnpm --dir web install
   pnpm --dir web dev
   ```
   Vite 默认代理中需显式保留流式传输能力（禁用响应压缩缓冲），保证 `/api/v1/overview/stream` 与 `/api/v1/logs/stream` 等 SSE 端点在开发态实时吐出事件。

### 2.2 常用运行时配置与环境变量
所有参数均支持命令行 Flag 与环境变量覆盖，方便本地调试或容器注入：

| 变量 / Flag | 默认值 | 说明 |
|---|---|---|
| `HAMETA_DATA_DIR` / `--data-dir` | `./data` | 存放 `hameta.db`（SQLite）与 `logs/` 目录 |
| `HAMETA_BIND` / `--bind` | `0.0.0.0:8080` | Web 面板 HTTP 监听地址 |
| `RUST_LOG` | `info` | 临时覆盖控制台与文件日志级别（如 `RUST_LOG=hameta_crawler=debug,dht_crawler=info`） |

### 2.3 单独调试 Worker 子进程
正常运行时 `crawler` 与 `indexer` 由 `hameta panel` 通过 `stdin` 一次性注入 `WorkerBootstrap` JSON 拉起；如需脱离面板单独调试某个 Worker（例如配合 IDE 断点或临时联调 Redis/Manticore），可使用 `--standalone` 模式直接读取本地配置文件：
```bash
cargo run -p hameta-cli -- worker crawler --standalone --config ./scratch/crawler-config.json
```
`--standalone` 模式下 Worker 从本地配置文件读取参数并跳过向 Panel 注册，直接打印心跳摘要到控制台，按 `Ctrl-C` 执行 `sink drain` 优雅退出。

### 2.4 重置管理员账号与密码（CLI）
忘记后台登录密码或需要修改管理员用户名时，可直接使用二进制运维命令重置：
```bash
hameta admin reset --data-dir ./data --username admin --password new_password
```
该命令会在本地 `hameta.db`（WAL 模式）中更新管理员用户名与加盐 `SHA-256` 密码摘要，并清空 `admin_sessions` 令现有登录会话立即失效。

### 2.5 单二进制生产构建流水线
```bash
# 1. 构建 Vue 3 生产静态资源至 web/dist
pnpm --dir web build

# 2. 编译嵌入式单二进制发布包（可选启用 mimalloc）
cargo build --release -p hameta-cli --features mimalloc
```
构建产物位于 `target/release/hameta`（Windows 为 `target/release/hameta.exe`）。Linux 交叉编译时可配合 `cross` 或 `cargo-zigbuild` 目标 `x86_64-unknown-linux-gnu`。

### 2.6 Linux 生产部署与反代注意事项
1. **数据目录与权限**：`/var/lib/hameta` 目录权限设为 `0700`，仅属主（如 `hameta` 用户）可读写其中的 `hameta.db`。
2. **`systemd` 单元参考模板**（位于 `deploy/hameta.service`）：
   ```ini
   [Unit]
   Description=hameta DHT Crawler & Indexer Web Panel
   After=network-online.target
   Wants=network-online.target

   [Service]
   Type=simple
   User=hameta
   Group=hameta
   WorkingDirectory=/opt/hameta
   ExecStart=/opt/hameta/hameta panel --data-dir /var/lib/hameta --bind 127.0.0.1:8080
   Restart=on-failure
   RestartSec=5s
   LimitNOFILE=262144
   TimeoutStopSec=30s

   [Install]
   WantedBy=multi-user.target
   ```
   > 若采集端配置高并发 `max_worker_count`（例如 `P17` 参考的 `4440`），`LimitNOFILE=262144` 是必需前置条件；面板也会主动读取进程软限制并在界面提示风险。
3. **反向代理 SSE 配置**：使用 Nginx / Caddy 对外提供 HTTPS 访问时，必须在 `/api/v1/*/stream` 等 SSE 路径上关闭缓冲（例如 Nginx 添加 `proxy_buffering off;`、`proxy_cache off;`、`proxy_read_timeout 3600s;`），否则日志与监控实时流会被代理缓冲卡住。

---

## 3. 工程与编码规范

### 3.1 Rust 后端规范
1. **零 Panic 防御**：生产代码路径严禁调用 `.unwrap()` 或 `.expect()`（仅允许用于编译期静态正则表达式且附带明确 `unwrap_or_else` 上下文说明，或仅限单元测试）。
2. **日志卫生**：避免在常规 `info` / `warn` 业务日志中主动打印 Redis / Manticore 明文密码与完整连接口令。
3. **错误处理分层**：
   - `hameta-common`、`hameta-filter`、`hameta-storage` 等基础 Crate 使用 `thiserror` 定义清晰的领域错误枚举；
   - `hameta-panel`、`hameta-crawler`、`hameta-indexer` 二进制层及 HTTP Handler 可结合 `anyhow` 补充操作上下文，并由统一 `AppError` 转为标准 JSON 错误响应（绝不把内部 SQL 或堆栈原文裸露给未鉴权或前端提示）。
4. **配置反序列化严格性**：
   - 继承 `crawler-node` 的良好实践，DHT 采集配置结构体（尤其是 L3 `crawl.*`、`discovery.*`、`peer_lookup.*`）开启 `#[serde(deny_unknown_fields)]`，防止前端或导入配置拼写错误字段时被静默忽略。
5. **跨平台隔离代码局部化**：涉及 Linux `/proc`、`libc::getrlimit`/`PR_SET_PDEATHSIG` 与 Windows Job Object 的代码严格隔离在 `#[cfg(target_os = "...")]` 模块内，保证两端均能通过编译与基础测试。

### 3.2 Vue 3 前端规范
1. **TypeScript 严格模式**：开启 `strict: true`，所有后端 REST DTO、SSE 事件载荷与 Pinia Store 状态均需在 `web/src/types/` 下维护完整类型定义，禁止滥用 `any`。
2. **统一请求封装与登录态接管**：所有 API 调用统一走 `web/src/api/client.ts`，全局拦截 `401` 跳转 `/login`、拦截系统未初始化错误跳转 `/setup`。
3. **高危操作与表单校验对齐**：
   - 服务配置中心（Redis/Manticore 修改）、Stream 修剪、种子删除、停机重启等高危操作必须统一复用全局确认弹窗组件；其中**删除种子弹窗默认勾选“保留 Redis `metadata:indexed:<hash>` 标记”**。
   - 采集参数表单对覆写字段必须明确区分“继承档位默认值”与“自定义覆写”，L3 JSON 视图支持实时 Schema/JSON 语法校验提示。

### 3.3 提交与合并前强制检查清单
每次提交代码前，必须在根目录依次执行并通过以下全量检查：
```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm --dir web typecheck
pnpm --dir web build
```

---

## 4. 分层测试策略

### 4.1 单元测试（无外部网络/中间件依赖，默认执行）
| 测试模块 | 重点覆盖场景 |
|---|---|
| `hameta-common` | `hash_to_id` 63-bit 确定性与非法长度边界；加盐 SHA-256 密码校验；只读 SQL 词法/多语句/写关键词拦截与 `LIMIT` 注入 |
| `hameta-filter` | 6 大类过滤规则全边界（包括 `0` 值关闭语义、`custom_rules` 全 `0` 直接拦截、`torrent_rules` 实质文件/空种/压缩包占比）；`refine_name` 全量清洗回归测试集；动态 `ad-phrases`/`ad-regexes` 热切换与草稿 A/B 对比；非法正则报错定位 |
| `hameta-storage` | 内存 `:memory:` / 临时目录 SQLite 迁移、WAL 模式开启、服务配置版本读写、Worker 配置版本与回滚、分钟/小时指标降采样聚合、CLI 管理员重置及清空会话 |
| `hameta-indexer` | Manticore `/bulk` 回执校验状态机（分组 `bulk` / 逐条回执 / 截断响应 / `skipped_lines > 0` / 部分失败均不得通过）；`TorrentRecord` 清洗为空拦截不回退 `raw_name` |
| `hameta-panel` | 初始化向导状态锁（初始化完成后 `/api/v1/setup/init` 返回 `403`）；登录限流；Cookie 鉴权中间件；受影响 Worker 列表计算 |

### 4.2 集成测试（`#[ignore]` 隔离执行）
通过环境变量指定隔离的本地测试实例（严禁连接线上生产 Redis/Manticore）：
- **Redis Streams 测试**（`HAMETA_TEST_REDIS_URL=redis://127.0.0.1:6379` + 随机测试前缀）：验证 4 个 Lua 脚本竞态幂等、`XREADGROUP` + `XACK`、`XAUTOCLAIM` 模拟消费者崩溃接管、毒丸消息超限自动确认跳过、`metadata:indexed:<hash>` 写入与回查。
- **Manticore 兼容性测试**（`HAMETA_TEST_MANTICORE_URL` 指向测试实例）：验证 `DESCRIBE torrents`、批量 `replace`、热度累加 `update` 及删除联动选项。
- **Supervisor 多进程集成测试**：真实拉起子进程完成 `stdin` 注入与 Loopback IPC 注册、模拟子进程主动 `abort()` 崩溃验证指数退避重启与熔断状态、验证主进程退出时子进程全数优雅回收无残留。

---

## 5. 分阶段开发实施路线（Milestone & Work Breakdown）

按照 `PRD.md` 第 6 章三大阶段，拆解为 **10 个按序推进的可验收里程碑（M1–M10）**：

### Phase 1：核心骨架与可运行闭环（MVP）

| 里程碑 | 核心工作包 | 涉及模块 | 验收标准 |
|---|---|---|---|
| **M1：工程骨架与基础契约** | 初始化 7-crate Rust Workspace 与 `web/` Vue 3 工程；实现 `hameta-common`（配置结构、L1/L2/L3 与 `P1`–`P17` 映射、`hash_to_id`、加盐 SHA-256 哈希、只读 SQL 校验、IPC DTO）、`hameta-storage`（SQLite WAL + 10 张表迁移 + CRUD 仓库）及 `hameta admin reset` CLI 重置命令 | `hameta-common`<br>`hameta-storage`<br>`hameta-cli`<br>`web/` | `cargo test` 通过存储、SQL 校验、密码哈希与 CLI 账密重置单测；`web` 可正常 `pnpm build` 并通过 `rust-embed` 打入 `hameta-cli` |
| **M2：过滤清洗引擎迁入** | 完整构建 `hameta-filter`：迁入并升级 6 大类过滤规则、4.2 节默认线上预设、动态词库（19 条 `ad-phrases` + 1 条 `ad-regexes` + 1,284 条 `iana-tlds`）、规则归因 `FilterEvaluation`、`ArcSwap` 封装及 `200` 条拦截样本缓冲 | `hameta-filter` | 全量原过滤与清洗单元测试在 `hameta-filter` 100% 通过，且支持动态规则与归因返回 |
| **M3：进程守护与双 Worker 实现** | 实现 `hameta-panel` Supervisor（`stdin` 一次性注入 + Loopback IPC、Linux/Windows 防孤儿、崩溃退避自愈）；实现 `hameta-crawler`（接入 `dht-crawler` + `moka` + Lua + `XADD` Streams）与 `hameta-indexer`（`XREADGROUP` + `XACK` + `XAUTOCLAIM` + Manticore `/bulk` + 热度累加 + `metadata:indexed:*` 回写） | `hameta-panel`<br>`hameta-crawler`<br>`hameta-indexer` | 集成测试验证 `panel` 能拉起/停止两类 Worker，完整跑通 `DHT/Mock -> Redis Streams -> Indexer -> Manticore` 数据链路 |
| **M4：引导、认证与服务配置中心** | 实现 `/setup/*`、`/auth/*`、`/settings/*` 后端 API 与 Vue 3 页面：Setup Wizard 初始化（含加载默认线上过滤规则预设）、登录与路由守卫、**服务配置中心**（Redis & Manticore 连接配置全部字段完整回显/一键连通与 `DESCRIBE torrents` 校验/联动滚动重启） | `hameta-panel`<br>`web/` | 未初始化自动跳 `/setup`；配置全部字段完整回显并可测试连通；变更 Redis/Manticore 可触发受影响 Worker 平滑滚动重启 |
| **M5：MVP 核心面板页面** | 实现全局总览（全链路漏斗 + KPI + SQLite 降采样趋势图）、Worker 管理与 **L1/L2/L3 采集参数分层调优页**、**磁力检索与详情管理页**（取代 `torrent-browser`） | `hameta-panel`<br>`web/` | 浏览器端可完成查看总览趋势、在线调整 `P1`–`P17` 及 L1/L2/L3 参数并平滑重启 `crawler`、检索查看种子详情 |

### Phase 2：过滤清洗工作台与深度诊断

| 里程碑 | 核心工作包 | 涉及模块 | 验收标准 |
|---|---|---|---|
| **M6：过滤规则与词库热更新台** | 对接 `/api/v1/filter/*` 后端与可视化工作台：6 大类规则 + 动态广告词库独立启停编辑、正则预编译校验、点击发布后 IPC 广播 `indexer` 无停机 `ArcSwap` 切换、Top 拦截排行与 200 条拦截样本巡检 | `hameta-filter`<br>`hameta-indexer`<br>`hameta-panel`<br>`web/` | 修改规则点击发布后，运行中的 `indexer` 在不重启状态下实时切换版本号并按新规则拦截 |
| **M7：在线试跑沙盒（Playground）** | 实现单条调试（分步清洗结果对比 + 精准命中规则）、批量抽样回归（Manticore 抽样 / 拦截样本 / 多行粘贴）、草稿 vs 线上版本 A/B 对比预览 | `hameta-filter`<br>`hameta-panel`<br>`web/` | 发布前可对草稿规则执行抽样 A/B 对比并清晰呈现状态翻转样本 |
| **M8：深度诊断、Redis 监控与 SQL 台** | 完成 `crawler` DHT 深度诊断视图（Metadata 分阶段超时/错误归因/直方图/UDP 丢包与背压/来源收益）、Redis Streams 监控（长度/Lag/PEL/样例预览/受控修剪/InfoHash 诊断）与只读 SQL 控制台（CodeMirror + 导出） | `hameta-crawler`<br>`hameta-panel`<br>`web/` | 所有 `dht-crawler` 可观测指标在页面完整可视；SQL 控制台拦截写操作并支持 CSV/JSON 导出 |

### Phase 3：稳定性打磨与部署完善

| 里程碑 | 核心工作包 | 涉及模块 | 验收标准 |
|---|---|---|---|
| **M9：日志流、版本 Diff 与联动删除** | 完成分进程按天日志文件落盘、SSE 实时日志流查看与日志下载；完成采集/服务配置历史版本 Diff 对比与一键回滚；完善种子新增/编辑/批量删除（含默认保留 `metadata:indexed:*` 选项） | `hameta-common`<br>`hameta-panel`<br>`web/` | 页面实时流畅订阅任意 Worker 日志；配置误改可一键回滚；删除种子按选项正确同步 Redis |
| **M10：指标端点、压测与部署交付** | 补齐 `/healthz` 与聚合 `/metrics`（Prometheus）；执行长稳与崩溃恢复集成测试；整理 Linux `systemd` 模板、编译构建脚本与生产部署说明 | 全 Workspace<br>`deploy/` | 全量 `fmt`/`clippy`/`test` 与单二进制 release 打包通过，Linux 部署验证成功 |
