#!/bin/sh
# libra installer - A robust installation script for libra
# Usage: curl -sSf https://download.libra.tools/install.sh | sh

set -e

# Configuration
BASE_URL="${LIBRA_BASE_URL:-https://download.libra.tools/libra/releases}"
INSTALL_DIR="${LIBRA_INSTALL_DIR:-/usr/local/bin}"
DEFAULT_VERSION="v0.1.1"

# Color codes for output
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    NC='\033[0m' # No Color
else
    RED=''
    GREEN=''
    YELLOW=''
    BLUE=''
    NC=''
fi

# Logging functions
info() {
    printf "${BLUE}info:${NC} %s\n" "$1"
}

success() {
    printf "${GREEN}success:${NC} %s\n" "$1"
}

warn() {
    printf "${YELLOW}warning:${NC} %s\n" "$1" >&2
}

error() {
    printf "${RED}error:${NC} %s\n" "$1" >&2
    exit 1
}

# Usage information
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
    curl -sSf https://download.libra.tools/install.sh | sh

    # Install specific version
    curl -sSf https://download.libra.tools/install.sh | sh -s -- -v v0.1.0

    # Install to custom directory
    curl -sSf https://download.libra.tools/install.sh | sh -s -- -d ~/.libra/bin

ENVIRONMENT VARIABLES:
    LIBRA_VERSION              Override version detection
    LIBRA_INSTALL_DIR          Override installation directory
    LIBRA_BASE_URL             Override download base URL

EOF
    exit 0
}

# Parse command line arguments
parse_args() {
    VERSION="${LIBRA_VERSION:-}"

    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                usage
                ;;
            -v|--version)
                VERSION="$2"
                shift 2
                ;;
            -d|--dir)
                INSTALL_DIR="$2"
                shift 2
                ;;
            *)
                error "Unknown option: $1. Use --help for usage information."
                ;;
        esac
    done
}

# Detect OS
detect_os() {
    OS="$(uname -s)"
    case "$OS" in
        Linux)
            OS="linux"
            ;;
        Darwin)
            OS="darwin"
            ;;
        *)
            error "Unsupported operating system: $OS"
            ;;
    esac
    echo "$OS"
}

# Detect architecture
detect_arch() {
    ARCH="$(uname -m)"
    case "$ARCH" in
        x86_64|amd64)
            ARCH="amd64"
            ;;
        aarch64|arm64)
            ARCH="arm64"
            ;;
        *)
            error "Unsupported architecture: $ARCH"
            ;;
    esac
    echo "$ARCH"
}

# Check for required tools
check_dependencies() {
    if command -v curl >/dev/null 2>&1; then
        DOWNLOADER="curl"
    elif command -v wget >/dev/null 2>&1; then
        DOWNLOADER="wget"
    else
        error "Neither curl nor wget found. Please install one of them."
    fi
}

# Download file
download_file() {
    local url="$1"
    local output="$2"

    if [ "$DOWNLOADER" = "curl" ]; then
        curl -fsSL "$url" -o "$output" || return 1
    else
        wget -q "$url" -O "$output" || return 1
    fi
    return 0
}

# Fetch latest version from GitHub API
fetch_latest_version() {
    local api_url="https://api.github.com/repos/web3infra-foundation/libra/releases/latest"
    local version=""

    if [ "$DOWNLOADER" = "curl" ]; then
        version=$(curl -fsSL "$api_url" | grep '"tag_name":' | head -n1 | sed -E 's/.*"tag_name": "([^"]+)".*/\1/' 2>/dev/null || true)
    else
        version=$(wget -qO- "$api_url" | grep '"tag_name":' | head -n1 | sed -E 's/.*"tag_name": "([^"]+)".*/\1/' 2>/dev/null || true)
    fi

    if [ -z "$version" ]; then
        version="$DEFAULT_VERSION"
    fi

    echo "$version"
}

# Pre-flight checks
preflight_checks() {
    info "Running pre-flight checks..."

    # Check GLIBC version on Linux
    if [ "$OS" = "linux" ]; then
        if command -v ldd >/dev/null 2>&1; then
            GLIBC_VERSION=$(ldd --version 2>&1 | head -n1 | grep -oE '[0-9]+\.[0-9]+' | head -n1)
            info "Detected GLIBC version: $GLIBC_VERSION"

            # libra requires GLIBC 2.31+ (Ubuntu 20.04+)
            GLIBC_MAJOR=$(echo "$GLIBC_VERSION" | cut -d. -f1)
            GLIBC_MINOR=$(echo "$GLIBC_VERSION" | cut -d. -f2)

            if [ "$GLIBC_MAJOR" -lt 2 ] || { [ "$GLIBC_MAJOR" -eq 2 ] && [ "$GLIBC_MINOR" -lt 31 ]; }; then
                warn "GLIBC version $GLIBC_VERSION detected. libra may require GLIBC 2.31 or higher."
            fi
        fi
    fi

    # Check available disk space (require at least 50MB)
    if command -v df >/dev/null 2>&1; then
        AVAILABLE_KB=$(df -k "$(dirname "$INSTALL_DIR")" | tail -1 | awk '{print $4}')
        if [ "$AVAILABLE_KB" -lt 51200 ]; then
            warn "Low disk space detected. At least 50MB recommended."
        fi
    fi
}

# Install libra
install_libra() {
    local binary_name="libra-${OS}-${ARCH}"
    local download_url="${BASE_URL}/${VERSION}/${binary_name}"
    local temp_file="${TEMP_DIR}/${binary_name}"

    info "Downloading libra from $download_url..."

    if ! download_file "$download_url" "$temp_file"; then
        error "Failed to download libra. Please check version and try again."
    fi

    # Verify download
    if [ ! -f "$temp_file" ] || [ ! -s "$temp_file" ]; then
        error "Downloaded file is empty or missing: $temp_file"
    fi

    # Make executable
    chmod +x "$temp_file"

    # Install to target directory
    local target_path="${INSTALL_DIR}/libra"

    info "Installing libra to $target_path..."

    # Check if we need sudo
    if [ -w "$INSTALL_DIR" ]; then
        mv "$temp_file" "$target_path"
    else
        if command -v sudo >/dev/null 2>&1; then
            sudo mv "$temp_file" "$target_path"
        else
            error "No write permission to $INSTALL_DIR and sudo not available. Try: export LIBRA_INSTALL_DIR=~/.libra/bin"
        fi
    fi

    success "Installed libra successfully"
}

# Main installation logic
main() {
    parse_args "$@"

    info "libra installer"
    info "==============="

    # Detect system
    OS=$(detect_os)
    ARCH=$(detect_arch)
    info "Detected platform: ${OS}/${ARCH}"

    # Check dependencies
    check_dependencies
    info "Using downloader: $DOWNLOADER"

    # Get version
    if [ -z "$VERSION" ]; then
        VERSION=$(fetch_latest_version)
        info "Fetched version: $VERSION"
    else
        info "Using specified version: $VERSION"
    fi

    # Pre-flight checks
    preflight_checks

    # Create temporary directory
    TEMP_DIR=$(mktemp -d)
    trap 'rm -rf "$TEMP_DIR"' EXIT

    # Create install directory if it doesn't exist
    if [ ! -d "$INSTALL_DIR" ]; then
        info "Creating installation directory: $INSTALL_DIR"
        if mkdir -p "$INSTALL_DIR" 2>/dev/null; then
            :
        else
            if command -v sudo >/dev/null 2>&1; then
                sudo mkdir -p "$INSTALL_DIR"
            else
                error "Cannot create $INSTALL_DIR. Try: export LIBRA_INSTALL_DIR=~/.libra/bin"
            fi
        fi
    fi

    # Install libra
    install_libra

    # Final success message
    echo ""
    info "Installed to: $INSTALL_DIR"

    case "$PATH" in
        *"$INSTALL_DIR"*)
            ;;
        *)
            echo ""
            warn "The installation directory is not in your PATH."
            info "Add it to your PATH by running:"
            info "  export PATH=\"\$PATH:$INSTALL_DIR\""
            echo ""
            info "To make it permanent, add the above line to your shell profile:"
            info "  ~/.bashrc (bash) or ~/.zshrc (zsh)"
            ;;
    esac

    echo ""
    info "Verify installation by running:"
    info "  libra --version"
}

main "$@"