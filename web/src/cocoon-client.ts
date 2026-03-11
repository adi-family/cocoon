import "@adi-family/plugin-signaling";
import { Logger, trace, type EventBus } from '@adi-family/sdk-plugin';
import type { SilkResponse } from './silk-types';
import { SilkSession } from './silk-session';
import { CocoonWebRTC, type WebRTCConfig } from './cocoon-webrtc';
import { CocoonConnection } from './cocoon-connection';
import { CocoonBusKey } from './generated/bus-types';
import './generated/bus-events';

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
  private readonly webrtc: CocoonWebRTC;
  private readonly sessions = new Map<string, SilkSession>();
  private readonly unsubs: (() => void)[] = [];

  constructor(cocoonId: string, server: SyncDataSender, bus: EventBus, rtcConfig?: WebRTCConfig, userId?: string) {
    this.cocoonId = cocoonId;
    this.bus = bus;
    this.server = server;
    this.webrtc = new CocoonWebRTC(cocoonId, server, bus, rtcConfig, userId);

    // Route silk responses from WebRTC data channel to session handlers
    this.unsubs.push(
      this.webrtc.onMessage((msg) => this.handleSilkMsg(msg)),
    );
  }

  createConnection(): CocoonConnection {
    return new CocoonConnection(this.cocoonId, this.webrtc);
  }

  getSession(sessionId: string): SilkSession | undefined {
    return this.sessions.get(sessionId);
  }

  allSessions(): ReadonlyMap<string, SilkSession> {
    return this.sessions;
  }

  @trace('creating silk session')
  async createSession(opts?: { cwd?: string; env?: Record<string, string>; shell?: string }): Promise<SilkSession> {
    // Ensure WebRTC data channel is open before sending
    console.log(`[CocoonClient] createSession: connecting WebRTC for cocoon=${this.cocoonId}`);
    await this.webrtc.connect();
    console.log(`[CocoonClient] createSession: WebRTC connected, sending silk_create_session`);

    return new Promise((resolve, reject) => {
      const cleanup = (): void => { unsub1(); unsub2(); };

      const unsub1 = this.bus.on(
        CocoonBusKey.SessionCreated,
        (ev) => {
          console.log(`[CocoonClient] SessionCreated event received: cocoonId=${ev.cocoonId} sessionId=${ev.sessionId}`);
          if (ev.cocoonId !== this.cocoonId) {
            console.log(`[CocoonClient] SessionCreated SKIPPED (cocoonId mismatch: ${ev.cocoonId} !== ${this.cocoonId})`);
            return;
          }
          cleanup();
          const dcSender = this.makeDcSender();
          const session = new SilkSession(ev.sessionId, this.cocoonId, ev.cwd, ev.shell, dcSender);
          this.sessions.set(ev.sessionId, session);
          console.log(`[CocoonClient] createSession RESOLVED! sessionId=${ev.sessionId}`);
          resolve(session);
        },
        SOURCE,
      );

      const unsub2 = this.bus.on(
        CocoonBusKey.Error,
        (ev) => {
          console.error(`[CocoonClient] Error event received: cocoonId=${ev.cocoonId} message=${ev.message}`);
          if (ev.cocoonId !== this.cocoonId) return;
          cleanup();
          reject(new Error(ev.message));
        },
        SOURCE,
      );

      const msg = {
        type: 'silk_create_session',
        cwd: opts?.cwd,
        env: opts?.env,
        shell: opts?.shell,
      };
      console.log(`[CocoonClient] sending silk_create_session:`, msg);
      this.webrtc.send(msg);
      console.log(`[CocoonClient] silk_create_session sent, waiting for response...`);
    });
  }

  dispose(): void {
    for (const session of this.sessions.values()) session.dispose();
    this.sessions.clear();
    this.webrtc.dispose();
    this.unsubs.forEach((fn) => fn());
    this.unsubs.length = 0;
  }

  /** Creates a SyncDataSender backed by the WebRTC data channel. */
  private makeDcSender(): SyncDataSender {
    return {
      url: this.server.url,
      sendSyncData: (payload) => this.webrtc.send(payload),
    };
  }

  private handleSilkMsg(payload: unknown): void {
    if (!payload || typeof payload !== 'object' || !('type' in payload)) {
      console.log(`[CocoonClient] handleSilkMsg: ignored non-object/no-type payload`, payload);
      return;
    }

    const response = payload as SilkResponse;
    const cocoonId = this.cocoonId;
    console.log(`[CocoonClient] handleSilkMsg: type=${response.type} cocoon=${cocoonId}`);

    switch (response.type) {
      case 'silk_create_session_response':
        console.log(`[CocoonClient] silk_create_session_response received! sessionId=${response.session_id} cwd=${response.cwd} shell=${response.shell}`);
        this.bus.emit(
          CocoonBusKey.SessionCreated,
          { cocoonId, sessionId: response.session_id, cwd: response.cwd, shell: response.shell },
          SOURCE,
        );
        break;

      case 'silk_session_closed': {
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

      case 'silk_output':
      case 'silk_pty_output':
      case 'silk_interactive_required':
      case 'silk_command_started':
      case 'silk_command_completed': {
        const session = this.sessions.get(response.session_id);
        if (session) session._handleResponse(response);
        break;
      }

      case 'silk_error': {
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
