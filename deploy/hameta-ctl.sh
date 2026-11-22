#!/usr/bin/env bash
set -u

export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"

INSTALL_DIR="/opt/hameta"
BIN="${INSTALL_DIR}/hameta"
ENV_FILE="${INSTALL_DIR}/hameta.env"
PID_FILE="${INSTALL_DIR}/run/hameta.pid"

if [ "$#" -gt 0 ]; then
    if [ ! -x "$BIN" ]; then
        echo "错误: 未找到主程序 $BIN" >&2
        exit 1
    fi
    exec "$BIN" "$@"
fi

if [ -f "$ENV_FILE" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
fi

HAMETA_BIND="${HAMETA_BIND:-0.0.0.0:8080}"
HAMETA_DATA_DIR="${HAMETA_DATA_DIR:-${INSTALL_DIR}/data}"
HAMETA_LOG_FILE="${HAMETA_LOG_FILE:-${INSTALL_DIR}/logs/hameta.log}"

if [ -t 1 ]; then
    C_RESET=$'\033[0m'
    C_BOLD=$'\033[1m'
    C_DIM=$'\033[2m'
    C_GREEN=$'\033[32m'
    C_RED=$'\033[31m'
    C_YELLOW=$'\033[33m'
    C_CYAN=$'\033[36m'
else
    C_RESET=""
    C_BOLD=""
    C_DIM=""
    C_GREEN=""
    C_RED=""
    C_YELLOW=""
    C_CYAN=""
fi

is_systemd() {
    [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1
}

service_backend_label() {
    if command -v service >/dev/null 2>&1 && is_systemd; then
        echo "service / systemd"
    elif is_systemd; then
        echo "systemd"
    elif command -v service >/dev/null 2>&1; then
        echo "service / SysVinit"
    else
        echo "init.d"
    fi
}

run_service() {
    local action="$1"
    local prefix=""
    if [ "$action" != "status" ] && [ "$(id -u)" -ne 0 ]; then
        if command -v sudo >/dev/null 2>&1; then
            prefix="sudo"
        else
            echo "${C_RED}需要 root 权限执行 service hameta ${action}${C_RESET}"
            return 1
        fi
    fi

    if command -v service >/dev/null 2>&1 && { [ -x /etc/init.d/hameta ] || [ -f /etc/systemd/system/hameta.service ]; }; then
        # 统一使用 Linux service 命令维护 hameta 服务
        $prefix service hameta "$action"
        return $?
    fi

    if is_systemd && [ -f /etc/systemd/system/hameta.service ]; then
        $prefix systemctl "$action" hameta
        return $?
    fi

    if [ -x /etc/init.d/hameta ]; then
        $prefix /etc/init.d/hameta "$action"
        return $?
    fi

    echo "${C_RED}未检测到已安装的 hameta 服务配置 (/etc/init.d/hameta 或 hameta.service)${C_RESET}"
    return 1
}

is_hameta_panel_pid() {
    local pid="$1"
    [ -n "$pid" ] && [ "$pid" -gt 0 ] 2>/dev/null || return 1
    kill -0 "$pid" 2>/dev/null || return 1
    if [ -r "/proc/${pid}/cmdline" ]; then
        local cmdline
        cmdline=$(tr '\0' ' ' <"/proc/${pid}/cmdline" 2>/dev/null || true)
        case "$cmdline" in
            *hameta*panel*) return 0 ;;
            *) return 1 ;;
        esac
    fi
    return 0
}

find_panel_pid() {
    local pid=""
    if is_systemd && [ -f /etc/systemd/system/hameta.service ]; then
        pid=$(systemctl show -p MainPID --value hameta 2>/dev/null || true)
        if is_hameta_panel_pid "$pid"; then
            echo "$pid"
            return 0
        fi
    fi

    if [ -f "$PID_FILE" ]; then
        pid=$(tr -dc '0-9' <"$PID_FILE" 2>/dev/null || true)
        if is_hameta_panel_pid "$pid"; then
            echo "$pid"
            return 0
        fi
    fi

    if command -v pgrep >/dev/null 2>&1; then
        pid=$(pgrep -f "^${BIN} panel" 2>/dev/null | head -n 1 || true)
        if is_hameta_panel_pid "$pid"; then
            echo "$pid"
            return 0
        fi
    fi

    return 1
}

proc_rss_kib() {
    local pid="$1"
    local kb=""
    if [ -r "/proc/${pid}/status" ]; then
        kb=$(awk '/^VmRSS:/ {print $2; exit}' "/proc/${pid}/status" 2>/dev/null || true)
    fi
    if [ -z "$kb" ] && command -v ps >/dev/null 2>&1; then
        kb=$(ps -p "$pid" -o rss= 2>/dev/null | tr -d ' ' || true)
    fi
    echo "${kb:-0}"
}

format_mib() {
    local kib="${1:-0}"
    awk -v k="$kib" 'BEGIN { printf "%.1f MiB", k / 1024.0 }'
}

proc_cpu_percent() {
    local pid="$1"
    ps -p "$pid" -o %cpu= 2>/dev/null | tr -d ' ' || echo "-"
}

proc_etime() {
    local pid="$1"
    ps -p "$pid" -o etime= 2>/dev/null | tr -d ' ' || echo "-"
}

proc_fd_info() {
    local pid="$1"
    local open_fds="-"
    local limit="-"
    if [ -d "/proc/${pid}/fd" ]; then
        open_fds=$(ls -1 "/proc/${pid}/fd" 2>/dev/null | wc -l | tr -d ' ')
    fi
    if [ -r "/proc/${pid}/limits" ]; then
        limit=$(awk '/^Max open files/ {print $4; exit}' "/proc/${pid}/limits" 2>/dev/null || echo "-")
    fi
    echo "${open_fds} / ${limit}"
}

extract_bind_port() {
    local addr="$1"
    local port="${addr##*:}"
    case "$port" in
        ""|*[!0-9]*) echo "8080" ;;
        *) echo "$port" ;;
    esac
}

probe_health_text() {
    local port
    port=$(extract_bind_port "$HAMETA_BIND")
    if ! command -v curl >/dev/null 2>&1; then
        echo "未知 (未安装 curl)"
        return
    fi
    local body
    body=$(curl -fsS --max-time 1 "http://127.0.0.1:${port}/healthz" 2>/dev/null || true)
    if [ -z "$body" ]; then
        echo "${C_YELLOW}端口未就绪或不可达 (127.0.0.1:${port})${C_RESET}"
        return
    fi
    if echo "$body" | grep -q '"initialized":true'; then
        echo "${C_GREEN}健康 (已完成初始化)${C_RESET}"
    elif echo "$body" | grep -q '"initialized":false'; then
        echo "${C_YELLOW}健康 (待 Web 向导初始化)${C_RESET}"
    else
        echo "${C_GREEN}健康${C_RESET}"
    fi
}

child_pids_of() {
    local parent_pid="$1"
    if command -v pgrep >/dev/null 2>&1; then
        pgrep -P "$parent_pid" 2>/dev/null || true
        return
    fi
    ps -eo pid=,ppid= 2>/dev/null | awk -v p="$parent_pid" '$2 == p {print $1}' || true
}

render_status_header() {
    local version_str="unknown"
    if [ -x "$BIN" ]; then
        version_str=$("$BIN" --version 2>/dev/null | awk '{print $NF}')
        [ -n "$version_str" ] || version_str="unknown"
    fi

    local panel_pid=""
    local backend
    backend=$(service_backend_label)

    echo ""
    echo "${C_CYAN}========================================================================${C_RESET}"
    printf "  %sHameta 运维控制台%s                                    版本: v%s\n" "$C_BOLD" "$C_RESET" "$version_str"
    echo "${C_CYAN}------------------------------------------------------------------------${C_RESET}"

    if panel_pid=$(find_panel_pid); then
        local panel_rss_kib panel_cpu panel_etime panel_fds health_str
        panel_rss_kib=$(proc_rss_kib "$panel_pid")
        panel_cpu=$(proc_cpu_percent "$panel_pid")
        panel_etime=$(proc_etime "$panel_pid")
        panel_fds=$(proc_fd_info "$panel_pid")
        health_str=$(probe_health_text)

        local child_lines=""
        local child_count=0
        local total_rss_kib="$panel_rss_kib"
        local cpid
        for cpid in $(child_pids_of "$panel_pid"); do
            is_hameta_panel_pid "$cpid" && continue
            kill -0 "$cpid" 2>/dev/null || continue
            local cmdline role crss_kib ccpu cetime cmib
            cmdline=$(tr '\0' ' ' <"/proc/${cpid}/cmdline" 2>/dev/null || true)
            case "$cmdline" in
                *hameta*worker*crawler*) role="crawler" ;;
                *hameta*worker*indexer*) role="indexer" ;;
                *hameta*worker*) role="worker " ;;
                *) continue ;;
            esac
            crss_kib=$(proc_rss_kib "$cpid")
            ccpu=$(proc_cpu_percent "$cpid")
            cetime=$(proc_etime "$cpid")
            cmib=$(format_mib "$crss_kib")
            total_rss_kib=$((total_rss_kib + crss_kib))
            child_count=$((child_count + 1))
            child_lines="${child_lines}$(printf "    - %-7s  PID %-7s  CPU %-6s  内存 %-10s  时长 %s" "$role" "$cpid" "${ccpu:-0}%" "$cmib" "$cetime")\n"
        done

        local panel_mib total_mib
        panel_mib=$(format_mib "$panel_rss_kib")
        total_mib=$(format_mib "$total_rss_kib")

        printf "  运行状态: %s● 运行中%s                     维护方式: %s\n" "$C_GREEN" "$C_RESET" "$backend"
        printf "  主进程 PID: %-10s              运行时长: %s\n" "$panel_pid" "$panel_etime"
        printf "  主进程资源: CPU %-6s / %-10s 总内存占用: %s\n" "${panel_cpu:-0}%" "$panel_mib" "$total_mib"
        printf "  文件句柄: %-20s      监听地址: %s\n" "$panel_fds" "$HAMETA_BIND"
        printf "  健康检查: %s\n" "$health_str"
        printf "  Worker 子进程: %d 个运行中\n" "$child_count"
        if [ -n "$child_lines" ]; then
            printf "%b" "$child_lines"
        fi
    else
        printf "  运行状态: %s○ 已停止%s                     维护方式: %s\n" "$C_RED" "$C_RESET" "$backend"
        printf "  主进程 PID: -                         监听配置: %s\n" "$HAMETA_BIND"
        printf "  数据目录:   %s\n" "$HAMETA_DATA_DIR"
        printf "  Worker 子进程: 0 个运行中\n"
    fi
    echo "${C_CYAN}========================================================================${C_RESET}"
}

pause_return() {
    [ -t 0 ] || return 0
    echo ""
    printf "按回车键返回主菜单..."
    read -r _ || true
}

action_toggle_start_stop() {
    echo ""
    if find_panel_pid >/dev/null 2>&1; then
        echo "${C_YELLOW}正在停止 Hameta 服务...${C_RESET}"
        if run_service stop; then
            echo "${C_GREEN}Hameta 已停止${C_RESET}"
        else
            echo "${C_RED}停止 Hameta 失败${C_RESET}"
        fi
    else
        echo "${C_YELLOW}正在启动 Hameta 服务...${C_RESET}"
        if run_service start; then
            echo "${C_GREEN}Hameta 已启动${C_RESET}"
        else
            echo "${C_RED}启动 Hameta 失败，请检查日志: ${HAMETA_LOG_FILE}${C_RESET}"
        fi
    fi
    pause_return
}

action_restart() {
    echo ""
    echo "${C_YELLOW}正在重启 Hameta 服务...${C_RESET}"
    if run_service restart; then
        echo "${C_GREEN}Hameta 已重启${C_RESET}"
    else
        echo "${C_RED}重启 Hameta 失败，请检查日志: ${HAMETA_LOG_FILE}${C_RESET}"
    fi
    pause_return
}

generate_random_password() {
    LC_ALL=C tr -dc 'A-Za-z0-9@#%+=_-' </dev/urandom 2>/dev/null | head -c 16 || echo "Hameta$(date +%s)"
}

extract_json_string() {
    local json="$1"
    local key="$2"
    echo "$json" | tr '\n' ' ' | sed -n "s/.*\"${key}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p"
}

action_reset_admin_password() {
    echo ""
    echo "${C_BOLD}=== 重置管理员密码 ===${C_RESET}"

    if [ ! -x "$BIN" ]; then
        echo "${C_RED}错误: 未找到主程序 $BIN${C_RESET}"
        pause_return
        return
    fi

    local default_user="admin"
    local info_json=""
    info_json=$("$BIN" admin info --data-dir "$HAMETA_DATA_DIR" --json 2>/dev/null || true)
    local current_user
    current_user=$(extract_json_string "$info_json" "admin_username")
    if [ -n "$current_user" ]; then
        default_user="$current_user"
    fi

    local input_user=""
    printf "请输入管理员用户名 [默认: %s]: " "$default_user"
    read -r input_user
    local target_user="${input_user:-$default_user}"
    target_user=$(echo "$target_user" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
    if [ -z "$target_user" ]; then
        echo "${C_RED}错误: 管理员用户名不能为空${C_RESET}"
        pause_return
        return
    fi

    local new_pass=""
    local confirm_pass=""
    local auto_generated=0
    printf "请输入新密码 (至少 6 位，直接回车自动生成 16 位强密码): "
    if [ -t 0 ]; then
        read -r -s new_pass
        echo ""
    else
        read -r new_pass
    fi

    if [ -z "$new_pass" ]; then
        new_pass=$(generate_random_password)
        auto_generated=1
    else
        if [ "${#new_pass}" -lt 6 ]; then
            echo "${C_RED}错误: 密码长度不能少于 6 位${C_RESET}"
            pause_return
            return
        fi
        if [ -t 0 ]; then
            printf "请再次输入新密码确认: "
            read -r -s confirm_pass
            echo ""
            if [ "$new_pass" != "$confirm_pass" ]; then
                echo "${C_RED}错误: 两次输入的密码不一致${C_RESET}"
                pause_return
                return
            fi
        fi
    fi

    local prefix=""
    if [ "$(id -u)" -ne 0 ] && [ -d "$HAMETA_DATA_DIR" ] && [ ! -w "$HAMETA_DATA_DIR" ]; then
        if command -v sudo >/dev/null 2>&1; then
            prefix="sudo"
        fi
    fi

    echo ""
    if $prefix "$BIN" admin reset --data-dir "$HAMETA_DATA_DIR" --username "$target_user" --password "$new_pass"; then
        echo "${C_GREEN}管理员凭证已更新，所有历史登录会话已失效。${C_RESET}"
        echo "  用户名: ${target_user}"
        if [ "$auto_generated" -eq 1 ]; then
            echo "  新密码: ${C_YELLOW}${new_pass}${C_RESET}  (请妥善保存)"
        fi
    else
        echo "${C_RED}重置管理员密码失败${C_RESET}"
    fi
    pause_return
}

action_view_admin_info() {
    echo ""
    echo "${C_BOLD}=== Hameta 管理信息 ===${C_RESET}"

    local port
    port=$(extract_bind_port "$HAMETA_BIND")
    local lan_ip=""
    if command -v hostname >/dev/null 2>&1; then
        lan_ip=$(hostname -I 2>/dev/null | awk '{print $1}' || true)
    fi
    local pub_ip=""
    if command -v curl >/dev/null 2>&1; then
        pub_ip=$(curl -fsS --max-time 2 https://api.ipify.org 2>/dev/null || true)
    fi

    echo "监听绑定:       ${HAMETA_BIND}"
    echo "本机面板地址:   http://127.0.0.1:${port}"
    [ -n "$lan_ip" ] && echo "内网访问地址:   http://${lan_ip}:${port}"
    [ -n "$pub_ip" ] && echo "公网访问地址:   http://${pub_ip}:${port}"
    echo "安装目录:       ${INSTALL_DIR}"
    echo "环境变量文件:   ${ENV_FILE}"
    echo "数据目录:       ${HAMETA_DATA_DIR}"
    echo "服务日志文件:   ${HAMETA_LOG_FILE}"
    echo "日常维护命令:   service hameta {start|stop|restart|status}"
    if is_systemd && [ -f /etc/systemd/system/hameta.service ]; then
        echo "Systemd 日志:   journalctl -u hameta -n 100 --no-pager"
    fi

    echo ""
    echo "${C_DIM}--- 实例状态摘要 ---${C_RESET}"
    if [ -x "$BIN" ]; then
        "$BIN" admin info --data-dir "$HAMETA_DATA_DIR" || echo "${C_RED}读取实例信息失败${C_RESET}"
    else
        echo "${C_RED}未找到主程序 $BIN${C_RESET}"
    fi
    pause_return
}

main_loop() {
    while true; do
        if [ -t 1 ]; then
            clear 2>/dev/null || true
        fi
        render_status_header

        local toggle_hint="启动程序"
        if find_panel_pid >/dev/null 2>&1; then
            toggle_hint="停止程序 (当前运行中)"
        else
            toggle_hint="启动程序 (当前已停止)"
        fi

        echo "  1、启动/停止程序   [${toggle_hint}]"
        echo "  2、重启程序"
        echo "  3、重置管理员密码"
        echo "  4、查看管理信息"
        echo "  0、退出菜单        [直接回车刷新状态]"
        echo ""
        printf "请输入操作编号 [0-4]: "

        local choice=""
        if ! read -r choice; then
            echo ""
            break
        fi

        case "$choice" in
            1)
                action_toggle_start_stop
                ;;
            2)
                action_restart
                ;;
            3)
                action_reset_admin_password
                ;;
            4)
                action_view_admin_info
                ;;
            0|q|Q|exit)
                echo "已退出。"
                break
                ;;
            "")
                ;;
            *)
                echo "${C_RED}无效选项: ${choice}，请输入 0-4${C_RESET}"
                sleep 1
                ;;
        esac
    done
}

main_loop
