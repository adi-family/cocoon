import type { Connection } from '@adi-family/plugin-signaling/bus';
import type { CocoonWebRTC } from './cocoon-webrtc';
import type { SignalingMessage } from './generated/messages';

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
  services: string[] = [];

  private readonly pending = new Map<string, Pending>();
  private readonly streams = new Map<string, StreamPending>();
  private unsub: (() => void) | null = null;

  constructor(
    cocoonId: string,
    private readonly webrtc: CocoonWebRTC,
  ) {
    this.id = cocoonId;
    this.unsub = webrtc.onAdiMessage((msg) => this.handleMessage(msg));
  }

  async request<T>(service: string, method: string, params?: unknown): Promise<T> {
    await this.webrtc.connect();
    const requestId = genId();
    return new Promise<T>((resolve, reject) => {
      this.pending.set(requestId, {
        resolve: resolve as (data: unknown) => void,
        reject,
      });
      this.webrtc.sendAdi({
        type: 'adi_request',
        request_id: requestId,
        service,
        method,
        params: params ?? {},
      });
    });
  }

  async *stream<T>(service: string, method: string, params?: unknown): AsyncGenerator<T> {
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

    this.webrtc.sendAdi({
      type: 'adi_request',
      request_id: requestId,
      service,
      method,
      params: params ?? {},
    });

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

  async httpProxy(service: string, path: string, init?: RequestInit): Promise<Response> {
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
        service_name: service,
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

  /** Queries available services from the cocoon and populates this.services. */
  async refreshServices(): Promise<string[]> {
    await this.webrtc.connect();
    const requestId = genId();
    const serviceInfos = await new Promise<Array<{ id: string }>>((resolve, reject) => {
      this.pending.set(requestId, {
        resolve: resolve as (data: unknown) => void,
        reject,
      });
      this.webrtc.sendAdi({ type: 'list_services', request_id: requestId });
    });
    this.services = serviceInfos.map(s => s.id);
    return this.services;
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
    this.unsub?.();
    this.unsub = null;
    for (const p of this.pending.values()) p.reject(new Error('Connection disposed'));
    this.pending.clear();
    for (const s of this.streams.values()) s.reject(new Error('Connection disposed'));
    this.streams.clear();
  }

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
        this.pending.get(requestId)?.reject(new Error(`${m['service']}.${m['method']}: ${m['message']}`));
        this.pending.delete(requestId);
        break;
      }
      case 'service_not_found': {
        this.pending.get(requestId)?.reject(new Error(`Service '${m['service']}' not found`));
        this.pending.delete(requestId);
        break;
      }
      case 'method_not_found': {
        this.pending.get(requestId)?.reject(new Error(`Method '${m['method']}' not found on '${m['service']}'`));
        this.pending.delete(requestId);
        break;
      }
      case 'services_list': {
        this.pending.get(requestId)?.resolve(m['services']);
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
