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
- Registers with unique device ID (COCOON_ID or generated UUID)
- Waits for command requests via SignalingMessage::SyncData
- PTY output streams continuously to client
- Terminal resize events sent from client to cocoon

## Environment Variables
- SIGNALING_SERVER_URL: WebSocket URL of signaling server (default: ws://localhost:8080/ws)
- COCOON_ID: Unique identifier for this cocoon instance (default: generated UUID)

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

Run with signaling server:
```bash
docker run \
  -e SIGNALING_SERVER_URL=ws://your-signaling-server:8080/ws \
  -e COCOON_ID=my-cocoon-1 \
  cocoon
```

Run with default settings (localhost):
```bash
docker run cocoon
# Uses ws://localhost:8080/ws by default
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
COCOON_ID=my-cocoon-1 \
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

  # Cocoon worker
  cocoon:
    build: ./crates/cocoon
    environment:
      - SIGNALING_SERVER_URL=ws://signaling:8080/ws
      - COCOON_ID=worker-1
    depends_on:
      - signaling
```

Then:
```bash
docker-compose up
```

## Expected Output

When cocoon starts successfully:
```
🐛 Cocoon starting
🔗 Connecting to signaling server: ws://localhost:8080/ws
🆔 Device ID: abc-123-def-456
✅ Connected and waiting for commands...
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
