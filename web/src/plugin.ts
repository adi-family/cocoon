import "@adi/signaling-web-plugin";
import { AdiPlugin } from '@adi-family/sdk-plugin';
import { AdiDebugScreenBusKey } from '@adi/debug-screen-web-plugin/bus';
import { CocoonClient } from './cocoon-client';
import { PLUGIN_ID, PLUGIN_VERSION } from './config';
import type { AdiCocoonDebugElement, CocoonDebugInfo } from './debug-section';
import './bus';

export interface CocoonApi {
  getClient(cocoonId: string): CocoonClient | undefined;
  allClients(): ReadonlyMap<string, CocoonClient>;
  createClient(cocoonId: string, signalingUrl: string): CocoonClient | undefined;
  removeClient(cocoonId: string): void;
}

export class CocoonPlugin extends AdiPlugin implements CocoonApi {
  readonly id = PLUGIN_ID;
  readonly version = PLUGIN_VERSION;

  private readonly clients = new Map<string, CocoonClient>();
  private debugEl: AdiCocoonDebugElement | null = null;
  private readonly debugUnsubs: (() => void)[] = [];

  get api(): CocoonApi {
    return this;
  }

  getClient(cocoonId: string): CocoonClient | undefined {
    return this.clients.get(cocoonId);
  }

  allClients(): ReadonlyMap<string, CocoonClient> {
    return this.clients;
  }

  createClient(cocoonId: string, signalingUrl: string): CocoonClient | undefined {
    if (this.clients.has(cocoonId)) return this.clients.get(cocoonId);

    const signalingApi = this.app.api('adi.signaling');
    const server = signalingApi.getServer(signalingUrl);
    if (!server) return undefined;

    const client = new CocoonClient(cocoonId, server, this.bus);
    this.clients.set(cocoonId, client);
    this.syncDebug();
    return client;
  }

  removeClient(cocoonId: string): void {
    const client = this.clients.get(cocoonId);
    if (!client) return;
    client.dispose();
    this.clients.delete(cocoonId);
    this.syncDebug();
  }

  override async onRegister(): Promise<void> {
    await import('./debug-section.js');
    this.bus.emit(
      AdiDebugScreenBusKey.RegisterSection,
      {
        pluginId: PLUGIN_ID,
        init: () => {
          this.debugEl = document.createElement('adi-cocoon-debug') as AdiCocoonDebugElement;
          this.syncDebug();
          return this.debugEl;
        },
        label: 'Cocoon',
      },
      PLUGIN_ID,
    );

    this.debugUnsubs.push(
      this.bus.on('cocoon:session-created', () => this.syncDebug(), PLUGIN_ID),
      this.bus.on('cocoon:session-closed', () => this.syncDebug(), PLUGIN_ID),
      this.bus.on('cocoon:error', () => this.syncDebug(), PLUGIN_ID),
    );
  }

  override onUnregister(): void {
    this.debugUnsubs.forEach((fn) => fn());
    this.debugUnsubs.length = 0;
    for (const client of this.clients.values()) client.dispose();
    this.clients.clear();
  }

  private syncDebug(): void {
    if (!this.debugEl) return;
    const infos: CocoonDebugInfo[] = [];
    for (const [cocoonId, client] of this.clients) {
      infos.push({ cocoonId, sessions: client.allSessions().size });
    }
    this.debugEl.cocoons = infos;
  }
}
