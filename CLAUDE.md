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

## Usage
```bash
docker build -t cocoon .
docker run -e SIGNALING_SERVER_URL=ws://server:8080/ws cocoon
```

## Name Origin
- "Cocoon" represents a protected, isolated environment where transformation happens
- The container wraps around the execution environment like a chrysalis
- Just as a cocoon enables metamorphosis, this enables code execution in isolation
