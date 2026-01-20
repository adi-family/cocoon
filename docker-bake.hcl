// Cocoon Docker Bake Configuration
// Build all variants: docker buildx bake
// Build specific: docker buildx bake alpine
// Build custom: docker buildx bake custom --set custom.args.CUSTOM_PACKAGES="vim htop"

variable "REGISTRY" {
  default = "git.the-ihor.com/adi"
}

variable "VERSION" {
  default = "latest"
}

// Custom packages for custom variant
variable "CUSTOM_BASE" {
  default = "ubuntu:24.04"
}

variable "CUSTOM_PACKAGES" {
  default = ""
}

variable "CUSTOM_SETUP_SCRIPT" {
  default = ""
}

// Shared build context
group "default" {
  targets = ["alpine", "debian", "ubuntu", "python", "node", "full"]
}

group "minimal" {
  targets = ["alpine", "debian"]
}

group "dev" {
  targets = ["ubuntu", "python", "node"]
}

// Base target with shared settings
target "_base" {
  context = "."
  dockerfile = "Dockerfile"
  platforms = ["linux/amd64", "linux/arm64"]
  labels = {
    "org.opencontainers.image.source" = "https://github.com/adi-family/cocoon"
    "org.opencontainers.image.description" = "Cocoon - Containerized worker environment"
    "org.opencontainers.image.licenses" = "BSL-1.0"
  }
}

// =============================================================================
// VARIANT: Alpine (Minimal)
// Size: ~15MB | Use: Production, minimal footprint
// =============================================================================
target "alpine" {
  inherits = ["_base"]
  dockerfile = "images/Dockerfile.alpine"
  tags = [
    "${REGISTRY}/cocoon:alpine",
    "${REGISTRY}/cocoon:alpine-${VERSION}",
    "${REGISTRY}/cocoon:minimal",
  ]
  labels = {
    "cocoon.variant" = "alpine"
    "cocoon.size" = "minimal"
  }
}

// =============================================================================
// VARIANT: Debian (Slim)
// Size: ~100MB | Use: Balanced dev environment
// =============================================================================
target "debian" {
  inherits = ["_base"]
  dockerfile = "images/Dockerfile.debian"
  tags = [
    "${REGISTRY}/cocoon:debian",
    "${REGISTRY}/cocoon:debian-${VERSION}",
    "${REGISTRY}/cocoon:slim",
  ]
  labels = {
    "cocoon.variant" = "debian"
    "cocoon.size" = "slim"
  }
}

// =============================================================================
// VARIANT: Ubuntu (Standard)
// Size: ~150MB | Use: Full development environment
// =============================================================================
target "ubuntu" {
  inherits = ["_base"]
  dockerfile = "images/Dockerfile.ubuntu"
  tags = [
    "${REGISTRY}/cocoon:ubuntu",
    "${REGISTRY}/cocoon:ubuntu-${VERSION}",
    "${REGISTRY}/cocoon:latest",
  ]
  labels = {
    "cocoon.variant" = "ubuntu"
    "cocoon.size" = "standard"
  }
}

// =============================================================================
// VARIANT: Python
// Size: ~180MB | Use: Python development and ML workloads
// =============================================================================
target "python" {
  inherits = ["_base"]
  dockerfile = "images/Dockerfile.python"
  tags = [
    "${REGISTRY}/cocoon:python",
    "${REGISTRY}/cocoon:python-${VERSION}",
    "${REGISTRY}/cocoon:py",
  ]
  labels = {
    "cocoon.variant" = "python"
    "cocoon.size" = "medium"
  }
}

// =============================================================================
// VARIANT: Node.js
// Size: ~200MB | Use: Node.js/TypeScript development
// =============================================================================
target "node" {
  inherits = ["_base"]
  dockerfile = "images/Dockerfile.node"
  tags = [
    "${REGISTRY}/cocoon:node",
    "${REGISTRY}/cocoon:node-${VERSION}",
    "${REGISTRY}/cocoon:js",
  ]
  labels = {
    "cocoon.variant" = "node"
    "cocoon.size" = "medium"
  }
}

// =============================================================================
// VARIANT: Full (Everything)
// Size: ~500MB | Use: Complete multi-language environment
// =============================================================================
target "full" {
  inherits = ["_base"]
  dockerfile = "images/Dockerfile.full"
  tags = [
    "${REGISTRY}/cocoon:full",
    "${REGISTRY}/cocoon:full-${VERSION}",
    "${REGISTRY}/cocoon:all",
  ]
  labels = {
    "cocoon.variant" = "full"
    "cocoon.size" = "large"
  }
}

// =============================================================================
// VARIANT: Custom (User-configurable)
// Build with: docker buildx bake custom \
//   --set custom.args.CUSTOM_BASE=debian:bookworm \
//   --set custom.args.CUSTOM_PACKAGES="vim htop neovim" \
//   --set custom.args.CUSTOM_SETUP_SCRIPT="curl -fsSL https://example.com/setup.sh | sh"
// =============================================================================
target "custom" {
  inherits = ["_base"]
  dockerfile = "images/Dockerfile.custom"
  tags = [
    "${REGISTRY}/cocoon:custom",
    "${REGISTRY}/cocoon:custom-${VERSION}",
  ]
  args = {
    CUSTOM_BASE = CUSTOM_BASE
    CUSTOM_PACKAGES = CUSTOM_PACKAGES
    CUSTOM_SETUP_SCRIPT = CUSTOM_SETUP_SCRIPT
  }
  labels = {
    "cocoon.variant" = "custom"
    "cocoon.base" = CUSTOM_BASE
  }
}

// =============================================================================
// VARIANT: GPU (CUDA-enabled)
// Size: ~2GB | Use: GPU workloads, ML inference
// =============================================================================
target "gpu" {
  inherits = ["_base"]
  dockerfile = "images/Dockerfile.gpu"
  platforms = ["linux/amd64"]  // GPU only on amd64
  tags = [
    "${REGISTRY}/cocoon:gpu",
    "${REGISTRY}/cocoon:gpu-${VERSION}",
    "${REGISTRY}/cocoon:cuda",
  ]
  labels = {
    "cocoon.variant" = "gpu"
    "cocoon.size" = "xlarge"
  }
}
