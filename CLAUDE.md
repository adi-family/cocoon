cocoon, containerized-worker, signaling-server, remote-execution, websocket, pty, interactive-terminal

## Overview
- Cocoon is a containerized worker environment that connects to a signaling server
- Enables both simple command execution and interactive PTY sessions (vim, htop, claude, etc.)
- Real-time bidirectional communication via WebSocket
- Remote controls terminal size, output is synced in real-time

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
2. **Sends secret to server**: During registration
3. **Server derives device ID**: `HMAC-SHA256(secret, salt)` → deterministic device ID
4. **Persistent sessions**: Same secret = same device ID across restarts

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

### Secret Storage Options
- **File (persistent)**: `/cocoon/.secret` - mount volume for persistence
- **Environment variable**: `COCOON_SECRET` - for manual management
- **Ephemeral**: Generated on each start if no file/env (new device ID each time)

### Server HMAC Salt
- **Environment variable**: `HMAC_SALT` on signaling server
- **Persistence**: Set same salt across server restarts to maintain device ID mapping
- **Security**: Keep salt secret, never expose publicly

## Environment Variables

### Cocoon
- `SIGNALING_SERVER_URL`: WebSocket URL (default: `ws://localhost:8080/ws`)
- `COCOON_SECRET`: Optional secret for persistent device ID (otherwise uses `/cocoon/.secret`)
- `RUST_LOG`: Log level for debugging (e.g., `cocoon=debug`)

### Signaling Server
- `HMAC_SALT`: Salt for device ID derivation (set for persistent device IDs across restarts)
- `PORT`: Server port (default: 8080)

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
    image: ghcr.io/adi-family/tarminal-signaling-server:latest
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

When cocoon starts successfully (with new secret):
```
🐛 Cocoon starting
🆕 Generated new strong secret (48 characters, 288 bits entropy)
💾 Saved secret to /cocoon/.secret for persistent sessions
🔗 Connecting to signaling server: ws://localhost:8080/ws
⏳ Waiting for derived device ID (persistent session)...
✅ Registration confirmed
🆔 Assigned device ID: a1b2c3d4e5f6... (HMAC-derived from secret)
```

When cocoon restarts (with existing secret):
```
🐛 Cocoon starting
🔑 Loaded existing secret from /cocoon/.secret
🔗 Connecting to signaling server: ws://localhost:8080/ws
⏳ Waiting for derived device ID (persistent session)...
✅ Registration confirmed
🆔 Assigned device ID: a1b2c3d4e5f6... (same ID as before!)
```

## Quick Test

1. Start signaling server:
```bash
cd crates/tarminal-signaling-server
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

## Name Origin
- "Cocoon" represents a protected, isolated environment where transformation happens
- The container wraps around the execution environment like a chrysalis
- Just as a cocoon enables metamorphosis, this enables code execution in isolation
