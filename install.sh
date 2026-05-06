#!/bin/sh
# libra installer · TUI
#
#   curl -fsSL https://libra.tools/install.sh | sh
#   curl -fsSL https://libra.tools/install.sh | sh -s -- -v v0.1.0
#
# Visual design ports the Libra TUI Installer mock — banner, conversational
# agent voice, animated per-step spinner, themed colors, success box.
# Set NO_COLOR=1 or LIBRA_NO_TUI=1 (or pipe to a non-tty) for plain output.

set -e

# ─── config ──────────────────────────────────────────────────────────────────
BASE_URL="${LIBRA_BASE_URL:-https://download.libra.tools/libra/releases}"
INSTALL_DIR="${LIBRA_INSTALL_DIR:-/usr/local/bin}"
DEFAULT_VERSION="v0.1.1"

# ─── theme (Dusk) ────────────────────────────────────────────────────────────
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ -z "${LIBRA_NO_TUI:-}" ] && [ "${TERM:-dumb}" != "dumb" ]; then
    TTY=1
else
    TTY=0
fi

if [ "$TTY" = "1" ]; then
    C_RESET=$(printf '\033[0m')
    C_BOLD=$(printf '\033[1m')
    C_DIM=$(printf '\033[38;5;244m')
    C_TEXT=$(printf '\033[38;5;252m')
    C_ACCENT=$(printf '\033[38;5;117m')
    C_ACCENT2=$(printf '\033[38;5;159m')
    C_SUCCESS=$(printf '\033[38;5;114m')
    C_WARN=$(printf '\033[38;5;221m')
    C_ERROR=$(printf '\033[38;5;210m')
    C_HIDE=$(printf '\033[?25l')
    C_SHOW=$(printf '\033[?25h')
    C_CLR=$(printf '\r\033[K')
    if sleep 0.05 2>/dev/null; then SPIN_DELAY=0.08; else SPIN_DELAY=1; fi
else
    C_RESET=; C_BOLD=; C_DIM=; C_TEXT=
    C_ACCENT=; C_ACCENT2=; C_SUCCESS=; C_WARN=; C_ERROR=
    C_HIDE=; C_SHOW=; C_CLR=
    SPIN_DELAY=1
fi

cleanup() {
    [ -n "${TEMP_DIR:-}" ] && rm -rf "$TEMP_DIR"
    [ "$TTY" = "1" ] && printf '%s' "$C_SHOW"
    return 0
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

# ─── drawing primitives ──────────────────────────────────────────────────────
banner() {
    printf '\n'
    printf '%s%s  ██╗     ██╗ ██████╗ ██████╗  █████╗ %s\n' "$C_BOLD" "$C_ACCENT" "$C_RESET"
    printf '%s%s  ██║     ██║ ██╔══██╗██╔══██╗██╔══██╗%s\n' "$C_BOLD" "$C_ACCENT" "$C_RESET"
    printf '%s%s  ██║     ██║ ██████╔╝██████╔╝███████║%s\n' "$C_BOLD" "$C_ACCENT" "$C_RESET"
    printf '%s%s  ██║     ██║ ██╔══██╗██╔══██╗██╔══██║%s\n' "$C_BOLD" "$C_ACCENT" "$C_RESET"
    printf '%s%s  ███████╗██║ ██████╔╝██║  ██║██║  ██║%s\n' "$C_BOLD" "$C_ACCENT" "$C_RESET"
    printf '%s%s  ╚══════╝╚═╝ ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝%s\n' "$C_BOLD" "$C_ACCENT" "$C_RESET"
    printf '    %s▸%s %sAI-agent-native version control · %s%s%s\n\n' \
        "$C_DIM" "$C_RESET" "$C_TEXT" "$C_ACCENT" "${VERSION:-$DEFAULT_VERSION}" "$C_RESET"
}

# Conversational box: ┌─ ◆ libra-agent ─… / └─…
agent_say() {
    if [ "$TTY" = "1" ]; then
        printf '%s┌─%s ◆ libra-agent %s─────────────────────────────────────────────────%s\n' \
            "$C_DIM" "$C_ACCENT" "$C_DIM" "$C_RESET"
        printf '  %s%s%s\n' "$C_TEXT" "$1" "$C_RESET"
        printf '%s└──────────────────────────────────────────────────────────────────────%s\n\n' \
            "$C_DIM" "$C_RESET"
    else
        printf '[libra-agent] %s\n\n' "$1"
    fi
}

section() {
    printf '  %s── %s ──%s\n' "$C_DIM" "$1" "$C_RESET"
}

fact() {
    printf '  %s✓%s  %s%-20s%s %s%s%s\n' \
        "$C_SUCCESS" "$C_RESET" \
        "$C_TEXT" "$1" "$C_RESET" \
        "$C_DIM" "$2" "$C_RESET"
}

warn_fact() {
    printf '  %s!%s  %s%-20s%s %s%s%s\n' \
        "$C_WARN" "$C_RESET" \
        "$C_TEXT" "$1" "$C_RESET" \
        "$C_DIM" "$2" "$C_RESET"
}

# Run a command with a Braille spinner; replace with ✓/✗ on completion.
run_step() {
    label=$1
    shift
    if [ "$TTY" != "1" ]; then
        printf '  ·  %s ... ' "$label"
        if "$@" >/dev/null 2>&1; then
            printf 'ok\n'
            return 0
        else
            rc=$?
            printf 'fail\n'
            return $rc
        fi
    fi

    log=$(mktemp 2>/dev/null || printf '/tmp/libra-step.%s' "$$")
    ( "$@" ) >"$log" 2>&1 &
    pid=$!

    printf '%s' "$C_HIDE"
    i=0
    while kill -0 "$pid" 2>/dev/null; do
        case $((i % 10)) in
            0) f='⠋' ;; 1) f='⠙' ;; 2) f='⠹' ;; 3) f='⠸' ;; 4) f='⠼' ;;
            5) f='⠴' ;; 6) f='⠦' ;; 7) f='⠧' ;; 8) f='⠇' ;; 9) f='⠏' ;;
        esac
        printf '%s  %s%s%s  %s%s%s' "$C_CLR" "$C_ACCENT" "$f" "$C_RESET" "$C_TEXT" "$label" "$C_RESET"
        i=$((i + 1))
        sleep "$SPIN_DELAY" 2>/dev/null || true
    done

    if wait "$pid"; then rc=0; else rc=$?; fi
    printf '%s' "$C_CLR"
    printf '%s' "$C_SHOW"

    if [ "$rc" = "0" ]; then
        printf '  %s✓%s  %s%s%s\n' "$C_SUCCESS" "$C_RESET" "$C_TEXT" "$label" "$C_RESET"
    else
        printf '  %s✗%s  %s%s%s\n' "$C_ERROR" "$C_RESET" "$C_ERROR" "$label" "$C_RESET"
        if [ -s "$log" ]; then
            while IFS= read -r ln; do
                printf '       %s%s%s\n' "$C_DIM" "$ln" "$C_RESET"
            done <"$log"
        fi
    fi
    rm -f "$log"
    return $rc
}

success_box() {
    printf '  %s%s╭───────────────────────────────╮%s\n' "$C_BOLD" "$C_SUCCESS" "$C_RESET"
    printf '  %s%s│                               │%s\n' "$C_BOLD" "$C_SUCCESS" "$C_RESET"
    printf '  %s%s│   ✓  libra is ready to use    │%s\n' "$C_BOLD" "$C_SUCCESS" "$C_RESET"
    printf '  %s%s│                               │%s\n' "$C_BOLD" "$C_SUCCESS" "$C_RESET"
    printf '  %s%s╰───────────────────────────────╯%s\n\n' "$C_BOLD" "$C_SUCCESS" "$C_RESET"
}

# Rust-compiler-styled error block + recovery hints; exits 1.
error_exit() {
    msg=$1
    stage=${2:-install}
    detail=${3:-}
    printf '\n  %s✗ install failed at stage — %s%s\n\n' "$C_ERROR" "$stage" "$C_RESET"
    printf '  %s┃%s  %serror:%s %s\n' "$C_ERROR" "$C_RESET" "$C_ERROR" "$C_RESET" "$msg"
    if [ -n "$detail" ]; then
        printf '  %s┃%s  %s%s%s\n' "$C_ERROR" "$C_RESET" "$C_DIM" "$detail" "$C_RESET"
    fi
    printf '  %s┃%s\n' "$C_ERROR" "$C_RESET"
    printf '  %s┗━%s I know this kind of failure. Try one of these:\n' "$C_ERROR" "$C_RESET"
    printf '       %s▸%s install to a user-writable dir   %sexport LIBRA_INSTALL_DIR=~/.libra/bin%s\n' \
        "$C_ACCENT" "$C_RESET" "$C_ACCENT2" "$C_RESET"
    printf '       %s▸%s pin a known-good version         %scurl -fsSL libra.tools/install.sh | sh -s -- -v v0.1.0%s\n' \
        "$C_ACCENT" "$C_RESET" "$C_ACCENT2" "$C_RESET"
    printf '       %s▸%s open a bug report                %sgithub.com/web3infra-foundation/libra/issues%s\n' \
        "$C_ACCENT" "$C_RESET" "$C_ACCENT2" "$C_RESET"
    printf '\n  %sa full log was saved to %s/.libra/install-fail-%s.log%s\n\n' \
        "$C_DIM" "${HOME:-/tmp}" "$(date +%Y-%m-%d 2>/dev/null || printf 'today')" "$C_RESET"
    exit 1
}

# ─── argument parsing ────────────────────────────────────────────────────────
usage() {
    cat <<EOF
libra installer

USAGE:
    install.sh [OPTIONS]

OPTIONS:
    -v, --version <VERSION>    Specify version (default: latest)
    -d, --dir <PATH>           Installation directory (default: /usr/local/bin)
    -h, --help                 Show this help message

EXAMPLES:
    # Install latest version
    curl -fsSL https://libra.tools/install.sh | sh

    # Install specific version
    curl -fsSL https://libra.tools/install.sh | sh -s -- -v v0.1.0

    # Install to custom directory
    curl -fsSL https://libra.tools/install.sh | sh -s -- -d ~/.libra/bin

ENVIRONMENT VARIABLES:
    LIBRA_VERSION              Override version detection
    LIBRA_INSTALL_DIR          Override installation directory
    LIBRA_BASE_URL             Override download base URL
    NO_COLOR / LIBRA_NO_TUI    Disable colored / animated output
EOF
    exit 0
}

parse_args() {
    VERSION="${LIBRA_VERSION:-}"
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)    usage ;;
            -v|--version) VERSION="$2"; shift 2 ;;
            -d|--dir)     INSTALL_DIR="$2"; shift 2 ;;
            *) error_exit "unknown option: $1" "args" "use --help to see supported flags" ;;
        esac
    done
}

# ─── platform detection ──────────────────────────────────────────────────────
detect_os() {
    OS_RAW=$(uname -s)
    case "$OS_RAW" in
        Linux)  OS=linux  ;;
        Darwin) OS=darwin ;;
        *) error_exit "unsupported operating system: $OS_RAW" "detect" "libra ships builds for linux & darwin" ;;
    esac
}

detect_arch() {
    ARCH_RAW=$(uname -m)
    case "$ARCH_RAW" in
        x86_64|amd64)  ARCH=amd64 ;;
        aarch64|arm64) ARCH=arm64 ;;
        *) error_exit "unsupported architecture: $ARCH_RAW" "detect" "libra builds amd64 and arm64" ;;
    esac
}

check_dependencies() {
    if command -v curl >/dev/null 2>&1; then
        DOWNLOADER=curl
    elif command -v wget >/dev/null 2>&1; then
        DOWNLOADER=wget
    else
        error_exit "neither curl nor wget found" "detect" "install one of them, then re-run"
    fi
}

download_file() {
    if [ "$DOWNLOADER" = "curl" ]; then
        curl -fsSL "$1" -o "$2"
    else
        wget -q "$1" -O "$2"
    fi
}

fetch_latest_version() {
    api_url="https://api.github.com/repos/web3infra-foundation/libra/releases/latest"
    if [ "$DOWNLOADER" = "curl" ]; then
        v=$(curl -fsSL "$api_url" 2>/dev/null | grep '"tag_name":' | head -n1 | sed -E 's/.*"tag_name": "([^"]+)".*/\1/' || true)
    else
        v=$(wget -qO- "$api_url" 2>/dev/null | grep '"tag_name":' | head -n1 | sed -E 's/.*"tag_name": "([^"]+)".*/\1/' || true)
    fi
    [ -n "$v" ] || v="$DEFAULT_VERSION"
    printf '%s' "$v"
}

probe_network() {
    if [ "$DOWNLOADER" = "curl" ]; then
        curl -fsSL --max-time 4 -o /dev/null https://api.github.com 2>/dev/null
    else
        wget -q --tries=1 --timeout=4 -O /dev/null https://api.github.com 2>/dev/null
    fi
}

# ─── screens (ports of the design) ───────────────────────────────────────────
screen_welcome() {
    banner
    agent_say "Hi — I'm the libra installer. I'll set up the AI-agent-native VCS for you in about 30 seconds. I'll show you what I'm doing at every step."
    printf '  %sgithub.com/web3infra-foundation/libra%s\n'   "$C_DIM" "$C_RESET"
    printf '  %scurl -fsSL libra.tools/install.sh | sh%s\n\n' "$C_DIM" "$C_RESET"
    [ "$TTY" = "1" ] && sleep 0.5 2>/dev/null || true
}

screen_detect() {
    section "01 · detect environment"
    agent_say "Scanning your system. This won't change anything yet — just looking around."

    fact "operating system" "$OS_RAW ($OS)"
    fact "architecture"     "$ARCH_RAW ($ARCH)"

    dl_ver=$($DOWNLOADER --version 2>/dev/null | head -n1 | awk '{print $2}')
    fact "downloader"       "$DOWNLOADER ${dl_ver:-?}"

    if command -v git >/dev/null 2>&1; then
        git_ver=$(git --version 2>/dev/null | awk '{print $3}')
        fact "git"          "${git_ver:-found} — will coexist"
    else
        warn_fact "git"     "not found — libra works without it"
    fi

    if command -v df >/dev/null 2>&1; then
        check_dir=$(dirname "$INSTALL_DIR")
        [ -d "$check_dir" ] || check_dir="${HOME:-/}"
        avail_kb=$(df -k "$check_dir" 2>/dev/null | awk 'NR==2 {print $4}')
        if [ -n "$avail_kb" ] && [ "$avail_kb" -gt 0 ] 2>/dev/null; then
            avail_mb=$((avail_kb / 1024))
            if [ "$avail_kb" -lt 51200 ]; then
                warn_fact "disk space" "${avail_mb} MB available — low (50 MB+ recommended)"
            else
                fact "disk space" "${avail_mb} MB available"
            fi
        fi
    fi

    if probe_network; then
        fact "network"      "github.com reachable"
    else
        warn_fact "network" "github.com unreachable — using fallback ${DEFAULT_VERSION}"
    fi

    fact "shell"            "${SHELL:-unknown}"

    if [ "$OS" = "linux" ] && command -v ldd >/dev/null 2>&1; then
        glibc=$(ldd --version 2>&1 | head -n1 | grep -oE '[0-9]+\.[0-9]+' | head -n1)
        if [ -n "$glibc" ]; then
            major=$(echo "$glibc" | cut -d. -f1)
            minor=$(echo "$glibc" | cut -d. -f2)
            if [ "$major" -lt 2 ] || { [ "$major" -eq 2 ] && [ "$minor" -lt 31 ]; }; then
                warn_fact "glibc"   "$glibc — libra prefers 2.31+"
            else
                fact "glibc"        "$glibc"
            fi
        fi
    fi

    printf '\n'
    agent_say "Everything checks out. You're on a supported platform with the toolchain I need."
}

screen_method() {
    section "02 · choose install method"
    agent_say "Picking the prebuilt binary — fastest, signed, ready in seconds. (cargo / source builds also available; re-run with --help to see flags.)"
    printf '  %s▸%s %s%sPrebuilt binary%s  %s(recommended)%s\n' \
        "$C_ACCENT" "$C_RESET" "$C_BOLD" "$C_TEXT" "$C_RESET" "$C_ACCENT2" "$C_RESET"
    printf '      %ssize:%s   ~12 MB compressed\n'  "$C_DIM" "$C_RESET"
    printf '      %stime:%s   a few seconds\n'      "$C_DIM" "$C_RESET"
    printf '      %sneeds:%s  %s\n\n'               "$C_DIM" "$C_RESET" "$DOWNLOADER"
}

screen_install() {
    section "03 · install"
    agent_say "Downloading and installing libra ${VERSION} for ${OS}/${ARCH} into ${INSTALL_DIR}."

    binary_name="libra-${OS}-${ARCH}"
    download_url="${BASE_URL}/${VERSION}/${binary_name}"
    TEMP_DIR=$(mktemp -d)
    temp_file="${TEMP_DIR}/${binary_name}"

    if [ ! -d "$INSTALL_DIR" ]; then
        if ! mkdir -p "$INSTALL_DIR" 2>/dev/null; then
            if command -v sudo >/dev/null 2>&1; then
                run_step "create $INSTALL_DIR (sudo)" sudo mkdir -p "$INSTALL_DIR" \
                    || error_exit "could not create install dir" "install" "set LIBRA_INSTALL_DIR to a writable path"
            else
                error_exit "cannot create $INSTALL_DIR" "install" "set LIBRA_INSTALL_DIR to a writable path"
            fi
        fi
    fi

    run_step "fetch $binary_name" download_file "$download_url" "$temp_file" \
        || error_exit "download failed" "install" "url: $download_url"

    [ -s "$temp_file" ] || error_exit "downloaded file is empty" "install" "the mirror may be corrupted — please retry"

    BIN_SIZE=$(wc -c <"$temp_file" 2>/dev/null | awk '{printf "%.1f MB", $1/1048576}')

    run_step "verify & make executable" chmod +x "$temp_file" \
        || error_exit "could not chmod binary" "install"

    target="${INSTALL_DIR}/libra"
    if [ -w "$INSTALL_DIR" ]; then
        run_step "install to $target" mv "$temp_file" "$target" \
            || error_exit "could not install to $target" "install"
    elif command -v sudo >/dev/null 2>&1; then
        run_step "install to $target (sudo)" sudo mv "$temp_file" "$target" \
            || error_exit "could not install to $target" "install"
    else
        error_exit "no write permission to $INSTALL_DIR" "install" \
            "set LIBRA_INSTALL_DIR to a writable path (e.g. ~/.libra/bin)"
    fi

    INSTALLED_PATH="$target"
    printf '\n'
}

screen_shell() {
    section "04 · shell integration"
    case ":$PATH:" in
        *":$INSTALL_DIR:"*)
            agent_say "${INSTALL_DIR} is already on your PATH — nothing else to wire up."
            ;;
        *)
            agent_say "${INSTALL_DIR} isn't on your PATH yet. Add the line below to your shell profile (~/.zshrc, ~/.bashrc) so libra works in new terminals."
            printf '  %spreview · append to %s%s\n' "$C_DIM" "${SHELL:-~/.zshrc}" "$C_RESET"
            printf '  %s# libra%s\n' "$C_SUCCESS" "$C_RESET"
            # shellcheck disable=SC2016  # $PATH must stay literal — it's shown to the user, not expanded
            printf '  %sexport PATH="%s:$PATH"%s\n\n' "$C_TEXT" "$INSTALL_DIR" "$C_RESET"
            ;;
    esac
}

screen_success() {
    success_box
    agent_say "Installed in about 30 seconds. You're all set — here's what to try first:"

    pad="                                       "
    fmtcmd() {
        cmd=$1; desc=$2
        len=${#cmd}
        # right-pad cmd to width 38
        if [ "$len" -lt 38 ]; then
            sp=$(printf '%s' "$pad" | cut -c1-$((38 - len)))
        else
            sp=' '
        fi
        printf '  %s$%s %s%s%s%s%s  %s%s%s\n' \
            "$C_DIM" "$C_RESET" \
            "$C_BOLD" "$C_ACCENT" "$cmd" "$C_RESET" "$sp" \
            "$C_DIM" "$desc" "$C_RESET"
    }

    fmtcmd 'libra init'                              'turn the current directory into a libra repo'
    fmtcmd 'libra agent ask "review my changes"'     'let the agent take a look'
    fmtcmd 'libra status'                            'familiar — works just like git'
    fmtcmd 'libra --help'                            'every command, with examples'
    printf '\n'

    section "installed"
    printf '  %s✓%s libra %s%s · %s · %s%s\n\n' \
        "$C_SUCCESS" "$C_RESET" \
        "$C_TEXT" "$VERSION" "${BIN_SIZE:-binary}" "${INSTALLED_PATH:-${INSTALL_DIR}/libra}" "$C_RESET"

    section "next"
    printf '  %s📖 docs.libra.tools%s\n'                          "$C_TEXT" "$C_RESET"
    printf '  %s💬 discord.libra.tools%s\n'                       "$C_TEXT" "$C_RESET"
    printf '  %s⭐ github.com/web3infra-foundation/libra%s\n\n'   "$C_TEXT" "$C_RESET"
}

# ─── main ────────────────────────────────────────────────────────────────────
main() {
    parse_args "$@"
    detect_os
    detect_arch
    check_dependencies

    if [ -z "$VERSION" ]; then
        VERSION=$(fetch_latest_version)
    fi

    screen_welcome
    screen_detect
    screen_method
    screen_install
    screen_shell
    screen_success
}

main "$@"
