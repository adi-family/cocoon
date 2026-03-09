// Cocoon Protocol — types shared between cocoon-core (Rust) and cocoon-web (TypeScript)
// Source of truth for code generation via lib-typespec-api protocol codegen.
//
// These types travel inside signaling SyncData payloads.
// Regenerate: build.rs runs automatically on `cargo build`

// ── Supporting Types ────────────────────────────────────────

enum SilkStream {
    stdout: "stdout",
    stderr: "stderr",
}

enum SilkSignal {
    interrupt: "interrupt",
    terminate: "terminate",
    kill: "kill",
}

model SilkHtmlSpan {
    text: string;
    classes?: string[];
    styles?: Record<string>;
}

enum QueryType {
    listTasks: "list_tasks",
    getTaskStats: "get_task_stats",
    searchTasks: "search_tasks",
    searchKnowledgebase: "search_knowledgebase",
}

// ── ADI Service Discovery Types ─────────────────────────────

model AdiServiceCapabilities {
    subscriptions: boolean;
    notifications: boolean;
    streaming: boolean;
}

model AdiMethodInfo {
    name: string;
    description: string;
    streaming: boolean;
    params_schema?: unknown;
    result_schema?: unknown;
    deprecated?: boolean;
    deprecated_message?: string;
}

model AdiServiceInfo {
    id: string;
    name: string;
    version: string;
    description?: string;
    methods: AdiMethodInfo[];
    capabilities: AdiServiceCapabilities;
}

// ── Silk Channel ────────────────────────────────────────────

@channel("silk")
interface Silk {
    // Client → Cocoon requests
    @request
    createSession(cwd?: string, env?: Record<string>, shell?: string): {
        session_id: string;
        cwd: string;
        shell: string;
    };

    @event
    execute(session_id: string, command: string, command_id: string, cols?: int32, rows?: int32, env?: Record<string>): void;

    @event
    input(session_id: string, command_id: string, data: string): void;

    @event
    resize(session_id: string, command_id: string, cols: int32, rows: int32): void;

    @event
    signal(session_id: string, command_id: string, signal: SilkSignal): void;

    @event
    closeSession(session_id: string): void;

    // Cocoon → Client responses
    @event
    commandStarted(session_id: string, command_id: string, interactive: boolean): void;

    @event
    output(session_id: string, command_id: string, stream: SilkStream, data: string, html?: SilkHtmlSpan[]): void;

    @event
    interactiveRequired(session_id: string, command_id: string, reason: string, pty_session_id: string): void;

    @event
    ptyOutput(session_id: string, command_id: string, pty_session_id: string, data: string): void;

    @event
    commandCompleted(session_id: string, command_id: string, exit_code: int32, cwd: string): void;

    @event
    sessionClosed(session_id: string): void;

    @event
    error(session_id?: string, command_id?: string, code: string, message: string): void;
}

// ── ADI Service Channel ─────────────────────────────────────

@channel("adi")
interface Adi {
    // Request/Response
    @event
    request(request_id: string, service: string, method: string, params: unknown): void;

    @event
    success(request_id: string, service: string, method: string, data: unknown): void;

    @event
    requestError(request_id: string, service: string, method: string, code: string, message: string): void;

    @event
    serviceNotFound(request_id: string, service: string): void;

    @event
    methodNotFound(request_id: string, service: string, method: string, available_methods: string[]): void;

    @event
    streamChunk(request_id: string, service: string, method: string, data: unknown, done: boolean): void;

    // Discovery
    @event
    listServices(request_id: string): void;

    @event
    servicesList(request_id: string, services: AdiServiceInfo[]): void;

    // Notifications
    @event
    servicesChanged(added: string[], removed: string[], updated: string[]): void;

    // Subscriptions
    @event
    subscribe(request_id: string, service: string, event: string, filter?: unknown): void;

    @event
    subscribed(request_id: string, subscription_id: string, service: string, event: string): void;

    @event
    unsubscribe(subscription_id: string): void;

    @event
    unsubscribed(subscription_id: string): void;

    @event
    subscriptionEvent(subscription_id: string, event: string, data: unknown): void;

    @event
    subscriptionError(request_id: string, code: string, message: string): void;
}

// ── Plugin Channel ─────────────────────────────────────────

model PluginInstallResult {
    request_id: string;
    success: boolean;
    plugin_id: string;
    stdout: string;
    stderr: string;
}

@channel("plugin")
interface Plugin {
    @request
    installPlugin(request_id: string, plugin_id: string, registry?: string, version?: string): PluginInstallResult;

    @event
    installError(request_id: string, plugin_id: string, code: string, message: string): void;
}

// ── WebRTC Channel ──────────────────────────────────────────

@channel("webrtc")
interface WebRtc {
    @event
    startSession(session_id: string, device_id: string, user_id?: string, data_channels?: string[]): void;

    @event
    offer(session_id: string, sdp: string): void;

    @event
    answer(session_id: string, sdp: string): void;

    @event
    iceCandidate(session_id: string, candidate: string, sdp_mid?: string, sdp_mline_index?: int32): void;

    @event
    sessionEnded(session_id: string, reason?: string): void;

    @event
    data(session_id: string, channel: string, data: string, binary: boolean): void;

    @event
    error(session_id: string, code: string, message: string): void;
}

// ── Query Channel ───────────────────────────────────────────

@channel("query")
interface Query {
    @event
    queryLocal(query_id: string, query_type: QueryType, params: unknown): void;

    @event
    queryResult(query_id: string, data: unknown, is_final: boolean): void;
}
