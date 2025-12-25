cocoon, containerized-worker, signaling-server, remote-execution, websocket

## Overview
- Cocoon is a containerized worker environment that connects to a signaling server
- Enables remote command execution inside Docker containers via WebSocket
- Replaces the old adi-worker's file-based execution model with real-time bidirectional communication

## Architecture
- Connects to signaling server on startup via WebSocket
- Registers with a unique device ID (COCOON_ID env var or generated UUID)
- Waits for command requests via SignalingMessage::SyncData
- Executes commands and sends responses back through signaling server
- Collects output files from /cocoon/output directory

## Environment Variables
- SIGNALING_SERVER_URL: WebSocket URL of signaling server (default: ws://localhost:8080/ws)
- COCOON_ID: Unique identifier for this cocoon instance (default: generated UUID)

## Command Protocol
- Request format: `{"type": "execute", "command": "...", "input": "..."}`
- Response includes: success, stdout, stderr, exit_code, output files
- Files in /cocoon/output are automatically collected and base64-encoded if binary

## Usage
```bash
docker build -t cocoon .
docker run -e SIGNALING_SERVER_URL=ws://server:8080/ws cocoon
```

## Name Origin
- "Cocoon" represents a protected, isolated environment where transformation happens
- The container wraps around the execution environment like a chrysalis
- Just as a cocoon enables metamorphosis, this enables code execution in isolation
