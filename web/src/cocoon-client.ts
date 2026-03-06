import "@adi/signaling-web-plugin/bus";
import { Logger, trace, type EventBus } from '@adi-family/sdk-plugin';
import type { SilkResponse } from './silk-types';
import { SilkSession } from './silk-session';
import { CocoonBusKey } from './bus';
import './bus';

export interface SyncDataSender {
  readonly url: string;
  sendSyncData(payload: unknown): void;
}

const SOURCE = 'cocoon';

export class CocoonClient {
  readonly cocoonId: string;

  private readonly log = new Logger('cocoon-client', () => ({
    cocoonId: this.cocoonId,
    server: this.server.url,
  }));
  private readonly bus: EventBus;
  private readonly server: SyncDataSender;
  private readonly sessions = new Map<string, SilkSession>();
  private readonly unsubs: (() => void)[] = [];

  constructor(cocoonId: string, server: SyncDataSender, bus: EventBus) {
    this.cocoonId = cocoonId;
    this.bus = bus;
    this.server = server;

    this.unsubs.push(
      bus.on(
        'signaling:sync-data',
        ({ url, payload }) => {
          if (url !== server.url) return;
          this.handleSyncData(payload);
        },
        SOURCE,
      ),
    );
  }

  getSession(sessionId: string): SilkSession | undefined {
    return this.sessions.get(sessionId);
  }

  allSessions(): ReadonlyMap<string, SilkSession> {
    return this.sessions;
  }

  @trace('creating silk session')
  createSession(opts?: { cwd?: string; env?: Record<string, string>; shell?: string }): Promise<SilkSession> {
    return new Promise((resolve, reject) => {
      const cleanup = (): void => { unsub1(); unsub2(); };

      const unsub1 = this.bus.on(
        CocoonBusKey.SessionCreated,
        (ev) => {
          if (ev.cocoonId !== this.cocoonId) return;
          cleanup();
          const session = new SilkSession(ev.sessionId, this.cocoonId, ev.cwd, ev.shell, this.server);
          this.sessions.set(ev.sessionId, session);
          resolve(session);
        },
        SOURCE,
      );

      const unsub2 = this.bus.on(
        CocoonBusKey.Error,
        (ev) => {
          if (ev.cocoonId !== this.cocoonId) return;
          cleanup();
          reject(new Error(ev.message));
        },
        SOURCE,
      );

      this.server.sendSyncData({
        type: 'create_session',
        cwd: opts?.cwd,
        env: opts?.env,
        shell: opts?.shell,
      });
    });
  }

  dispose(): void {
    for (const session of this.sessions.values()) session.dispose();
    this.sessions.clear();
    this.unsubs.forEach((fn) => fn());
    this.unsubs.length = 0;
  }

  private handleSyncData(payload: unknown): void {
    if (!payload || typeof payload !== 'object' || !('type' in payload)) return;

    const response = payload as SilkResponse;
    const cocoonId = this.cocoonId;

    switch (response.type) {
      case 'session_created':
        this.bus.emit(
          CocoonBusKey.SessionCreated,
          { cocoonId, sessionId: response.session_id, cwd: response.cwd, shell: response.shell },
          SOURCE,
        );
        break;

      case 'session_closed': {
        const session = this.sessions.get(response.session_id);
        if (session) {
          session._handleResponse(response);
          session.dispose();
          this.sessions.delete(response.session_id);
        }
        this.bus.emit(
          CocoonBusKey.SessionClosed,
          { cocoonId, sessionId: response.session_id },
          SOURCE,
        );
        break;
      }

      case 'output':
      case 'pty_output':
      case 'interactive_required':
      case 'command_started':
      case 'command_completed': {
        const session = this.sessions.get(response.session_id);
        if (session) session._handleResponse(response);
        break;
      }

      case 'error': {
        if (response.session_id) {
          const session = this.sessions.get(response.session_id);
          if (session) session._handleResponse(response);
        } else {
          this.bus.emit(
            CocoonBusKey.Error,
            { cocoonId, code: response.code, message: response.message },
            SOURCE,
          );
        }
        break;
      }
    }
  }
}
