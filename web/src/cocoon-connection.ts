import type { Connection } from '@adi-family/plugin-signaling/bus';
import type { CocoonWebRTC } from './cocoon-webrtc';
import type { SignalingMessage } from './generated/messages';
import { buildRequestFrame, parseResponseFrame, decodePayloadJson, decodePayloadText } from './adi-frame';
import type { ResponseStatus } from './adi-frame';

const genId = (): string => {
  if (typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  const h = Array.from(crypto.getRandomValues(new Uint8Array(16)), b => b.toString(16).padStart(2, '0')).join('');
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
};

/** Typed result from plugin install — extracted from generated SignalingMessage. */
export type PluginInstallResult = Omit<
  Extract<SignalingMessage, { type: 'plugin_install_plugin_response' }>,
  'type'
>;

type Pending = {
  resolve: (data: unknown) => void;
  reject: (err: Error) => void;
};

type StreamPending = {
  push: (data: unknown) => void;
  done: () => void;
  reject: (err: Error) => void;
};

/** Implements Connection interface over a CocoonWebRTC data channel. */
export class CocoonConnection implements Connection {
  readonly id: string;
  plugins: string[] = [];

  private readonly pending = new Map<string, Pending>();
  private readonly streams = new Map<string, StreamPending>();
  private unsubText: (() => void) | null = null;
  private unsubBinary: (() => void) | null = null;

  constructor(
    cocoonId: string,
    private readonly webrtc: CocoonWebRTC,
  ) {
    this.id = cocoonId;
    this.unsubText = webrtc.onAdiMessage((msg) => this.handleMessage(msg));
    this.unsubBinary = webrtc.onAdiBinaryMessage((data) => this.handleBinaryFrame(data));
  }

  async request<T>(plugin: string, method: string, params?: unknown): Promise<T> {
    await this.webrtc.connect();
    const requestId = genId();
    return new Promise<T>((resolve, reject) => {
      this.pending.set(requestId, {
        resolve: resolve as (data: unknown) => void,
        reject,
      });
      this.webrtc.sendAdiBinary(buildRequestFrame(requestId, plugin, method, params));
    });
  }

  async *stream<T>(plugin: string, method: string, params?: unknown): AsyncGenerator<T> {
    await this.webrtc.connect();
    const requestId = genId();
    const buffer: unknown[] = [];
    let finished = false;
    let error: Error | null = null;
    let notify: (() => void) | null = null;

    this.streams.set(requestId, {
      push: (data) => { buffer.push(data); notify?.(); },
      done: () => { finished = true; notify?.(); },
      reject: (err) => { error = err; notify?.(); },
    });

    this.webrtc.sendAdiBinary(buildRequestFrame(requestId, plugin, method, params, true));

    try {
      while (true) {
        if (error) throw error;
        if (buffer.length > 0) {
          yield buffer.shift() as T;
          continue;
        }
        if (finished) return;
        await new Promise<void>((r) => { notify = r; });
        notify = null;
      }
    } finally {
      this.streams.delete(requestId);
    }
  }

  async httpProxy(plugin: string, path: string, init?: RequestInit): Promise<Response> {
    await this.webrtc.connect();
    const requestId = genId();
    return new Promise<Response>((resolve, reject) => {
      this.pending.set(requestId, {
        resolve: (data) => {
          const d = data as { status_code: number; headers: Record<string, string>; body: string };
          resolve(new Response(d.body, { status: d.status_code, headers: d.headers }));
        },
        reject,
      });
      this.webrtc.sendAdi({
        type: 'proxy_http',
        request_id: requestId,
        service_name: plugin,
        method: init?.method ?? 'GET',
        path,
        headers: Object.fromEntries(new Headers(init?.headers).entries()),
        body: init?.body ?? null,
      });
    });
  }

  async httpDirect(url: string, init?: RequestInit): Promise<Response> {
    return fetch(url, init);
  }

  /** Queries available plugins from the cocoon and populates this.plugins. */
  async refreshPlugins(): Promise<string[]> {
    await this.webrtc.connect();
    const requestId = genId();
    const pluginInfos = await new Promise<Array<{ id: string }>>((resolve, reject) => {
      this.pending.set(requestId, {
        resolve: resolve as (data: unknown) => void,
        reject,
      });
      this.webrtc.sendAdi({ type: 'list_plugins', request_id: requestId });
    });
    this.plugins = pluginInfos.map(s => s.id);
    return this.plugins;
  }

  /** Install an ADI plugin on the cocoon (typed protocol message). */
  async installPlugin(
    pluginId: string,
    opts?: { registry?: string; version?: string },
  ): Promise<PluginInstallResult> {
    await this.webrtc.connect();
    const requestId = genId();
    return new Promise<PluginInstallResult>((resolve, reject) => {
      this.pending.set(requestId, {
        resolve: resolve as (data: unknown) => void,
        reject,
      });
      this.webrtc.sendAdi({
        type: 'plugin_install_plugin',
        request_id: requestId,
        plugin_id: pluginId,
        registry: opts?.registry,
        version: opts?.version,
      } satisfies Extract<SignalingMessage, { type: 'plugin_install_plugin' }>);
    });
  }

  dispose(): void {
    this.unsubText?.();
    this.unsubText = null;
    this.unsubBinary?.();
    this.unsubBinary = null;
    for (const p of this.pending.values()) p.reject(new Error('Connection disposed'));
    this.pending.clear();
    for (const s of this.streams.values()) s.reject(new Error('Connection disposed'));
    this.streams.clear();
  }

  private handleBinaryFrame(data: ArrayBuffer): void {
    try {
      const { header, payload } = parseResponseFrame(data);
      const requestId = header.id;

      const statusHandlers: Record<ResponseStatus, () => void> = {
        success: () => {
          const result = decodePayloadJson(payload);
          this.pending.get(requestId)?.resolve(result);
          this.pending.delete(requestId);
        },
        error: () => {
          const message = decodePayloadText(payload);
          this.pending.get(requestId)?.reject(new Error(message));
          this.pending.delete(requestId);
          const stream = this.streams.get(requestId);
          if (stream) { stream.reject(new Error(message)); this.streams.delete(requestId); }
        },
        plugin_not_found: () => {
          const message = decodePayloadText(payload);
          this.pending.get(requestId)?.reject(new Error(`Plugin not found: ${message}`));
          this.pending.delete(requestId);
        },
        method_not_found: () => {
          const message = decodePayloadText(payload);
          this.pending.get(requestId)?.reject(new Error(`Method not found: ${message}`));
          this.pending.delete(requestId);
        },
        invalid_request: () => {
          const message = decodePayloadText(payload);
          this.pending.get(requestId)?.reject(new Error(`Invalid request: ${message}`));
          this.pending.delete(requestId);
        },
        stream_chunk: () => {
          const stream = this.streams.get(requestId);
          if (stream) stream.push(decodePayloadJson(payload));
        },
        stream_end: () => {
          const stream = this.streams.get(requestId);
          if (stream) {
            if (payload.length > 0) stream.push(decodePayloadJson(payload));
            stream.done();
            this.streams.delete(requestId);
          }
        },
      };

      statusHandlers[header.status]?.();
    } catch (err) {
      console.error('[CocoonConnection] failed to parse binary frame:', err);
    }
  }

  /** Handle text JSON messages (discovery, subscriptions, legacy). */
  private handleMessage(msg: unknown): void {
    if (!msg || typeof msg !== 'object' || !('type' in msg)) return;
    const m = msg as Record<string, unknown>;
    const requestId = m['request_id'] as string | undefined;
    if (!requestId) return;

    switch (m['type']) {
      case 'success': {
        this.pending.get(requestId)?.resolve(m['data']);
        this.pending.delete(requestId);
        break;
      }
      case 'error': {
        this.pending.get(requestId)?.reject(new Error(`${m['plugin']}.${m['method']}: ${m['message']}`));
        this.pending.delete(requestId);
        break;
      }
      case 'plugin_not_found': {
        this.pending.get(requestId)?.reject(new Error(`Plugin '${m['plugin']}' not found`));
        this.pending.delete(requestId);
        break;
      }
      case 'method_not_found': {
        this.pending.get(requestId)?.reject(new Error(`Method '${m['method']}' not found on '${m['plugin']}'`));
        this.pending.delete(requestId);
        break;
      }
      case 'plugins_list': {
        this.pending.get(requestId)?.resolve(m['plugins']);
        this.pending.delete(requestId);
        break;
      }
      case 'stream': {
        const stream = this.streams.get(requestId);
        if (stream) {
          stream.push(m['data']);
          if (m['done']) { stream.done(); this.streams.delete(requestId); }
        }
        break;
      }
      case 'proxy_result': {
        this.pending.get(requestId)?.resolve(m);
        this.pending.delete(requestId);
        break;
      }
      case 'plugin_install_plugin_response': {
        const { type: _, ...result } = m as Record<string, unknown>;
        this.pending.get(requestId)?.resolve(result);
        this.pending.delete(requestId);
        break;
      }
      case 'plugin_install_error': {
        this.pending.get(requestId)?.reject(
          new Error(`Plugin install failed [${m['code']}]: ${m['message']}`),
        );
        this.pending.delete(requestId);
        break;
      }
    }
  }
}
