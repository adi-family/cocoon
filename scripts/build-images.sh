#!/bin/bash
# Cocoon Docker Images Build & Push Script
# Usage: ./scripts/build-images.sh [OPTIONS]
#
# Options:
#   --push          Push images to registry after build
#   --variant NAME  Build specific variant (alpine, debian, ubuntu, python, node, full, gpu, custom)
#   --all           Build all variants (default)
#   --minimal       Build minimal variants (alpine, debian)
#   --dev           Build dev variants (ubuntu, python, node)
#   --tag TAG       Override version tag (default: latest)
#   --platform PLAT Build for specific platform (default: linux/amd64,linux/arm64)
#   --no-cache      Build without cache
#   --dry-run       Show what would be built without building
#   --help          Show this help message

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Configuration
REGISTRY="git.the-ihor.com/adi"
VERSION="latest"
PUSH=false
DRY_RUN=false
NO_CACHE=""
PLATFORMS="linux/amd64,linux/arm64"
TARGETS="default"

# Script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COCOON_DIR="$(dirname "$SCRIPT_DIR")"
REPO_ROOT="$(cd "$COCOON_DIR/../.." && pwd)"

info() {
    printf "${CYAN}[INFO]${NC} %s\n" "$1"
}

success() {
    printf "${GREEN}[OK]${NC} %s\n" "$1"
}

warn() {
    printf "${YELLOW}[WARN]${NC} %s\n" "$1"
}

error() {
    printf "${RED}[ERROR]${NC} %s\n" "$1" >&2
    exit 1
}

show_help() {
    head -20 "$0" | tail -16
    exit 0
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --push)
            PUSH=true
            shift
            ;;
        --variant)
            TARGETS="$2"
            shift 2
            ;;
        --all)
            TARGETS="default"
            shift
            ;;
        --minimal)
            TARGETS="minimal"
            shift
            ;;
        --dev)
            TARGETS="dev"
            shift
            ;;
        --tag)
            VERSION="$2"
            shift 2
            ;;
        --platform)
            PLATFORMS="$2"
            shift 2
            ;;
        --no-cache)
            NO_CACHE="--no-cache"
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --help|-h)
            show_help
            ;;
        *)
            error "Unknown option: $1"
            ;;
    esac
done

# Check prerequisites
check_prerequisites() {
    info "Checking prerequisites..."
    
    if ! command -v docker &> /dev/null; then
        error "Docker is not installed"
    fi
    
    if ! docker buildx version &> /dev/null; then
        error "Docker Buildx is not available"
    fi
    
    # Check if buildx builder exists
    if ! docker buildx inspect cocoon-builder &> /dev/null; then
        info "Creating buildx builder 'cocoon-builder'..."
        docker buildx create --name cocoon-builder --use --bootstrap
    else
        docker buildx use cocoon-builder
    fi
    
    success "Prerequisites OK"
}

# Login to registry
login_registry() {
    if [ "$PUSH" = true ]; then
        info "Checking registry authentication..."
        
        if ! docker login git.the-ihor.com &> /dev/null; then
            warn "Not logged in to git.the-ihor.com"
            echo ""
            echo "Please login with:"
            printf "  ${CYAN}docker login git.the-ihor.com${NC}\n"
            echo ""
            read -p "Press Enter after logging in, or Ctrl+C to cancel..."
        fi
        
        success "Registry authentication OK"
    fi
}

# Build images
build_images() {
    info "Building cocoon images..."
    echo ""
    printf "  ${BLUE}Registry:${NC}  $REGISTRY\n"
    printf "  ${BLUE}Version:${NC}   $VERSION\n"
    printf "  ${BLUE}Targets:${NC}   $TARGETS\n"
    printf "  ${BLUE}Platforms:${NC} $PLATFORMS\n"
    printf "  ${BLUE}Push:${NC}      $PUSH\n"
    echo ""
    
    cd "$REPO_ROOT"
    
    # Build bake command
    BAKE_CMD="docker buildx bake"
    BAKE_CMD="$BAKE_CMD -f $COCOON_DIR/docker-bake.hcl"
    BAKE_CMD="$BAKE_CMD --set *.platform=$PLATFORMS"
    BAKE_CMD="$BAKE_CMD --set REGISTRY=$REGISTRY"
    BAKE_CMD="$BAKE_CMD --set VERSION=$VERSION"
    
    if [ "$PUSH" = true ]; then
        BAKE_CMD="$BAKE_CMD --push"
    else
        BAKE_CMD="$BAKE_CMD --load"
    fi
    
    if [ -n "$NO_CACHE" ]; then
        BAKE_CMD="$BAKE_CMD --no-cache"
    fi
    
    BAKE_CMD="$BAKE_CMD $TARGETS"
    
    if [ "$DRY_RUN" = true ]; then
        info "Dry run - would execute:"
        echo ""
        printf "  ${CYAN}$BAKE_CMD${NC}\n"
        echo ""
        
        info "Would build these targets:"
        docker buildx bake -f "$COCOON_DIR/docker-bake.hcl" --print $TARGETS 2>/dev/null | jq -r '.target | keys[]' | while read target; do
            echo "  - $target"
        done
        return
    fi
    
    info "Executing: $BAKE_CMD"
    echo ""
    
    eval "$BAKE_CMD"
    
    success "Build completed!"
}

# Show results
show_results() {
    if [ "$DRY_RUN" = true ]; then
        return
    fi
    
    echo ""
    info "Built images:"
    echo ""
    
    # Parse bake file to show tags
    if [ "$PUSH" = true ]; then
        echo "  Pushed to $REGISTRY:"
    else
        echo "  Loaded locally:"
    fi
    
    case $TARGETS in
        default)
            echo "    - $REGISTRY/cocoon:alpine"
            echo "    - $REGISTRY/cocoon:debian"
            echo "    - $REGISTRY/cocoon:ubuntu (latest)"
            echo "    - $REGISTRY/cocoon:python"
            echo "    - $REGISTRY/cocoon:node"
            echo "    - $REGISTRY/cocoon:full"
            ;;
        minimal)
            echo "    - $REGISTRY/cocoon:alpine"
            echo "    - $REGISTRY/cocoon:debian"
            ;;
        dev)
            echo "    - $REGISTRY/cocoon:ubuntu"
            echo "    - $REGISTRY/cocoon:python"
            echo "    - $REGISTRY/cocoon:node"
            ;;
        *)
            echo "    - $REGISTRY/cocoon:$TARGETS"
            ;;
    esac
    
    echo ""
    
    if [ "$PUSH" = true ]; then
        success "Images pushed to $REGISTRY"
        echo ""
        echo "Pull with:"
        printf "  ${CYAN}docker pull $REGISTRY/cocoon:ubuntu${NC}\n"
    else
        echo "To push images, run:"
        printf "  ${CYAN}$0 --push${NC}\n"
    fi
}

# Main
main() {
    echo ""
    printf "${BLUE}Cocoon Docker Image Builder${NC}\n"
    echo ""
    
    check_prerequisites
    login_registry
    build_images
    show_results
    
    echo ""
    success "Done!"
}

main
