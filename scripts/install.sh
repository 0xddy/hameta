#!/usr/bin/env bash
set -euo pipefail

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"

REPO="${HAMETA_REPO:-0xddy/hameta}"
BRANCH="${HAMETA_BRANCH:-main}"
INSTALL_DIR="/opt/hameta"
BIN_PATH="${INSTALL_DIR}/hameta"
CTL_PATH="${INSTALL_DIR}/hameta-ctl.sh"
ENV_PATH="${INSTALL_DIR}/hameta.env"
SYMLINK_PATH="/usr/local/bin/hameta"
INIT_DEST="/etc/init.d/hameta"
SYSTEMD_DEST="/etc/systemd/system/hameta.service"

HAMETA_VERSION="${HAMETA_VERSION:-latest}"
HAMETA_BIND="${HAMETA_BIND:-0.0.0.0:8080}"
BIND_EXPLICIT=0
FROM_SOURCE=0
LOCAL_BINARY=""
DO_UNINSTALL=0
DO_PURGE=0

if [ -t 1 ]; then
    C_RESET=$'\033[0m'
    C_GREEN=$'\033[32m'
    C_YELLOW=$'\033[33m'
    C_RED=$'\033[31m'
    C_CYAN=$'\033[36m'
else
    C_RESET=""
    C_GREEN=""
    C_YELLOW=""
    C_RED=""
    C_CYAN=""
fi

log_info()  { echo "${C_CYAN}[INFO]${C_RESET} $*"; }
log_ok()    { echo "${C_GREEN}[OK]${C_RESET} $*"; }
log_warn()  { echo "${C_YELLOW}[WARN]${C_RESET} $*"; }
log_err()   { echo "${C_RED}[ERROR]${C_RESET} $*" >&2; }

usage() {
    cat <<'EOF'
Hameta Linux 一键安装与管理脚本

用法:
  install.sh [选项]

选项:
  --version <tag>     安装指定 Release 版本标签（默认: latest）
  --bind <addr:port>  设置 Web 面板监听地址（默认: 0.0.0.0:8080）
  --from-source       跳过预编译包，直接拉取源码在本地编译安装
  --binary <path>     使用本地已编译的 hameta 二进制离线安装
  --uninstall         停止服务并卸载程序（保留 /opt/hameta/data 与配置文件）
  --purge             配合 --uninstall 使用，彻底删除整个 /opt/hameta 目录
  -h, --help          显示此帮助信息
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || { log_err "--version 缺少参数"; exit 1; }
            HAMETA_VERSION="$2"
            shift 2
            ;;
        --bind)
            [ "$#" -ge 2 ] || { log_err "--bind 缺少参数"; exit 1; }
            HAMETA_BIND="$2"
            BIND_EXPLICIT=1
            shift 2
            ;;
        --from-source)
            FROM_SOURCE=1
            shift
            ;;
        --binary)
            [ "$#" -ge 2 ] || { log_err "--binary 缺少参数"; exit 1; }
            LOCAL_BINARY="$2"
            shift 2
            ;;
        --uninstall)
            DO_UNINSTALL=1
            shift
            ;;
        --purge)
            DO_PURGE=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            log_err "未知选项: $1"
            usage
            exit 1
            ;;
    esac
done

require_root() {
    if [ "$(id -u)" -ne 0 ]; then
        log_err "请使用 root 身份或通过 sudo 运行此安装脚本。"
        exit 1
    fi
}

check_os() {
    local os
    os=$(uname -s)
    if [ "$os" != "Linux" ]; then
        log_err "当前脚本仅支持 Linux 系统，检测到: ${os}"
        exit 1
    fi
}

detect_arch() {
    local machine
    machine=$(uname -m)
    case "$machine" in
        x86_64|amd64) echo "amd64" ;;
        aarch64|arm64) echo "arm64" ;;
        *)
            log_err "暂不支持的 CPU 架构: ${machine}（仅支持 x86_64/amd64 与 aarch64/arm64）"
            exit 1
            ;;
    esac
}

is_systemd() {
    [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1
}

download_file() {
    local url="$1"
    local dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 2 --connect-timeout 15 -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q --timeout=15 -O "$dest" "$url"
    else
        log_err "缺少 curl 或 wget，无法下载文件"
        return 1
    fi
}

ensure_base_packages() {
    local missing=()
    local cmd
    for cmd in curl tar gzip; do
        command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
    done
    [ "${#missing[@]}" -eq 0 ] && return 0

    log_info "正在安装基础工具: ${missing[*]}"
    if command -v apt-get >/dev/null 2>&1; then
        DEBIAN_FRONTEND=noninteractive apt-get update -qq
        DEBIAN_FRONTEND=noninteractive apt-get install -y -qq ca-certificates "${missing[@]}"
    elif command -v dnf >/dev/null 2>&1; then
        dnf install -y -q ca-certificates "${missing[@]}"
    elif command -v yum >/dev/null 2>&1; then
        yum install -y -q ca-certificates "${missing[@]}"
    elif command -v apk >/dev/null 2>&1; then
        apk add --no-cache ca-certificates "${missing[@]}"
    else
        log_err "未找到支持的包管理器，请先手动安装: ${missing[*]}"
        exit 1
    fi
}

stop_existing_service() {
    if is_systemd && systemctl is-active --quiet hameta 2>/dev/null; then
        log_info "检测到正在运行的 hameta 服务，正在停止..."
        if command -v service >/dev/null 2>&1; then
            service hameta stop || systemctl stop hameta || true
        else
            systemctl stop hameta || true
        fi
        return 0
    fi

    if [ -x "$INIT_DEST" ]; then
        if command -v service >/dev/null 2>&1; then
            service hameta stop >/dev/null 2>&1 || "$INIT_DEST" stop >/dev/null 2>&1 || true
        else
            "$INIT_DEST" stop >/dev/null 2>&1 || true
        fi
    fi
}

uninstall_hameta() {
    require_root
    log_info "开始卸载 Hameta..."
    stop_existing_service

    if is_systemd && [ -f "$SYSTEMD_DEST" ]; then
        systemctl disable hameta >/dev/null 2>&1 || true
        rm -f "$SYSTEMD_DEST"
        systemctl daemon-reload >/dev/null 2>&1 || true
    fi

    if [ -f "$INIT_DEST" ]; then
        if command -v update-rc.d >/dev/null 2>&1; then
            update-rc.d -f hameta remove >/dev/null 2>&1 || true
        elif command -v chkconfig >/dev/null 2>&1; then
            chkconfig --del hameta >/dev/null 2>&1 || true
        fi
        rm -f "$INIT_DEST"
    fi

    rm -f "$SYMLINK_PATH"
    rm -f "$BIN_PATH" "$CTL_PATH" "${INSTALL_DIR}/install.sh"
    rm -rf "${INSTALL_DIR}/run"

    if [ "$DO_PURGE" -eq 1 ]; then
        rm -rf "$INSTALL_DIR"
        log_ok "已彻底删除 Hameta 及所有数据目录 (${INSTALL_DIR})"
    else
        log_ok "已卸载 Hameta 服务与主程序。数据与配置保留在: ${INSTALL_DIR}"
        log_info "如需连同数据彻底清除，请追加 --purge 参数重新执行。"
    fi
}

if [ "$DO_UNINSTALL" -eq 1 ]; then
    uninstall_hameta
    exit 0
fi

TMP_DIR=$(mktemp -d /tmp/hameta-install.XXXXXX)
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

verify_sha256_if_present() {
    local archive_path="$1"
    local archive_name="$2"
    local sums_file="$3"
    [ -f "$sums_file" ] || return 0

    local expected
    expected=$(awk -v name="$archive_name" '$2 == name || $2 == ("*" name) {print $1; exit}' "$sums_file")
    [ -n "$expected" ] || return 0

    local actual=""
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$archive_path" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$archive_path" | awk '{print $1}')
    else
        log_warn "未找到 sha256sum，跳过压缩包摘要校验"
        return 0
    fi

    if [ "$actual" != "$expected" ]; then
        log_err "SHA256 校验失败: ${archive_name} (期望 ${expected}, 实际 ${actual})"
        return 1
    fi
    log_ok "SHA256 校验通过: ${archive_name}"
    return 0
}

fetch_deploy_assets_from_repo() {
    local dest_deploy="$1"
    mkdir -p "$dest_deploy"
    local ref="$BRANCH"
    if [ "$HAMETA_VERSION" != "latest" ]; then
        ref="$HAMETA_VERSION"
    fi
    local raw_base="https://raw.githubusercontent.com/${REPO}/${ref}"
    download_file "${raw_base}/deploy/hameta-ctl.sh" "${dest_deploy}/hameta-ctl.sh"
    download_file "${raw_base}/deploy/hameta.init" "${dest_deploy}/hameta.init"
    download_file "${raw_base}/deploy/hameta.service" "${dest_deploy}/hameta.service"
    download_file "${raw_base}/scripts/install.sh" "${dest_deploy}/install.sh" || true
}

stage_from_local_binary() {
    local bin_src="$1"
    local stage="$2"
    [ -f "$bin_src" ] || { log_err "指定的二进制不存在: $bin_src"; exit 1; }
    mkdir -p "${stage}/deploy"
    cp -f "$bin_src" "${stage}/hameta"
    chmod 0755 "${stage}/hameta"

    local script_dir=""
    if [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then
        script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
    fi

    if [ -n "$script_dir" ] && [ -d "${script_dir}/../deploy" ]; then
        cp -f "${script_dir}/../deploy/hameta-ctl.sh" "${stage}/deploy/hameta-ctl.sh"
        cp -f "${script_dir}/../deploy/hameta.init" "${stage}/deploy/hameta.init"
        cp -f "${script_dir}/../deploy/hameta.service" "${stage}/deploy/hameta.service"
        cp -f "${script_dir}/install.sh" "${stage}/deploy/install.sh" 2>/dev/null || true
    else
        fetch_deploy_assets_from_repo "${stage}/deploy"
    fi
}

try_download_prebuilt() {
    local arch="$1"
    local stage="$2"
    local base_url
    if [ "$HAMETA_VERSION" = "latest" ]; then
        base_url="https://github.com/${REPO}/releases/latest/download"
    else
        base_url="https://github.com/${REPO}/releases/download/${HAMETA_VERSION}"
    fi

    local sums_file="${TMP_DIR}/SHA256SUMS.txt"
    download_file "${base_url}/SHA256SUMS.txt" "$sums_file" 2>/dev/null || rm -f "$sums_file"

    local variant archive_name archive_path extract_dir
    for variant in "hameta-linux-${arch}" "hameta-linux-${arch}-musl"; do
        archive_name="${variant}.tar.gz"
        archive_path="${TMP_DIR}/${archive_name}"
        extract_dir="${TMP_DIR}/${variant}"

        log_info "尝试下载预编译包: ${archive_name} ..."
        if ! download_file "${base_url}/${archive_name}" "$archive_path" 2>/dev/null; then
            log_warn "未找到预编译包 ${archive_name}"
            continue
        fi

        if ! verify_sha256_if_present "$archive_path" "$archive_name" "$sums_file"; then
            continue
        fi

        rm -rf "$extract_dir"
        mkdir -p "$extract_dir"
        if ! tar -xzf "$archive_path" -C "$extract_dir"; then
            log_warn "解压 ${archive_name} 失败"
            continue
        fi

        if [ ! -f "${extract_dir}/hameta" ]; then
            log_warn "${archive_name} 中缺少 hameta 可执行文件"
            continue
        fi

        chmod 0755 "${extract_dir}/hameta"
        if ! "${extract_dir}/hameta" --version >/dev/null 2>&1; then
            log_warn "${archive_name} 在当前系统 libc 环境下无法运行，尝试回退..."
            continue
        fi

        cp -f "${extract_dir}/hameta" "${stage}/hameta"
        mkdir -p "${stage}/deploy"
        if [ -f "${extract_dir}/deploy/hameta-ctl.sh" ] && [ -f "${extract_dir}/deploy/hameta.init" ] && [ -f "${extract_dir}/deploy/hameta.service" ]; then
            cp -f "${extract_dir}/deploy/hameta-ctl.sh" "${stage}/deploy/hameta-ctl.sh"
            cp -f "${extract_dir}/deploy/hameta.init" "${stage}/deploy/hameta.init"
            cp -f "${extract_dir}/deploy/hameta.service" "${stage}/deploy/hameta.service"
            cp -f "${extract_dir}/scripts/install.sh" "${stage}/deploy/install.sh" 2>/dev/null || true
        else
            fetch_deploy_assets_from_repo "${stage}/deploy"
        fi

        log_ok "已获取并验证预编译版本 (${variant})"
        return 0
    done

    return 1
}

ensure_source_build_toolchain() {
    local pkgs=()
    command -v git >/dev/null 2>&1 || pkgs+=(git)
    command -v gcc >/dev/null 2>&1 || pkgs+=(gcc)
    command -v make >/dev/null 2>&1 || pkgs+=(make)

    if [ "${#pkgs[@]}" -gt 0 ]; then
        log_info "正在安装源码编译基础工具: ${pkgs[*]}"
        if command -v apt-get >/dev/null 2>&1; then
            DEBIAN_FRONTEND=noninteractive apt-get update -qq
            DEBIAN_FRONTEND=noninteractive apt-get install -y -qq build-essential pkg-config git
        elif command -v dnf >/dev/null 2>&1; then
            dnf install -y -q gcc gcc-c++ make pkgconfig git
        elif command -v yum >/dev/null 2>&1; then
            yum install -y -q gcc gcc-c++ make pkgconfig git
        elif command -v apk >/dev/null 2>&1; then
            apk add --no-cache build-base pkgconfig git
        else
            log_err "请先手动安装: git gcc make"
            exit 1
        fi
    fi

    if ! command -v node >/dev/null 2>&1; then
        log_info "未检测到 Node.js，正在尝试安装..."
        if command -v apt-get >/dev/null 2>&1 && curl -fsSL https://deb.nodesource.com/setup_24.x | bash - >/dev/null 2>&1; then
            DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nodejs
        elif command -v dnf >/dev/null 2>&1 && curl -fsSL https://rpm.nodesource.com/setup_24.x | bash - >/dev/null 2>&1; then
            dnf install -y -q nodejs
        else
            log_err "自动安装 Node.js 24+ 失败，请手动安装 Node.js 24+ 与 pnpm 后重试"
            exit 1
        fi
    fi

    if ! command -v pnpm >/dev/null 2>&1; then
        log_info "正在安装 pnpm..."
        if command -v corepack >/dev/null 2>&1; then
            corepack enable >/dev/null 2>&1 || true
            corepack prepare pnpm@latest --activate >/dev/null 2>&1 || true
        fi
        if ! command -v pnpm >/dev/null 2>&1 && command -v npm >/dev/null 2>&1; then
            npm install -g pnpm
        fi
        command -v pnpm >/dev/null 2>&1 || { log_err "安装 pnpm 失败，请手动安装后重试"; exit 1; }
    fi

    if ! command -v cargo >/dev/null 2>&1; then
        log_info "未检测到 Rust 工具链，正在安装 rustup (stable)..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
        # shellcheck disable=SC1090
        [ -f "${HOME}/.cargo/env" ] && . "${HOME}/.cargo/env"
        command -v cargo >/dev/null 2>&1 || { log_err "Rust 安装后仍未找到 cargo"; exit 1; }
    fi
}

build_from_source() {
    local stage="$1"
    ensure_source_build_toolchain

    local src_dir="${TMP_DIR}/src"
    local clone_ref="$BRANCH"
    if [ "$HAMETA_VERSION" != "latest" ]; then
        clone_ref="$HAMETA_VERSION"
    fi

    log_info "正在拉取源码: https://github.com/${REPO}.git (${clone_ref})..."
    git clone --depth 1 --branch "$clone_ref" "https://github.com/${REPO}.git" "$src_dir"

    log_info "正在构建 Vue 3 前端静态资源..."
    pnpm --dir "${src_dir}/web" install --frozen-lockfile
    pnpm --dir "${src_dir}/web" build

    log_info "正在编译 Rust 单二进制 (release + mimalloc)..."
    (cd "$src_dir" && cargo build --release -p hameta-cli --features mimalloc)

    cp -f "${src_dir}/target/release/hameta" "${stage}/hameta"
    chmod 0755 "${stage}/hameta"
    mkdir -p "${stage}/deploy"
    cp -f "${src_dir}/deploy/hameta-ctl.sh" "${stage}/deploy/hameta-ctl.sh"
    cp -f "${src_dir}/deploy/hameta.init" "${stage}/deploy/hameta.init"
    cp -f "${src_dir}/deploy/hameta.service" "${stage}/deploy/hameta.service"
    cp -f "${src_dir}/scripts/install.sh" "${stage}/deploy/install.sh" 2>/dev/null || true
    log_ok "源码编译完成"
}

write_or_update_env() {
    if [ ! -f "$ENV_PATH" ]; then
        cat >"$ENV_PATH" <<EOF
# Hameta 运行时环境变量
HAMETA_BIND=${HAMETA_BIND}
HAMETA_DATA_DIR=${INSTALL_DIR}/data
HAMETA_LOG_FILE=${INSTALL_DIR}/logs/hameta.log
RUST_LOG=info
EOF
        chmod 0644 "$ENV_PATH"
        return 0
    fi

    if [ "$BIND_EXPLICIT" -eq 1 ]; then
        if grep -q '^HAMETA_BIND=' "$ENV_PATH"; then
            sed -i "s|^HAMETA_BIND=.*|HAMETA_BIND=${HAMETA_BIND}|" "$ENV_PATH"
        else
            echo "HAMETA_BIND=${HAMETA_BIND}" >>"$ENV_PATH"
        fi
    fi
}

install_service_files() {
    local stage="$1"

    stop_existing_service

    mkdir -p "$INSTALL_DIR" "${INSTALL_DIR}/data" "${INSTALL_DIR}/logs" "${INSTALL_DIR}/run"
    chmod 0755 "$INSTALL_DIR"
    chmod 0700 "${INSTALL_DIR}/data"

    install -m 0755 "${stage}/hameta" "$BIN_PATH"
    install -m 0755 "${stage}/deploy/hameta-ctl.sh" "$CTL_PATH"
    if [ -f "${stage}/deploy/install.sh" ]; then
        install -m 0755 "${stage}/deploy/install.sh" "${INSTALL_DIR}/install.sh"
    elif [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then
        install -m 0755 "${BASH_SOURCE[0]}" "${INSTALL_DIR}/install.sh" || true
    fi

    mkdir -p /usr/local/bin
    ln -sfn "$CTL_PATH" "$SYMLINK_PATH"
    write_or_update_env

    install -m 0755 "${stage}/deploy/hameta.init" "$INIT_DEST"

    if is_systemd; then
        install -m 0644 "${stage}/deploy/hameta.service" "$SYSTEMD_DEST"
        systemctl daemon-reload
        systemctl enable hameta >/dev/null 2>&1 || true
        log_ok "已安装 systemd 服务并设置开机自启 (兼容 service hameta 命令)"
    else
        if command -v update-rc.d >/dev/null 2>&1; then
            update-rc.d hameta defaults >/dev/null 2>&1 || true
        elif command -v chkconfig >/dev/null 2>&1; then
            chkconfig --add hameta >/dev/null 2>&1 || true
            chkconfig hameta on >/dev/null 2>&1 || true
        fi
        log_ok "已安装 SysVinit 服务 (/etc/init.d/hameta)"
    fi
}

start_and_verify_service() {
    log_info "正在通过 service 启动 hameta..."
    if command -v service >/dev/null 2>&1; then
        service hameta start
    elif is_systemd; then
        systemctl start hameta
    else
        "$INIT_DEST" start
    fi

    sleep 1
    if command -v service >/dev/null 2>&1; then
        if service hameta status >/dev/null 2>&1; then
            log_ok "hameta 服务已在后台正常运行"
            return 0
        fi
    elif "$INIT_DEST" status >/dev/null 2>&1; then
        log_ok "hameta 服务已在后台正常运行"
        return 0
    fi

    log_warn "hameta 启动状态校验未通过，请执行 'service hameta status' 或查看日志 ${INSTALL_DIR}/logs/hameta.log"
    return 1
}

print_summary() {
    # shellcheck disable=SC1090
    [ -f "$ENV_PATH" ] && . "$ENV_PATH"
    local bind_addr="${HAMETA_BIND:-0.0.0.0:8080}"
    local port="${bind_addr##*:}"
    local lan_ip=""
    if command -v hostname >/dev/null 2>&1; then
        lan_ip=$(hostname -I 2>/dev/null | awk '{print $1}' || true)
    fi

    echo ""
    echo "${C_GREEN}========================================================================${C_RESET}"
    echo "  Hameta 安装完成！"
    echo "------------------------------------------------------------------------"
    echo "  安装目录:     ${INSTALL_DIR}"
    echo "  交互式菜单:   运行 ${C_CYAN}hameta${C_RESET} 启动运维控制台（含顶部实时进程状态）"
    echo "  服务维护:     ${C_CYAN}service hameta {start|stop|restart|status}${C_RESET}"
    echo "  监听地址:     ${bind_addr}"
    echo "  本机访问:     http://127.0.0.1:${port}"
    [ -n "$lan_ip" ] && echo "  内网访问:     http://${lan_ip}:${port}"
    echo "  首次提示:     打开 Web 面板后将自动进入初始化向导（配置管理员/Redis/Manticore）"
    echo "${C_GREEN}========================================================================${C_RESET}"
}

main() {
    require_root
    check_os
    ensure_base_packages

    local arch stage_dir
    arch=$(detect_arch)
    stage_dir="${TMP_DIR}/stage"
    mkdir -p "$stage_dir"

    if [ -n "$LOCAL_BINARY" ]; then
        log_info "使用本地二进制安装: ${LOCAL_BINARY}"
        stage_from_local_binary "$LOCAL_BINARY" "$stage_dir"
    elif [ "$FROM_SOURCE" -eq 1 ]; then
        build_from_source "$stage_dir"
    else
        if ! try_download_prebuilt "$arch" "$stage_dir"; then
            log_warn "未能获取可用预编译包，自动切换为从源码编译安装..."
            build_from_source "$stage_dir"
        fi
    fi

    install_service_files "$stage_dir"
    start_and_verify_service || true
    print_summary
}

main
