cocoon, containerized-worker, signaling-server, remote-execution, websocket, pty, interactive-terminal

## Overview
- Cocoon is a containerized worker environment that connects to a signaling server
- Enables both simple command execution and interactive PTY sessions (vim, htop, claude, etc.)
- Real-time bidirectional communication via WebSocket
- Remote controls terminal size, output is synced in real-time

## CLI Commands (ADI Plugin)

The cocoon plugin provides a production-ready CLI for managing cocoon workers.

### Quick Start
```bash
# Install ADI CLI (if not already installed)
curl -fsSL https://adi.the-ihor.com/install.sh | sh

# Start cocoon in foreground (development)
adi cocoon run

# Start cocoon in Docker daemon mode (production)
adi cocoon start docker --url wss://adi.the-ihor.com/api/signaling/ws

# Install as system service (Linux/macOS)
adi cocoon service install
adi cocoon service start
```

### Commands

#### `adi cocoon run`
Start cocoon natively in foreground (development mode).

```bash
# Basic usage
adi cocoon run

# With custom signaling server
SIGNALING_SERVER_URL=wss://example.com/ws adi cocoon run
```

#### `adi cocoon start docker [OPTIONS]`
Start cocoon in Docker daemon mode (production-ready).

**Flags:**
- `--name NAME` - Container name (default: auto-generated)
- `--url URL` - Signaling server URL (default: ws://localhost:8080/ws)
- `--token TOKEN` - Setup token for auto-claim
- `--secret SECRET` - Pre-generated secret for device ID

**Examples:**
```bash
# Minimal (auto-named, localhost)
adi cocoon start docker

# With custom name
adi cocoon start docker --name my-worker

# Production (one-liner)
adi cocoon start docker \
  --name prod-worker \
  --url wss://adi.the-ihor.com/api/signaling/ws \
  --token <your-setup-token>

# Multiple workers
adi cocoon start docker --name worker-1 --url wss://example.com/ws
adi cocoon start docker --name worker-2 --url wss://example.com/ws
adi cocoon start docker --name worker-3 --url wss://example.com/ws

# Check status
docker ps --filter "name=worker-"
docker logs -f worker-1
```

**Auto-naming:** If `--name` is not specified, automatically generates names:
- First: `cocoon-worker`
- Second: `cocoon-worker-2`
- Third: `cocoon-worker-3`, etc.

**Priority order:**
1. `--name` flag (highest)
2. `COCOON_NAME` env var
3. Auto-generation (fallback)

#### `adi cocoon service [ACTION]`
Manage cocoon as a system service (systemd on Linux, launchd on macOS).

**Actions:**
- `install` - Install systemd/launchd service
- `uninstall` - Remove service
- `start` - Start service
- `stop` - Stop service
- `restart` - Restart service
- `status` - Show service status
- `logs` - Follow service logs

**Examples:**
```bash
# Install service (reads SIGNALING_SERVER_URL from env)
SIGNALING_SERVER_URL=wss://adi.the-ihor.com/api/signaling/ws \
adi cocoon service install

# Start service
adi cocoon service start

# Check status
adi cocoon service status

# View logs (follow mode)
adi cocoon service logs

# Restart service
adi cocoon service restart

# Uninstall service
adi cocoon service stop
adi cocoon service uninstall
```

**Service files created:**
- Linux: `~/.config/systemd/user/cocoon.service`
- macOS: `~/Library/LaunchAgents/com.adi.cocoon.plist`
- Secret: `~/.config/cocoon/secret`

**Service features:**
- Auto-start on login/boot
- Auto-restart on crash
- Persistent across terminal sessions
- Runs in background

## Getting Started - Choose Your Setup

### 1. Your Own Machine (Development/Personal Use)

**Best for**: Development, testing, personal projects

#### Option A: Quick Start with ADI Plugin (Recommended)
```bash
# One-command install
curl -fsSL https://adi.the-ihor.com/install.sh | sh

# Start natively in foreground (development)
adi cocoon run

# Or start in Docker daemon mode (production)
adi cocoon start docker \
  --url wss://adi.the-ihor.com/api/signaling/ws \
  --token <your-setup-token>
```

**Pros**: Easy to install, integrates with ADI ecosystem, production-ready
**Cons**: Requires ADI CLI installation

#### Option B: System Service (Linux/macOS)
```bash
# Install ADI CLI
curl -fsSL https://adi.the-ihor.com/install.sh | sh

# Install and start service
SIGNALING_SERVER_URL=wss://adi.the-ihor.com/api/signaling/ws \
adi cocoon service install

adi cocoon service start

# Check status
adi cocoon service status
```

**Pros**: Auto-start on boot, runs in background, production-ready
**Cons**: Requires ADI CLI installation

#### Option C: Docker Only (No ADI Installation)
```bash
# Production daemon mode (auto-restart, persistent)
docker run -d --restart unless-stopped \
  --name cocoon-worker \
  -e SIGNALING_SERVER_URL=wss://adi.the-ihor.com/api/signaling/ws \
  -e COCOON_SETUP_TOKEN=<your-token> \
  -v cocoon-data:/cocoon \
  git.the-ihor.com/adi/cocoon:latest

# Check logs
docker logs -f cocoon-worker
```

**Pros**: No ADI installation needed, isolated environment
**Cons**: Manual Docker management required

### 2. Remote Server/VPS (DigitalOcean, AWS EC2, Hetzner, etc.)

**Best for**: Always-on workers, production deployments, cloud computing

#### Option A: ADI CLI with Docker (Recommended)
```bash
# SSH into your server
ssh user@your-server.com

# Install ADI CLI
curl -fsSL https://adi.the-ihor.com/install.sh | sh

# Install Docker
curl -fsSL https://get.docker.com | sh

# Start cocoon in Docker daemon mode
adi cocoon start docker \
  --name prod-worker \
  --url wss://adi.the-ihor.com/api/signaling/ws \
  --token <your-setup-token>

# Check logs
docker logs -f prod-worker
```

#### Option B: System Service (Linux)
```bash
# SSH into your server
ssh user@your-server.com

# Install ADI CLI
curl -fsSL https://adi.the-ihor.com/install.sh | sh

# Install and start service
SIGNALING_SERVER_URL=wss://adi.the-ihor.com/api/signaling/ws \
COCOON_SETUP_TOKEN=<your-token> \
adi cocoon service install

adi cocoon service start

# Check status
adi cocoon service status
```

#### Option C: Direct Docker (No ADI CLI)
```bash
# Install Docker
curl -fsSL https://get.docker.com | sh

# Start cocoon with auto-restart
docker run -d --restart unless-stopped \
  --name cocoon-worker \
  -e SIGNALING_SERVER_URL=wss://adi.the-ihor.com/api/signaling/ws \
  -e COCOON_SETUP_TOKEN=<your-token> \
  -v cocoon-data:/cocoon \
  git.the-ihor.com/adi/cocoon:latest

# Check logs
docker logs -f cocoon-worker
```

**Pros**: Always running, automatic restarts, isolated
**Cons**: Costs money for server rental

**Cost Reference**:
- DigitalOcean Droplet: $6-12/month (1-2GB RAM)
- AWS EC2 t3.micro: ~$7.50/month
- Hetzner Cloud CX11: €4.15/month (~$4.50)

### 3. Cloud Container Services (Serverless)

**Best for**: Auto-scaling, pay-per-use, minimal management

#### AWS ECS/Fargate
```bash
# Use image: git.the-ihor.com/adi/cocoon:latest
# Set environment variables:
SIGNALING_SERVER_URL=wss://adi.the-ihor.com/api/signaling/ws
COCOON_SETUP_TOKEN=<your-token>

# Mount EFS volume at /cocoon for persistence
```

#### Google Cloud Run
```bash
gcloud run deploy cocoon-worker \\
  --image git.the-ihor.com/adi/cocoon:latest \\
  --set-env-vars SIGNALING_SERVER_URL=wss://adi.the-ihor.com/api/signaling/ws \\
  --set-env-vars COCOON_SETUP_TOKEN=<your-token> \\
  --min-instances 1
```

**Pros**: Scales automatically, managed infrastructure
**Cons**: More complex setup, may be more expensive at scale

### 4. Rented Compute (Vast.ai, RunPod, Lambda Labs)

**Best for**: GPU workloads, temporary high-performance computing

#### Vast.ai Example
```bash
# In container startup script:
docker run -d --restart unless-stopped \\
  --gpus all \\
  -e SIGNALING_SERVER_URL=wss://adi.the-ihor.com/api/signaling/ws \\
  -e COCOON_SETUP_TOKEN=<your-token> \\
  -v /workspace/cocoon-data:/cocoon \\
  git.the-ihor.com/adi/cocoon:latest
```

**Pros**: Access to GPUs, pay-per-hour pricing
**Cons**: Less reliable, may lose data on termination

## Quick Decision Guide

**Choose `adi cocoon run` (Native Foreground)** if:
- ✅ You're developing/testing locally
- ✅ You want to see logs in real-time
- ✅ You want maximum performance
- ✅ You need to stop it easily (Ctrl+C)

**Choose `adi cocoon start docker` (Docker Daemon)** if:
- ✅ You want production-ready deployment
- ✅ You want isolation from your system
- ✅ You're running on a remote server
- ✅ You need auto-restart on failure
- ✅ You want to run multiple instances easily

**Choose `adi cocoon service` (System Service)** if:
- ✅ You want auto-start on boot
- ✅ You prefer native execution (no Docker)
- ✅ You're on Linux or macOS
- ✅ You want system integration (systemd/launchd)

**Choose Cloud Container Service** if:
- ✅ You need auto-scaling
- ✅ You want managed infrastructure
- ✅ You have existing cloud deployments

**Choose Rented Compute** if:
- ✅ You need GPU access
- ✅ You want temporary high-performance computing
- ✅ Cost is more important than reliability

## Multiple Cocoons

Run multiple cocoon workers with unique names:

```bash
# Using ADI CLI (auto-naming)
adi cocoon start docker --url wss://adi.the-ihor.com/api/signaling/ws
adi cocoon start docker --url wss://adi.the-ihor.com/api/signaling/ws
adi cocoon start docker --url wss://adi.the-ihor.com/api/signaling/ws
# Creates: cocoon-worker, cocoon-worker-2, cocoon-worker-3

# Using ADI CLI (custom names)
adi cocoon start docker --name dev-worker --url wss://example.com/ws
adi cocoon start docker --name staging-worker --url wss://example.com/ws
adi cocoon start docker --name prod-worker --url wss://example.com/ws

# Manage multiple workers
docker ps --filter "name=worker-"
docker logs -f dev-worker
docker stop staging-worker
docker restart prod-worker
```

## Capabilities

### 1. Simple Command Execution
- Execute non-interactive commands (scripts, builds, etc.)
- Capture stdout, stderr, exit code
- Collect output files from /cocoon/output

### 2. Interactive PTY Sessions
- Full pseudo-terminal support for TUI applications
- Supports vim, htop, tmux, claude, and any terminal-based tool
- Real-time output streaming with ANSI escape codes
- Remote-controlled terminal resize (client dictates size)
- Multiple concurrent PTY sessions
- TERM=xterm-256color for full color support

### 3. HTTP Service Proxy (NEW - Phase 2)
- Proxy HTTP requests to local services running on cocoon
- Access local APIs, databases, or any HTTP service via signaling server
- 30-second timeout for proxy requests
- Full HTTP method support (GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS)
- Headers and body forwarding

**Use cases:**
- Access FlowMap API running on remote cocoon
- Query local Postgres/MySQL databases
- Call local microservices
- Access development servers (Next.js, Vite, etc.)

**Service Configuration:**
```bash
# Via environment variable
COCOON_SERVICES="flowmap-api:8092,postgres:5432,redis:6379" cocoon

# Via Docker
docker run -e COCOON_SERVICES="flowmap-api:8092" cocoon
```

**Example Proxy Request:**
```json
{
  "type": "proxy_http",
  "request_id": "req-123",
  "service_name": "flowmap-api",
  "method": "GET",
  "path": "/api/parse?path=/project",
  "headers": {"Accept": "application/json"},
  "body": null
}
```

**Example Proxy Response:**
```json
{
  "type": "proxy_result",
  "request_id": "req-123",
  "status_code": 200,
  "headers": {"content-type": "application/json"},
  "body": "{\"flows\": [...]}"
}
```

### 4. Local Query Aggregation (NEW - Phase 2)
- Query local data stores for multi-device aggregation
- Respond to queries from signaling server
- Support for multiple query types

**Supported Query Types:**
- `ListTasks` - List all tasks from local task store
- `GetTaskStats` - Get task statistics (pending, running, completed, failed)
- `SearchTasks` - Search tasks by query string
- `SearchKnowledgebase` - Search local knowledgebase
- `Custom { query_name }` - Custom query handlers

**Example Query Request:**
```json
{
  "type": "query_local",
  "query_id": "query-456",
  "query_type": "ListTasks",
  "params": {"status": "running", "limit": 10}
}
```

**Example Query Response:**
```json
{
  "type": "query_result",
  "query_id": "query-456",
  "data": {
    "tasks": [],
    "total": 0,
    "source": "cocoon-local"
  },
  "is_final": true
}
```

**Note:** Query handlers currently return empty results. Integration with lib-task-store will be added in a future update.

## Architecture
- Connects to signaling server on startup via WebSocket
- **Secure persistent sessions**: Client secret → HMAC-SHA256 → Device ID
- Same secret always produces same device ID (enables session persistence)
- Server never stores secrets, only derives IDs
- Waits for command requests via SignalingMessage::SyncData
- PTY output streams continuously to client
- Terminal resize events sent from client to cocoon

## Security & Persistent Sessions

### How It Works
1. **Cocoon generates/loads secret**: Strong secret stored in `/cocoon/.secret` or `COCOON_SECRET` env var
2. **First registration**: Sends `Register { secret, device_id: None }` to server
3. **Server derives device ID**: `HMAC-SHA256(secret, salt)` → deterministic device ID
4. **Cocoon saves device ID**: Stores in `/cocoon/.device_id` for verification
5. **Reconnection**: Sends `Register { secret, device_id: Some(saved_id) }` to server
6. **Server verifies**: Checks that `saved_id == HMAC-SHA256(secret, salt)`
7. **Security**: If mismatch → rejects connection (prevents stolen secret attacks)

### Secret Strength Requirements
**CRITICAL**: Secrets MUST be cryptographically strong. Both client and server enforce this.

- **Minimum length**: 32 characters (server rejects shorter secrets)
- **Auto-generated secrets**: 48 characters with 288 bits of entropy
- **Character variety**: Must have at least 10 unique characters
- **Rejected patterns**:
  - Only numbers (e.g., "12345678901234567890123456789012")
  - Only lowercase letters
  - Repetitive characters (e.g., "aaaaaaaa...")
  - Weak patterns: "password", "secret", "admin", "12345", "qwerty", "test", "example"

**Generate strong secret manually**:
```bash
openssl rand -base64 36
# Produces: e.g., "kX9mP2vR8nQ4sT6wY1zC3hF5jL7dN0bM9pK8gV4aS2="
```

**What happens with weak secrets**:
- Client with `COCOON_SECRET`: Validates on startup, exits if weak
- Client with file secret: Regenerates if weak, saves new strong secret
- Server: Rejects registration with error message about weak secret

### Device ID Verification (Anti-Theft Protection)
**Why device_id verification matters**:
- Without it: Attacker steals secret → registers new device → gets same device_id → intercepts traffic
- With it: Attacker steals secret → tries to register → server rejects (device_id doesn't match)

**How it works**:
- First connection: `device_id = None` → server assigns ID → cocoon saves to `/cocoon/.device_id`
- Reconnection: `device_id = Some(saved)` → server verifies `saved == HMAC(secret)` → rejects if mismatch
- Result: Even if secret is stolen, attacker can't impersonate the original device

**Files created**:
- `/cocoon/.secret` - Cryptographically strong secret (48 chars)
- `/cocoon/.device_id` - Server-assigned device ID (HMAC-derived from secret)
- Both must be stolen together to impersonate a device (harder attack)

### Secret Storage Options
- **File (persistent)**: `/cocoon/.secret` - mount volume for persistence
- **Environment variable**: `COCOON_SECRET` - for manual management
- **Ephemeral**: Generated on each start if no file/env (new device ID each time)

### Server HMAC Salt
- **Environment variable**: `HMAC_SALT` on signaling server
- **Persistence**: Set same salt across server restarts to maintain device ID mapping
- **Security**: Keep salt secret, never expose publicly

## Environment Variables

### Cocoon (CLI Flags Preferred)

When using `adi cocoon` CLI, prefer flags over environment variables:
- Use `--url` instead of `SIGNALING_SERVER_URL`
- Use `--token` instead of `COCOON_SETUP_TOKEN`
- Use `--secret` instead of `COCOON_SECRET`
- Use `--name` instead of `COCOON_NAME`

**Priority:** CLI flags > Environment variables > Defaults

**Environment variables (fallback):**
- `SIGNALING_SERVER_URL`: WebSocket URL (default: `ws://localhost:8080/ws`)
- `COCOON_SECRET`: Optional secret for persistent device ID (otherwise uses `/cocoon/.secret`)
- `COCOON_SETUP_TOKEN`: Setup token for auto-claim
- `COCOON_NAME`: Container name for Docker mode
- `COCOON_SERVICES`: Service registry (format: `"service1:port1,service2:port2"`)
  - Example: `"flowmap-api:8092,postgres:5432,redis:6379"`
- `RUST_LOG`: Log level for debugging (e.g., `cocoon=debug`)

### Signaling Server
- `HMAC_SALT`: Salt for device ID derivation (set for persistent device IDs across restarts)
- `PORT`: Server port (default: 8080)

### WebRTC Configuration (Optional)
WebRTC is used for low-latency peer-to-peer communication. If WebRTC connections fail after ~30 seconds, this usually indicates ICE connectivity issues.

**Environment Variables:**
- `WEBRTC_ICE_SERVERS`: Comma-separated list of STUN/TURN server URLs
  - Default: `stun:stun.l.google.com:19302` (Google's public STUN)
  - Example: `stun:stun.l.google.com:19302,turn:turn.example.com:3478`
- `WEBRTC_TURN_USERNAME`: Username for TURN server authentication
- `WEBRTC_TURN_CREDENTIAL`: Credential/password for TURN server authentication

**When to configure TURN:**
- Both peers are behind symmetric NAT (most corporate/cloud networks)
- STUN-only connections consistently fail
- Need guaranteed connectivity through firewalls

**Example with Coturn TURN server:**
```bash
docker run -d \
  -e SIGNALING_SERVER_URL=wss://adi.the-ihor.com/api/signaling/ws \
  -e WEBRTC_ICE_SERVERS="stun:stun.l.google.com:19302,turn:your-turn-server.com:3478" \
  -e WEBRTC_TURN_USERNAME="turnuser" \
  -e WEBRTC_TURN_CREDENTIAL="turnpassword" \
  -v cocoon-data:/cocoon \
  git.the-ihor.com/adi/cocoon:latest
```

**Free TURN servers (for testing only):**
- OpenRelay: `turn:openrelay.metered.ca:80` (requires registration)
- Twilio: Requires account, provides TURN-as-a-service

**Self-hosted TURN (recommended for production):**
- [Coturn](https://github.com/coturn/coturn) - Most popular open-source TURN server
- Deploy on a VPS with public IP for best results

## Command Protocol

### Execute (Simple Command)
```json
{"type": "execute", "command": "ls -la", "input": "optional stdin"}
```
Response: `{"type": "execute_result", "success": true, "data": {...}, "files": [...]}`

### AttachPty (Interactive Terminal)
```json
{"type": "attach_pty", "command": "vim test.txt", "cols": 80, "rows": 24, "env": {}}
```
Response: `{"type": "pty_created", "session_id": "uuid"}`
Then continuous: `{"type": "pty_output", "session_id": "uuid", "data": "...ANSI..."}`

### PtyInput (Send Keystrokes)
```json
{"type": "pty_input", "session_id": "uuid", "data": "\x1b[A"}
```

### PtyResize (Remote Controls Size)
```json
{"type": "pty_resize", "session_id": "uuid", "cols": 100, "rows": 30}
```

### PtyClose (Terminate Session)
```json
{"type": "pty_close", "session_id": "uuid"}
```
Response: `{"type": "pty_exited", "session_id": "uuid", "exit_code": 0}`

## Getting Started

### Docker (Recommended)

Build the image:
```bash
cd crates/cocoon
docker build -t cocoon .
```

Run with signaling server (ephemeral - new device ID each restart):
```bash
docker run \
  -e SIGNALING_SERVER_URL=ws://your-signaling-server:8080/ws \
  cocoon
```

Run with persistent device ID (mount volume):
```bash
docker run \
  -e SIGNALING_SERVER_URL=ws://your-signaling-server:8080/ws \
  -v cocoon-data:/cocoon \
  cocoon
# Secret saved to /cocoon/.secret, same device ID on restart
```

Run with manual secret (persistent):
```bash
# Generate strong secret first: openssl rand -base64 36
docker run \
  -e SIGNALING_SERVER_URL=ws://your-signaling-server:8080/ws \
  -e COCOON_SECRET="kX9mP2vR8nQ4sT6wY1zC3hF5jL7dN0bM9pK8gV4aS2=" \
  cocoon
# Always gets same device ID from this secret (must be 32+ chars)
```

Run with default settings (localhost, ephemeral):
```bash
docker run cocoon
```

### Build from Source

Build:
```bash
cd crates/cocoon
cargo build --release
```

Run:
```bash
SIGNALING_SERVER_URL=ws://your-signaling-server:8080/ws \
./target/release/cocoon
```

### Docker Compose (Full Stack)

Create `docker-compose.yml`:
```yaml
version: '3.8'

services:
  # Signaling server
  signaling:
    image: ghcr.io/adi-family/signaling-server:latest
    ports:
      - "8080:8080"
    environment:
      - PORT=8080

  # Cocoon worker (with persistent sessions)
  cocoon:
    build: ./crates/cocoon
    environment:
      - SIGNALING_SERVER_URL=ws://signaling:8080/ws
      - HMAC_SALT=your-server-salt-here  # Set on signaling server
    volumes:
      - cocoon-data:/cocoon  # Persist secret for same device ID
    depends_on:
      - signaling

volumes:
  cocoon-data:
```

Then:
```bash
docker-compose up
```

## Expected Output

When cocoon starts successfully (first time):
```
🐛 Cocoon starting
🆕 Generated new strong secret (48 characters, 288 bits entropy)
💾 Saved secret to /cocoon/.secret for persistent sessions
🔗 Connecting to signaling server: ws://localhost:8080/ws
⏳ Waiting for derived device ID (first registration)...
✅ Registration confirmed
🆔 Device ID: a1b2c3d4e5f6... (HMAC-derived from secret)
💾 Saved device ID to /cocoon/.device_id for reconnection verification
```

When cocoon restarts (with existing secret + device_id):
```
🐛 Cocoon starting
🔑 Loaded existing secret from /cocoon/.secret
📱 Loaded existing device ID from /cocoon/.device_id
🔗 Connecting to signaling server: ws://localhost:8080/ws
⏳ Reconnecting with device ID verification...
✅ Registration confirmed
🆔 Device ID: a1b2c3d4e5f6... (verified - secret matches!)
```

When attacker tries with stolen secret (but wrong device_id):
```
Server rejects with: "Registration rejected - device_id does not match secret. Possible stolen secret attack."
```

## Quick Test

1. Start signaling server:
```bash
cd crates/signaling-server
cargo run
```

2. Start cocoon (in another terminal):
```bash
cd crates/cocoon
cargo run
```

3. Send a test command via WebSocket client:
```json
{
  "type": "sync_data",
  "payload": {
    "type": "execute",
    "command": "echo 'Hello from cocoon!'",
    "input": null
  }
}
```

## Docker Image Variants

Cocoon provides multiple Docker image variants for different use cases. Build with Docker Bake:

```bash
# Build all variants
docker buildx bake

# Build specific variant
docker buildx bake alpine
docker buildx bake ubuntu

# Build minimal set
docker buildx bake minimal

# Build dev set
docker buildx bake dev
```

### Available Variants

| Image | Base | Size | Use Case | Key Tools |
|-------|------|------|----------|-----------|
| `cocoon:alpine` | Alpine 3.20 | ~15MB | Production, minimal | bash, curl, git, jq |
| `cocoon:debian` | Debian Bookworm | ~100MB | Balanced dev | build-essential, python3, vim, ssh |
| `cocoon:ubuntu` | Ubuntu 24.04 | ~150MB | Full dev (default) | nodejs, clang, cmake, sudo, zsh |
| `cocoon:python` | Python 3.12 | ~180MB | Python/ML | pip, poetry, uv, pytest, jupyter |
| `cocoon:node` | Node.js 22 | ~200MB | JS/TS dev | npm, yarn, pnpm, bun, typescript |
| `cocoon:full` | Ubuntu 24.04 | ~500MB | Everything | rust, go, docker-cli, kubectl, terraform |
| `cocoon:gpu` | CUDA 12.4 | ~2GB | GPU/ML workloads | cuda, cudnn, pytorch-ready |
| `cocoon:custom` | Configurable | Varies | Your own setup | User-defined |

### Tag Aliases

- `cocoon:latest` → `cocoon:ubuntu`
- `cocoon:minimal` → `cocoon:alpine`
- `cocoon:slim` → `cocoon:debian`
- `cocoon:py` → `cocoon:python`
- `cocoon:js` → `cocoon:node`
- `cocoon:all` → `cocoon:full`
- `cocoon:cuda` → `cocoon:gpu`

### Custom Image

Build your own cocoon with custom base and packages:

```bash
# Via Docker Bake
docker buildx bake custom \
  --set custom.args.CUSTOM_BASE=debian:bookworm \
  --set custom.args.CUSTOM_PACKAGES="vim htop neovim postgresql-client" \
  --set custom.args.CUSTOM_SETUP_SCRIPT="curl -fsSL https://example.com/setup.sh | sh"

# Or direct build
docker build -f images/Dockerfile.custom \
  --build-arg CUSTOM_BASE=fedora:40 \
  --build-arg CUSTOM_PACKAGES="nodejs npm rust cargo" \
  -t my-cocoon .
```

**Supported base images:**
- Debian/Ubuntu (apt-get)
- Alpine (apk)
- Fedora/RHEL (dnf)
- CentOS (yum)
- Arch Linux (pacman)
- openSUSE (zypper)

### GPU Image

For CUDA workloads with NVIDIA GPUs:

```bash
# Build
docker buildx bake gpu

# Run with GPU access
docker run --gpus all \
  -e SIGNALING_SERVER_URL=wss://adi.the-ihor.com/api/signaling/ws \
  -v cocoon-data:/cocoon \
  git.the-ihor.com/adi/cocoon:gpu

# Run on Vast.ai / RunPod
docker run --gpus all \
  -e SIGNALING_SERVER_URL=wss://adi.the-ihor.com/api/signaling/ws \
  -e COCOON_SETUP_TOKEN=<token> \
  -v /workspace/models:/cocoon/models \
  git.the-ihor.com/adi/cocoon:gpu
```

**Requirements:**
- NVIDIA GPU with CUDA support
- NVIDIA Container Toolkit on host
- Linux host (no macOS/Windows Docker Desktop GPU support)

### Multi-Platform Support

All images (except GPU) support both architectures:
- `linux/amd64` (x86_64)
- `linux/arm64` (Apple Silicon, AWS Graviton)

### Image Selection Guide

**Choose `alpine`** if:
- Deploying to production with minimal attack surface
- Running on resource-constrained environments
- Only need basic shell commands

**Choose `debian`** if:
- Need build tools without full dev environment
- Want Python without extra ML libraries
- Balanced size vs. functionality

**Choose `ubuntu`** (default) if:
- General development work
- Need Node.js + Python together
- Want `sudo` access and common dev tools

**Choose `python`** if:
- Python-centric development
- Machine learning (CPU)
- Data science workflows

**Choose `node`** if:
- JavaScript/TypeScript development
- Frontend tooling
- Need multiple package managers (npm, yarn, pnpm, bun)

**Choose `full`** if:
- Multi-language polyglot development
- Need cloud tools (kubectl, terraform, gh)
- CI/CD workloads
- Don't care about image size

**Choose `gpu`** if:
- GPU-accelerated ML inference
- CUDA development
- Running on cloud GPU instances (Vast.ai, RunPod, Lambda Labs)

**Choose `custom`** if:
- Need specific packages not in other variants
- Using a different Linux distro
- Have company-specific tooling requirements

## Name Origin
- "Cocoon" represents a protected, isolated environment where transformation happens
- The container wraps around the execution environment like a chrysalis
- Just as a cocoon enables metamorphosis, this enables code execution in isolation
