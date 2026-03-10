import '@adi-family/plugin-auth';
import '@adi-family/plugin-signaling';
import { AdiPlugin } from '@adi-family/sdk-plugin';
import { AdiDebugScreenBusKey } from '@adi-family/plugin-debug-screen/bus';
import { AdiSignalingBusKey, type DeviceInfo } from '@adi-family/plugin-signaling/bus';
import { CocoonBusKey, type ConnectionSettings } from '@adi-family/cocoon-plugin-interface';
import { CocoonClient } from './cocoon-client';
import { CocoonConnection } from './cocoon-connection';
import type { WebRTCConfig } from './cocoon-webrtc';
import { PLUGIN_ID, PLUGIN_VERSION } from './config';
import type { AdiCocoonDebugElement, CocoonDebugInfo } from './debug-section';
import type { AdiCocoonListElement, CocoonListItem, SetupConnectEvent } from './component';
import './bus';

interface RegistryPlugin {
  id: string;
  pluginTypes: string[];
}

async function fetchRegistryPlugins(registryUrls: string[]): Promise<RegistryPlugin[]> {
  const seen = new Set<string>();
  const result: RegistryPlugin[] = [];

  for (const url of registryUrls) {
    try {
      const resp = await fetch(`${url}/v1/index`);
      if (!resp.ok) continue;
      const data = await resp.json();
      for (const p of data.plugins ?? []) {
        if (seen.has(p.id)) continue;
        seen.add(p.id);
        result.push({ id: p.id, pluginTypes: p.pluginTypes ?? [] });
      }
    } catch {
      // skip unavailable registries
    }
  }

  return result;
}

export interface CocoonApi {
  getClient(cocoonId: string): CocoonClient | undefined;
  allClients(): ReadonlyMap<string, CocoonClient>;
  createClient(cocoonId: string, signalingUrl: string, rtcConfig?: WebRTCConfig): CocoonClient | undefined;
  removeClient(cocoonId: string): void;
  getSettings(cocoonId: string): ConnectionSettings;
  updateSettings(cocoonId: string, patch: Partial<ConnectionSettings>): void;
}

export class CocoonPlugin extends AdiPlugin implements CocoonApi {
  readonly id = PLUGIN_ID;
  readonly version = PLUGIN_VERSION;

  private readonly clients = new Map<string, CocoonClient>();
  private readonly connections = new Map<string, CocoonConnection>();
  private readonly connectionSettings = new Map<string, ConnectionSettings>();
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

  getSettings(cocoonId: string): ConnectionSettings {
    return this.connectionSettings.get(cocoonId) ?? {};
  }

  updateSettings(cocoonId: string, patch: Partial<ConnectionSettings>): void {
    const current = this.connectionSettings.get(cocoonId) ?? {};
    const updated = { ...current, ...patch };
    this.connectionSettings.set(cocoonId, updated);
    this.bus.emit(CocoonBusKey.SettingsChanged, { id: cocoonId, settings: updated }, PLUGIN_ID);

    if (patch.autoinstallPlugins) {
      void this.autoinstallIfNeeded(cocoonId);
    }
  }

  createClient(cocoonId: string, signalingUrl: string, rtcConfig?: WebRTCConfig): CocoonClient | undefined {
    if (this.clients.has(cocoonId)) return this.clients.get(cocoonId);

    const signalingApi = this.app.api('adi.signaling');
    const server = signalingApi.getServer(signalingUrl);
    if (!server) return undefined;

    const client = new CocoonClient(cocoonId, server, this.bus, rtcConfig);
    this.clients.set(cocoonId, client);

    const connection = client.createConnection();
    this.connections.set(cocoonId, connection);
    this.bus.emit(CocoonBusKey.ConnectionAdded, { id: cocoonId, connection }, PLUGIN_ID);

    void this.autoinstallIfNeeded(cocoonId);
    this.syncViews();
    return client;
  }

  removeClient(cocoonId: string): void {
    const client = this.clients.get(cocoonId);
    if (!client) return;

    const connection = this.connections.get(cocoonId);
    if (connection) {
      connection.dispose();
      this.connections.delete(cocoonId);
      this.bus.emit(CocoonBusKey.ConnectionRemoved, { id: cocoonId }, PLUGIN_ID);
    }

    client.dispose();
    this.clients.delete(cocoonId);
    this.connectionSettings.delete(cocoonId);
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

    const signalingApi = this.app.api('adi.signaling');
    this.lastDevices = [...signalingApi.allDevices()];

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
    for (const conn of this.connections.values()) conn.dispose();
    this.connections.clear();
    for (const client of this.clients.values()) client.dispose();
    this.clients.clear();
    this.connectionSettings.clear();
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

  private async autoinstallIfNeeded(cocoonId: string): Promise<void> {
    const settings = this.connectionSettings.get(cocoonId);
    if (!settings?.autoinstallPlugins) return;

    const connection = this.connections.get(cocoonId);
    if (!connection) return;

    try {
      const registryUrls: string[] = this.app.env('DEFAULT_REGISTRY_URLS') ?? [];
      if (registryUrls.length === 0) return;

      const [registryPlugins] = await Promise.all([
        fetchRegistryPlugins(registryUrls),
        connection.refreshPlugins(),
      ]);

      const extensionPlugins = registryPlugins.filter(p => p.pluginTypes.includes('extension'));
      const installedIds = new Set(connection.plugins);
      const missing = extensionPlugins.filter(p => !installedIds.has(p.id));

      if (missing.length === 0) return;

      for (const p of missing) {
        try {
          await connection.installPlugin(p.id);
        } catch {
          // skip failed installs
        }
      }

      await connection.refreshPlugins();
    } catch {
      // auto-install is best-effort
    }
  }
}
