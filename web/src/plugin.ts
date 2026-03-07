import '@adi/auth-web-plugin';
import '@adi/signaling-web-plugin';
import { AdiPlugin } from '@adi-family/sdk-plugin';
import { AdiDebugScreenBusKey } from '@adi/debug-screen-web-plugin/bus';
import { AdiSignalingBusKey, type DeviceInfo } from '@adi/signaling-web-plugin/bus';
import { AdiRouterBusKey } from '@adi/router-web-plugin/bus';
import { CocoonClient } from './cocoon-client';
import type { WebRTCConfig } from './cocoon-webrtc';
import { PLUGIN_ID, PLUGIN_VERSION } from './config';
import type { AdiCocoonDebugElement, CocoonDebugInfo } from './debug-section';
import type { AdiCocoonListElement, CocoonListItem, SetupConnectEvent } from './component';
import './bus';

export interface CocoonApi {
  getClient(cocoonId: string): CocoonClient | undefined;
  allClients(): ReadonlyMap<string, CocoonClient>;
  createClient(cocoonId: string, signalingUrl: string, rtcConfig?: WebRTCConfig): CocoonClient | undefined;
  removeClient(cocoonId: string): void;
}

export class CocoonPlugin extends AdiPlugin implements CocoonApi {
  readonly id = PLUGIN_ID;
  readonly version = PLUGIN_VERSION;

  private readonly clients = new Map<string, CocoonClient>();
  private debugEl: AdiCocoonDebugElement | null = null;
  private listEl: AdiCocoonListElement | null = null;
  private lastDevices: DeviceInfo[] = [];
  private readonly unsubs: (() => void)[] = [];

  get api(): CocoonApi {
    return this;
  }

  getClient(cocoonId: string): CocoonClient | undefined {
    return this.clients.get(cocoonId);
  }

  allClients(): ReadonlyMap<string, CocoonClient> {
    return this.clients;
  }

  createClient(cocoonId: string, signalingUrl: string, rtcConfig?: WebRTCConfig): CocoonClient | undefined {
    if (this.clients.has(cocoonId)) return this.clients.get(cocoonId);

    const signalingApi = this.app.api('adi.signaling');
    const server = signalingApi.getServer(signalingUrl);
    if (!server) return undefined;

    const client = new CocoonClient(cocoonId, server, this.bus, rtcConfig);
    this.clients.set(cocoonId, client);
    this.syncViews();
    return client;
  }

  removeClient(cocoonId: string): void {
    const client = this.clients.get(cocoonId);
    if (!client) return;
    client.dispose();
    this.clients.delete(cocoonId);
    this.syncViews();
  }

  override async onRegister(): Promise<void> {
    const [, { AdiCocoonListElement }] = await Promise.all([
      import('./debug-section.js'),
      import('./component.js'),
    ]);

    if (!customElements.get('adi-cocoon-list')) {
      customElements.define('adi-cocoon-list', AdiCocoonListElement);
    }

    this.bus.emit(AdiRouterBusKey.RegisterRoute, {
      pluginId: PLUGIN_ID,
      path: '',
      init: () => {
        this.listEl = document.createElement('adi-cocoon-list') as AdiCocoonListElement;
        this.listEl.subtokenProvider = () => this.getSetupSubtoken();
        this.listEl.addEventListener('setup-connect', ((e: CustomEvent<SetupConnectEvent>) => {
          const url = e.detail.signalingUrl;
          const signalingApi = this.app.api('adi.signaling');
          if (!signalingApi.getServer(url)) {
            signalingApi.addServer(url);
          }
        }) as EventListener);
        this.syncList();
        return this.listEl;
      },
      label: 'Cocoons',
    }, PLUGIN_ID);

    this.bus.emit(
      AdiDebugScreenBusKey.RegisterSection,
      {
        pluginId: PLUGIN_ID,
        init: () => {
          this.debugEl = document.createElement('adi-cocoon-debug') as AdiCocoonDebugElement;
          this.debugEl.subtokenProvider = () => this.getSetupSubtoken();
          this.syncDebug();
          return this.debugEl;
        },
        label: 'Cocoon',
      },
      PLUGIN_ID,
    );

    this.unsubs.push(
      this.bus.on('cocoon:session-created', () => this.syncViews(), PLUGIN_ID),
      this.bus.on('cocoon:session-closed', () => this.syncViews(), PLUGIN_ID),
      this.bus.on('cocoon:error', () => this.syncViews(), PLUGIN_ID),
      this.bus.on(AdiSignalingBusKey.Devices, ({ devices }) => {
        this.lastDevices = devices;
        this.debugEl?.updateDevices(devices);
      }, PLUGIN_ID),
    );
  }

  override onUnregister(): void {
    this.unsubs.forEach((fn) => fn());
    this.unsubs.length = 0;
    for (const client of this.clients.values()) client.dispose();
    this.clients.clear();
  }

  private syncViews(): void {
    this.syncDebug();
    this.syncList();
  }

  private syncDebug(): void {
    if (!this.debugEl) return;
    const infos: CocoonDebugInfo[] = [];
    for (const [cocoonId, client] of this.clients) {
      infos.push({ cocoonId, sessions: client.allSessions().size });
    }
    this.debugEl.cocoons = infos;
    this.debugEl.updateDevices(this.lastDevices);
    const signalingApi = this.app.api('adi.signaling');
    this.debugEl.signalingUrls = [...signalingApi.allServers().keys()];
  }

  private getAuthDomain(): string | null {
    const signalingApi = this.app.api('adi.signaling');
    const firstServer = signalingApi.allServers().values().next().value;
    if (!firstServer) return null;

    const wsUrl = new URL(firstServer.url);
    return `${wsUrl.protocol === 'wss:' ? 'https:' : 'http:'}//${wsUrl.host}/api/auth`;
  }

  private async getSetupSubtoken(): Promise<string | null> {
    const authDomain = this.getAuthDomain();
    if (!authDomain) return null;

    try {
      const token = await this.app.api('adi.auth').getToken(authDomain);
      if (!token) return null;

      const resp = await fetch(`${authDomain}/subtoken`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${token}`,
        },
        body: JSON.stringify({ ttlSeconds: 600 }),
      });
      if (!resp.ok) return null;

      const data = await resp.json();
      return data.accessToken ?? data.access_token ?? null;
    } catch {
      return null;
    }
  }

  private syncList(): void {
    if (!this.listEl) return;
    const items: CocoonListItem[] = [];
    for (const [cocoonId, client] of this.clients) {
      const sessions = [...client.allSessions().values()].map(s => ({
        sessionId: s.sessionId,
        cwd: s.cwd,
        shell: s.shell,
        closed: s.closed,
      }));
      items.push({ cocoonId, sessionCount: sessions.length, sessions });
    }
    this.listEl.cocoons = items;

    const signalingApi = this.app.api('adi.signaling');
    this.listEl.signalingUrls = [...signalingApi.allServers().keys()];
  }
}
