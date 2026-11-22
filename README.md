# Hameta

基于 Rust + Vue 3 的高性能 BitTorrent DHT 采集、Redis Streams 解耦、Manticore Search 索引与可视化 Web 运维控制面板（单二进制一体化交付）。

## 核心特性

- **单二进制部署**：内置 Vue 3 + TypeScript + Element Plus 管理后台（通过 `rust-embed` 静态嵌入），开箱即用。
- **主控守护（Panel Supervisor）**：统一管理 `crawler` 与 `indexer` 子进程，支持 `stdin` 一次性引导注入、Loopback IPC 鉴权、防孤儿进程绑定及崩溃指数退避自愈。
- **采集调优（DHT Crawler）**：基于 [dht-crawler](https://github.com/0xddy/dht-crawler)，内置 `P1`–`P17` 档位预设与 L1/L2/L3 分层配置，支持 `RLIMIT_NOFILE` 自检与元数据竞速观测。
- **过滤清洗工作台 v2**：三段式流水线（结构门控 -> 8 阶段标题清洗 -> 有序 Allow/Reject 策略链），支持热更新 `ArcSwap`、旧版规则智能转译与在线 A/B 试跑沙盒。
- **全链路可观测与 SQL 运维**：全局实时漏斗与 SSE 趋势、Redis Streams Lag/PEL 与 InfoHash 链路诊断、Manticore 种子检索管理与只读 SQL 控制台。

## Linux 一键安装

以 `root` 身份在 Linux 服务器（支持 `amd64` / `arm64`，兼容 `systemd` 与 `SysVinit`）执行：

```bash
curl -fsSL https://raw.githubusercontent.com/0xddy/hameta/main/scripts/install.sh | sudo bash
```

可选参数：

```bash
# 指定面板监听端口
curl -fsSL https://raw.githubusercontent.com/0xddy/hameta/main/scripts/install.sh | sudo bash -s -- --bind 0.0.0.0:8080

# 从源码编译安装（无对应预编译包或需要自定义编译时）
curl -fsSL https://raw.githubusercontent.com/0xddy/hameta/main/scripts/install.sh | sudo bash -s -- --from-source

# 卸载程序（默认保留 /opt/hameta/data 数据与配置，追加 --purge 彻底清理）
sudo bash /opt/hameta/install.sh --uninstall
```

## 运维与交互式菜单

安装路径固定为 `/opt/hameta`，并通过 Linux `service` 命令维护后台服务：

```bash
# 打开交互式运维菜单（顶部实时显示主进程与 Worker 运行状态）
hameta
```

菜单功能：
1. **启动/停止程序**（调用 `service hameta start` / `service hameta stop`）
2. **重启程序**（调用 `service hameta restart`）
3. **重置管理员密码**（自动回填当前管理员，支持随机生成强密码或手动输入）
4. **查看管理信息**（面板访问地址、初始化状态、管理员信息、Redis/Manticore 端点与日志路径）
5. **更新版本**（自动检测并下载 GitHub Release 对应架构升级包，校对 SHA256 后停服覆盖升级并自动拉起，保留 `/opt/hameta/data` 与 `hameta.env`）

常用 `service` 命令与 CLI 透传：

```bash
service hameta start
service hameta stop
service hameta restart
service hameta status

# 传递参数给底层二进制（例如查看或重置管理员）
hameta admin info --data-dir /opt/hameta/data
hameta admin reset --data-dir /opt/hameta/data --username admin --password new_password
```

## `/opt/hameta` 目录布局

```text
/opt/hameta/
├── hameta            # 单二进制主程序（panel / worker / admin）
├── hameta-ctl.sh     # 交互式运维脚本（软链至 /usr/local/bin/hameta）
├── hameta.env        # 运行环境变量（HAMETA_BIND / HAMETA_DATA_DIR / HAMETA_LOG_FILE / RUST_LOG）
├── data/             # SQLite 状态库 hameta.db（权限 0700）
├── logs/             # 服务标准输出日志（SysVinit 模式下写至 hameta.log）
└── run/              # PID 文件目录（hameta.pid）
```

## 从源码构建

前置依赖：Rust `1.91+`（Edition 2024）、Node.js `24+`、`pnpm`。

```bash
# 1. 构建前端静态资源至 web/dist
pnpm --dir web install --frozen-lockfile
pnpm --dir web build

# 2. 编译嵌入式单二进制
cargo build --release -p hameta-cli --features mimalloc

# 3. 本地开发运行
./target/release/hameta panel --bind 0.0.0.0:8080 --data-dir ./data
```

## GitHub Actions 多平台发布

仓库内置 `.github/workflows/release.yml`，支持推送 `v*` 标签或手动运行工作流填写版本号编译全部 7 个目标并上传至 GitHub Release：

- `linux/amd64`（gnu + musl）、`linux/arm64`（gnu + musl）
- `darwin/arm64`（Apple Silicon）、`darwin/amd64`（Intel）
- `windows/amd64`

在 GitHub 仓库页面进入 **Actions -> Release -> Run workflow**，输入版本号（例如 `v0.2.0` 或 `0.2.0`）即可自动构建并生成对应 Release 与 `SHA256SUMS.txt`。

## 关联文档

- [产品需求文档 (PRD)](docs/PRD.md)
- [系统架构设计 (ARCHITECTURE)](docs/ARCHITECTURE.md)
- [技术栈说明 (TECH_STACK)](docs/TECH_STACK.md)
- [开发指南 (DEV_GUIDE)](docs/DEV_GUIDE.md)

> 文档提示：上方开发文档中如仍提及旧式本地路径，在公开仓库中已统一为 Cargo Git 依赖 `https://github.com/0xddy/dht-crawler.git`。
