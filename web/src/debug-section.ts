import { LitElement, html, nothing } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import type { DeviceInfo } from '@adi-family/plugin-signaling/bus';

export interface CocoonDebugInfo {
  cocoonId: string;
  sessions: number;
}

export interface CocoonDeviceInfo {
  deviceId: string;
  name?: string;
  online: boolean;
  protocols: string[];
  adiServices: string[];
}

export type SubtokenProvider = () => Promise<string | null>;

type SetupState = 'idle' | 'scanning' | 'found' | 'connecting' | 'connected' | 'error';

interface SetupMachine {
  name: string;
  version: string;
  signalingUrl?: string;
}

const SETUP_PORT = 14730;
const SETUP_POLL_MS = 2000;

@customElement('adi-cocoon-debug')
export class AdiCocoonDebugElement extends LitElement {
  @state() cocoons: CocoonDebugInfo[] = [];
  @state() connectedCocoons: CocoonDeviceInfo[] = [];
  @state() signalingUrls: string[] = [];
  subtokenProvider: SubtokenProvider | null = null;

  @state() private setupState: SetupState = 'idle';
  @state() private setupMachine: SetupMachine | null = null;
  @state() private setupError = '';
  @state() private setupUrl = '';

  private pollTimer: ReturnType<typeof setInterval> | null = null;

  override createRenderRoot() {
    return this;
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.stopPolling();
  }

  private startSetup(): void {
    this.setupState = 'scanning';
    this.setupError = '';
    this.setupMachine = null;
    if (this.signalingUrls.length > 0 && !this.setupUrl) {
      this.setupUrl = this.signalingUrls[0];
    }
    this.pollForSetupServer();
    this.pollTimer = setInterval(() => this.pollForSetupServer(), SETUP_POLL_MS);
  }

  private cancelSetup(): void {
    this.stopPolling();
    this.setupState = 'idle';
    this.setupMachine = null;
    this.setupError = '';
  }

  private stopPolling(): void {
    if (this.pollTimer) {
      clearInterval(this.pollTimer);
      this.pollTimer = null;
    }
  }

  private async pollForSetupServer(): Promise<void> {
    try {
      const resp = await fetch(`http://localhost:${SETUP_PORT}/health`, {
        signal: AbortSignal.timeout(1500),
      });
      if (!resp.ok) return;
      const data = await resp.json();
      if (data.status === 'ok') {
        this.stopPolling();
        this.setupMachine = {
          name: data.name || 'Unknown Machine',
          version: data.version || '',
          signalingUrl: data.signaling_url,
        };
        if (data.signaling_url) {
          this.setupUrl = data.signaling_url;
        }
        this.setupState = 'found';
      }
    } catch {
      // Not found yet, keep polling
    }
  }

  private async connectSetup(): Promise<void> {
    if (!this.setupUrl.trim()) {
      this.setupError = 'Signaling URL is required';
      return;
    }

    this.setupState = 'connecting';
    this.setupError = '';

    try {
      const setupToken = this.subtokenProvider ? await this.subtokenProvider() : null;

      const resp = await fetch(`http://localhost:${SETUP_PORT}/connect`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          token: setupToken ?? '',
          signaling_url: this.setupUrl.trim(),
        }),
      });

      const data = await resp.json();
      if (data.status === 'connecting' || data.status === 'already_connected') {
        this.setupState = 'connected';
        setTimeout(() => {
          this.setupState = 'idle';
          this.setupMachine = null;
        }, 3000);
      } else {
        this.setupState = 'error';
        this.setupError = data.message || 'Connection failed';
      }
    } catch (e) {
      this.setupState = 'error';
      this.setupError = e instanceof Error ? e.message : 'Connection failed';
    }
  }

  override render() {
    return html`
      ${this.renderConnectedCocoons()}
      ${this.renderSetup()}
      ${this.renderCocoonTable()}
    `;
  }

  updateDevices(devices: DeviceInfo[]): void {
    this.connectedCocoons = devices
      .filter((d) => d.device_type === 'cocoon')
      .map((d) => {
        const config = d.device_config as { adi_services?: string[]; protocols?: string[] } | undefined;
        return {
          deviceId: d.device_id,
          name: d.tags?.name,
          online: d.online,
          protocols: config?.protocols ?? [],
          adiServices: config?.adi_services ?? [],
        };
      });
  }

  private renderConnectedCocoons() {
    if (this.connectedCocoons.length === 0) return nothing;

    return html`
      <div style="margin-bottom: 8px;">
        <div class="text-xs uppercase" style="color:var(--adi-text-muted);font-weight:600;margin-bottom:4px">
          Connected Cocoons (${this.connectedCocoons.length})
        </div>
        ${this.connectedCocoons.map((c) => html`
          <div style="border:1px solid var(--adi-border, #333);border-radius:4px;padding:6px 8px;margin-bottom:4px;font-size:12px;">
            <div style="display:flex;align-items:center;gap:6px">
              ${c.online
                ? html`<span style="color:var(--adi-accent)">●</span>`
                : html`<span style="color:var(--adi-text-muted)">●</span>`}
              <code style="font-size:0.75rem">${c.deviceId.slice(0, 12)}…</code>
              ${c.name ? html`<span>${c.name}</span>` : nothing}
            </div>
            ${c.protocols.length > 0
              ? html`<div style="margin-top:3px;color:var(--adi-text-muted)">protocols: ${c.protocols.join(', ')}</div>`
              : nothing}
            ${c.adiServices.length > 0
              ? html`<div style="margin-top:2px;color:var(--adi-text-muted)">services: ${c.adiServices.join(', ')}</div>`
              : nothing}
          </div>
        `)}
      </div>
    `;
  }

  private renderCocoonTable() {
    if (this.cocoons.length === 0) {
      return html`<div style="color: var(--adi-text-muted, #888); padding: 8px;">No cocoon clients connected</div>`;
    }

    return html`
      <table style="width: 100%; border-collapse: collapse; font-size: 13px;">
        <thead>
          <tr style="border-bottom: 1px solid var(--adi-border, #333);">
            <th style="text-align: left; padding: 4px 8px;">Cocoon ID</th>
            <th style="text-align: right; padding: 4px 8px;">Sessions</th>
          </tr>
        </thead>
        <tbody>
          ${this.cocoons.map(
            (c) => html`
              <tr style="border-bottom: 1px solid var(--adi-border, #222);">
                <td style="padding: 4px 8px; font-family: monospace;">${c.cocoonId}</td>
                <td style="padding: 4px 8px; text-align: right;">${c.sessions}</td>
              </tr>
            `,
          )}
        </tbody>
      </table>
    `;
  }

  private renderSetup() {
    const btnStyle = 'padding: 4px 12px; border: 1px solid var(--adi-border, #333); border-radius: 4px; background: transparent; color: inherit; font-size: 12px; cursor: pointer; margin-bottom: 8px;';
    const btnPrimary = 'padding: 4px 12px; border: none; border-radius: 4px; background: var(--brand, #6366f1); color: white; font-size: 12px; cursor: pointer;';
    const inputStyle = 'width: 100%; padding: 4px 8px; border: 1px solid var(--adi-border, #333); border-radius: 4px; background: var(--bg-input, #1a1a2e); color: inherit; font-size: 12px; box-sizing: border-box;';

    if (this.setupState === 'idle') {
      return html`<button style="${btnStyle}" @click=${() => this.startSetup()}>Setup Manual Cocoon</button>`;
    }

    if (this.setupState === 'scanning') {
      return html`
        <div style="padding: 8px; border: 1px solid var(--adi-border, #333); border-radius: 4px; margin-bottom: 8px; font-size: 12px;">
          <div style="display: flex; align-items: center; gap: 6px; margin-bottom: 4px;">
            <span style="animation: spin 1s linear infinite; display: inline-block;">&#9696;</span>
            <span>Scanning for setup server...</span>
            <button style="margin-left: auto; padding: 2px 8px; border: 1px solid var(--adi-border, #333); border-radius: 4px; background: transparent; color: inherit; font-size: 11px; cursor: pointer;" @click=${() => this.cancelSetup()}>Cancel</button>
          </div>
          <div style="color: var(--adi-text-muted, #888);">
            Run <code style="background: var(--bg-input, #1a1a2e); padding: 1px 4px; border-radius: 3px;">adi cocoon setup</code> on the target machine.
          </div>
          <style>@keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }</style>
        </div>
      `;
    }

    if (this.setupState === 'found' && this.setupMachine) {
      return html`
        <div style="padding: 8px; border: 1px solid var(--adi-border, #333); border-radius: 4px; margin-bottom: 8px; font-size: 12px;">
          <div style="display: flex; align-items: center; gap: 6px; margin-bottom: 8px;">
            <span style="color: var(--text-success, #4ade80);">&#9679;</span>
            <span style="font-weight: 500;">Found: ${this.setupMachine.name}</span>
            ${this.setupMachine.version ? html`<span style="color: var(--adi-text-muted, #888); font-size: 11px;">v${this.setupMachine.version}</span>` : nothing}
            <button style="margin-left: auto; padding: 2px 8px; border: 1px solid var(--adi-border, #333); border-radius: 4px; background: transparent; color: inherit; font-size: 11px; cursor: pointer;" @click=${() => this.cancelSetup()}>Cancel</button>
          </div>

          ${!this.setupMachine.signalingUrl ? html`
            <div style="margin-bottom: 8px;">
              <label style="display: block; font-size: 11px; color: var(--adi-text-muted, #888); margin-bottom: 2px;">Signaling Server</label>
              ${this.signalingUrls.length > 0
                ? html`
                  <select style="${inputStyle}" .value=${this.setupUrl} @change=${(e: Event) => { this.setupUrl = (e.target as HTMLSelectElement).value; }}>
                    ${this.signalingUrls.map(url => html`<option .value=${url}>${url}</option>`)}
                  </select>
                `
                : html`
                  <input style="${inputStyle}" placeholder="ws://localhost:8080/ws" .value=${this.setupUrl} @input=${(e: InputEvent) => { this.setupUrl = (e.target as HTMLInputElement).value; }} />
                `
              }
            </div>
          ` : nothing}

          ${this.setupError ? html`<div style="color: var(--text-error, #f87171); margin-bottom: 4px;">${this.setupError}</div>` : nothing}

          <div style="display: flex; justify-content: flex-end;">
            <button style="${btnPrimary}" @click=${() => this.connectSetup()}>Connect</button>
          </div>
        </div>
      `;
    }

    if (this.setupState === 'connecting') {
      return html`
        <div style="padding: 8px; border: 1px solid var(--adi-border, #333); border-radius: 4px; margin-bottom: 8px; font-size: 12px;">
          <span style="animation: spin 1s linear infinite; display: inline-block;">&#9696;</span>
          Connecting ${this.setupMachine?.name ?? ''}...
          <style>@keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }</style>
        </div>
      `;
    }

    if (this.setupState === 'connected') {
      return html`
        <div style="padding: 8px; border: 1px solid var(--adi-border, #333); border-radius: 4px; margin-bottom: 8px; font-size: 12px;">
          <span style="color: var(--text-success, #4ade80);">&#10003;</span>
          ${this.setupMachine?.name ?? 'Machine'} connected! Cocoon is starting...
        </div>
      `;
    }

    if (this.setupState === 'error') {
      return html`
        <div style="padding: 8px; border: 1px solid var(--adi-border, #333); border-radius: 4px; margin-bottom: 8px; font-size: 12px;">
          <div style="display: flex; align-items: center; gap: 6px; margin-bottom: 4px;">
            <span style="color: var(--text-error, #f87171);">&#10007;</span>
            <span>Setup failed: ${this.setupError}</span>
          </div>
          <button style="${btnStyle}" @click=${() => this.startSetup()}>Retry</button>
        </div>
      `;
    }

    return nothing;
  }
}
