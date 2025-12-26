#!/bin/sh
# Cocoon Installer
# Usage: curl -fsSL https://adi.the-ihor.com/cocoon/install.sh | sh
#
# One-command install with setup token:
#   curl -fsSL https://adi.the-ihor.com/cocoon/install.sh | sh -s -- <setup-token>
#
# Environment variables:
#   COCOON_INSTALL_DIR    - Installation directory (default: ~/.local/bin)
#   COCOON_VERSION        - Specific version to install (default: latest)
#   SIGNALING_SERVER_URL  - Signaling server URL (default: wss://signal.adi.the-ihor.com/ws)
#   COCOON_SECRET         - Pre-generated secret (optional, auto-generated if not set)

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
NC='\033[0m' # No Color

REPO="adi-family/cocoon"
BINARY_NAME="cocoon"
DEFAULT_SIGNALING_URL="wss://signal.adi.the-ihor.com/ws"

info() {
    printf "${CYAN}info${NC} %s\n" "$1"
}

success() {
    printf "${GREEN}done${NC} %s\n" "$1"
}

warn() {
    printf "${YELLOW}warn${NC} %s\n" "$1"
}

error() {
    printf "${RED}error${NC} %s\n" "$1" >&2
    exit 1
}

# Detect OS
detect_os() {
    case "$(uname -s)" in
        Darwin)
            echo "darwin"
            ;;
        Linux)
            echo "linux"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            error "Windows is not supported. Use WSL2 or Docker instead."
            ;;
        *)
            error "Unsupported operating system: $(uname -s)"
            ;;
    esac
}

# Detect architecture
detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)
            echo "x86_64"
            ;;
        arm64|aarch64)
            echo "aarch64"
            ;;
        *)
            error "Unsupported architecture: $(uname -m)"
            ;;
    esac
}

# Get target triple
get_target() {
    local os="$1"
    local arch="$2"

    case "$os" in
        darwin)
            echo "${arch}-apple-darwin"
            ;;
        linux)
            echo "${arch}-unknown-linux-musl"
            ;;
    esac
}

# Fetch latest version from GitHub API
fetch_latest_version() {
    local url="https://api.github.com/repos/${REPO}/releases/latest"

    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" 2>/dev/null | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/'
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "$url" 2>/dev/null | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/'
    else
        error "Neither curl nor wget found. Please install one of them."
    fi
}

# Download file
download() {
    local url="$1"
    local output="$2"

    info "Downloading from $url"

    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$output"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$url" -O "$output"
    else
        error "Neither curl nor wget found"
    fi
}

# Verify checksum
verify_checksum() {
    local file="$1"
    local expected="$2"

    if [ -z "$expected" ]; then
        warn "Skipping checksum verification (checksum not available)"
        return 0
    fi

    local actual=""
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$file" | cut -d' ' -f1)
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$file" | cut -d' ' -f1)
    else
        warn "Skipping checksum verification (sha256sum/shasum not found)"
        return 0
    fi

    if [ "$actual" != "$expected" ]; then
        error "Checksum verification failed!\nExpected: $expected\nActual: $actual"
    fi

    success "Checksum verified"
}

# Extract archive
extract() {
    local archive="$1"
    local dest="$2"

    info "Extracting archive"

    case "$archive" in
        *.tar.gz|*.tgz)
            tar -xzf "$archive" -C "$dest"
            ;;
        *.zip)
            unzip -q "$archive" -d "$dest"
            ;;
        *)
            error "Unknown archive format: $archive"
            ;;
    esac
}

# Generate strong secret
generate_secret() {
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -base64 36
    elif [ -r /dev/urandom ]; then
        head -c 36 /dev/urandom | base64 | tr -d '\n'
    else
        error "Cannot generate secure random secret. Please set COCOON_SECRET manually."
    fi
}

# Setup systemd service (Linux)
setup_systemd() {
    local install_dir="$1"
    local signaling_url="$2"
    local secret="$3"
    local setup_token="$4"

    if [ "$(id -u)" -ne 0 ]; then
        warn "Run with sudo to install systemd service"
        echo ""
        echo "To install as a service later, run:"
        printf "  ${CYAN}sudo $install_dir/$BINARY_NAME service install${NC}\n"
        return 0
    fi

    local service_file="/etc/systemd/system/cocoon.service"

    cat > "$service_file" << EOF
[Unit]
Description=Cocoon - Remote containerized worker
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$install_dir/$BINARY_NAME
Restart=always
RestartSec=5
Environment=SIGNALING_SERVER_URL=$signaling_url
Environment=COCOON_SECRET=$secret
${setup_token:+Environment=COCOON_SETUP_TOKEN=$setup_token}

[Install]
WantedBy=multi-user.target
EOF

    systemctl daemon-reload
    systemctl enable cocoon
    success "Systemd service installed and enabled"

    echo ""
    echo "Start the service with:"
    printf "  ${CYAN}sudo systemctl start cocoon${NC}\n"
}

# Setup launchd service (macOS)
setup_launchd() {
    local install_dir="$1"
    local signaling_url="$2"
    local secret="$3"
    local setup_token="$4"

    local plist_dir="$HOME/Library/LaunchAgents"
    local plist_file="$plist_dir/com.adi.cocoon.plist"

    mkdir -p "$plist_dir"

    cat > "$plist_file" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.adi.cocoon</string>
    <key>ProgramArguments</key>
    <array>
        <string>$install_dir/$BINARY_NAME</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>SIGNALING_SERVER_URL</key>
        <string>$signaling_url</string>
        <key>COCOON_SECRET</key>
        <string>$secret</string>
EOF

    if [ -n "$setup_token" ]; then
        cat >> "$plist_file" << EOF
        <key>COCOON_SETUP_TOKEN</key>
        <string>$setup_token</string>
EOF
    fi

    cat >> "$plist_file" << EOF
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/cocoon.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/cocoon.error.log</string>
</dict>
</plist>
EOF

    success "LaunchAgent plist created"

    echo ""
    echo "Load the service with:"
    printf "  ${CYAN}launchctl load $plist_file${NC}\n"
    echo ""
    echo "Or start manually:"
    printf "  ${CYAN}SIGNALING_SERVER_URL=$signaling_url COCOON_SECRET=<secret> $install_dir/$BINARY_NAME${NC}\n"
}

# Add to PATH
setup_path() {
    local install_dir="$1"
    local shell_name=""
    local rc_file=""

    if [ -n "$SHELL" ]; then
        shell_name=$(basename "$SHELL")
    fi

    case "$shell_name" in
        zsh)
            rc_file="$HOME/.zshrc"
            ;;
        bash)
            if [ -f "$HOME/.bashrc" ]; then
                rc_file="$HOME/.bashrc"
            elif [ -f "$HOME/.bash_profile" ]; then
                rc_file="$HOME/.bash_profile"
            fi
            ;;
        fish)
            rc_file="$HOME/.config/fish/config.fish"
            ;;
        *)
            rc_file="$HOME/.profile"
            ;;
    esac

    case ":$PATH:" in
        *":$install_dir:"*)
            return 0
            ;;
    esac

    echo ""
    warn "$install_dir is not in your PATH"
    echo ""
    echo "Add it by running:"
    echo ""

    case "$shell_name" in
        fish)
            printf "  ${CYAN}fish_add_path %s${NC}\n" "$install_dir"
            ;;
        *)
            printf "  ${CYAN}echo 'export PATH=\"%s:\$PATH\"' >> %s${NC}\n" "$install_dir" "$rc_file"
            ;;
    esac
}

main() {
    local setup_token="$1"

    echo ""
    printf "${MAGENTA}Cocoon Installer${NC}\n"
    echo ""

    # Detect platform
    local os=$(detect_os)
    local arch=$(detect_arch)
    local target=$(get_target "$os" "$arch")

    info "Detected platform: $target"

    # Determine version
    local version="${COCOON_VERSION:-}"
    if [ -z "$version" ]; then
        info "Fetching latest version"
        version=$(fetch_latest_version)
        if [ -z "$version" ]; then
            # Fallback if no releases yet
            warn "No releases found, using docker image instead"
            echo ""
            echo "Run Cocoon with Docker:"
            printf "  ${CYAN}docker run -e SIGNALING_SERVER_URL=$DEFAULT_SIGNALING_URL ghcr.io/adi-family/cocoon:latest${NC}\n"
            exit 0
        fi
    fi

    info "Installing version: $version"

    # Determine install directory
    local install_dir="${COCOON_INSTALL_DIR:-$HOME/.local/bin}"
    mkdir -p "$install_dir"

    info "Install directory: $install_dir"

    # Construct download URL
    local archive_name="cocoon-${version}-${target}.tar.gz"
    local download_url="https://github.com/${REPO}/releases/download/${version}/${archive_name}"
    local checksums_url="https://github.com/${REPO}/releases/download/${version}/SHA256SUMS"

    # Create temp directory
    local temp_dir=$(mktemp -d)
    trap "rm -rf '$temp_dir'" EXIT

    # Download archive
    local archive_path="$temp_dir/$archive_name"
    download "$download_url" "$archive_path"

    # Download and verify checksum
    local checksums_path="$temp_dir/SHA256SUMS"
    if download "$checksums_url" "$checksums_path" 2>/dev/null; then
        local expected_checksum=$(grep "$archive_name" "$checksums_path" | cut -d' ' -f1)
        verify_checksum "$archive_path" "$expected_checksum"
    else
        warn "Checksums file not available, skipping verification"
    fi

    # Extract
    extract "$archive_path" "$temp_dir"

    # Install binary
    local binary_path="$temp_dir/$BINARY_NAME"
    if [ ! -f "$binary_path" ]; then
        error "Binary not found in archive"
    fi

    chmod +x "$binary_path"
    mv "$binary_path" "$install_dir/$BINARY_NAME"

    success "Installed $BINARY_NAME to $install_dir/$BINARY_NAME"

    # Setup PATH
    setup_path "$install_dir"

    # Generate or use provided secret
    local secret="${COCOON_SECRET:-}"
    if [ -z "$secret" ]; then
        info "Generating secure secret"
        secret=$(generate_secret)
    fi

    local signaling_url="${SIGNALING_SERVER_URL:-$DEFAULT_SIGNALING_URL}"

    # Create config directory
    local config_dir="$HOME/.config/cocoon"
    mkdir -p "$config_dir"

    # Save secret securely
    echo "$secret" > "$config_dir/secret"
    chmod 600 "$config_dir/secret"
    success "Secret saved to $config_dir/secret"

    # Setup service based on OS
    echo ""
    case "$os" in
        linux)
            setup_systemd "$install_dir" "$signaling_url" "$secret" "$setup_token"
            ;;
        darwin)
            setup_launchd "$install_dir" "$signaling_url" "$secret" "$setup_token"
            ;;
    esac

    # Final output
    echo ""
    success "Cocoon installed successfully!"
    echo ""
    printf "  ${CYAN}Binary:${NC}    $install_dir/$BINARY_NAME\n"
    printf "  ${CYAN}Config:${NC}    $config_dir/\n"
    printf "  ${CYAN}Server:${NC}    $signaling_url\n"
    if [ -n "$setup_token" ]; then
        printf "  ${CYAN}Token:${NC}     (setup token provided)\n"
    fi
    echo ""
    echo "Quick start:"
    printf "  ${CYAN}SIGNALING_SERVER_URL=$signaling_url COCOON_SECRET=\$(cat $config_dir/secret) cocoon${NC}\n"
    echo ""
    echo "Or use Docker:"
    printf "  ${CYAN}docker run -e SIGNALING_SERVER_URL=$signaling_url -v cocoon-data:/cocoon ghcr.io/adi-family/cocoon:latest${NC}\n"
}

main "$@"
