import { LitElement, html, nothing } from 'lit';
import { state } from 'lit/decorators.js';

export interface CocoonListItem {
  cocoonId: string;
  sessionCount: number;
  sessions: CocoonSessionItem[];
}

export interface CocoonSessionItem {
  sessionId: string;
  cwd: string;
  shell: string;
  closed: boolean;
}

export interface SetupConnectEvent {
  signalingUrl: string;
}

type SetupState = 'idle' | 'scanning' | 'found' | 'connecting' | 'connected' | 'error';

interface SetupMachine {
  name: string;
  version: string;
  signalingUrl?: string;
}

const SETUP_PORT = 14730;
const SETUP_POLL_MS = 2000;

export type AuthTokenProvider = () => Promise<string | null>;

export class AdiCocoonListElement extends LitElement {
  @state() cocoons: CocoonListItem[] = [];
  @state() signalingUrls: string[] = [];
  authTokenProvider: AuthTokenProvider | null = null;
  @state() private expanded: string | null = null;

  @state() private setupState: SetupState = 'idle';
  @state() private setupMachine: SetupMachine | null = null;
  @state() private setupError = '';
  @state() private setupUrl = '';

  private pollTimer: ReturnType<typeof setInterval> | null = null;

  override createRenderRoot() { return this; }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.stopPolling();
  }

  private toggleExpand(cocoonId: string): void {
    this.expanded = this.expanded === cocoonId ? null : cocoonId;
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
      const authToken = this.authTokenProvider ? await this.authTokenProvider() : null;

      const resp = await fetch(`http://localhost:${SETUP_PORT}/connect`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          token: '',
          auth_token: authToken ?? '',
          signaling_url: this.setupUrl.trim(),
        }),
      });

      const data = await resp.json();
      if (data.status === 'connecting' || data.status === 'already_connected') {
        this.setupState = 'connected';
        this.dispatchEvent(new CustomEvent<SetupConnectEvent>('setup-connect', {
          detail: { signalingUrl: this.setupUrl.trim() },
          bubbles: true,
          composed: true,
        }));
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
    const btnPrimary = 'padding: 8px 16px; border: none; border-radius: 6px; background: var(--brand, #6366f1); color: white; font-size: 0.85rem; cursor: pointer;';
    const btnSecondary = 'padding: 8px 16px; border: 1px solid var(--border-color, #333); border-radius: 6px; background: transparent; color: inherit; font-size: 0.85rem; cursor: pointer;';

    return html`
      <div style="padding: 16px;">
        <div style="display: flex; align-items: center; justify-content: space-between; margin-bottom: 16px;">
          <h2 style="margin: 0; font-size: 1.25rem; font-weight: 600;">Cocoons</h2>
          ${this.setupState === 'idle' ? html`
            <button style="${btnPrimary}" @click=${() => this.startSetup()}>+ Setup Cocoon</button>
          ` : this.setupState !== 'connected' ? html`
            <button style="${btnSecondary}" @click=${() => this.cancelSetup()}>Cancel</button>
          ` : nothing}
        </div>

        ${this.renderSetup()}

        ${this.cocoons.length === 0 && this.setupState === 'idle'
          ? html`<p style="color: var(--text-muted, #888);">No cocoon clients connected.</p>`
          : html`
            <div style="display: flex; flex-direction: column; gap: 8px;">
              ${this.cocoons.map(c => this.renderCocoonCard(c))}
            </div>
          `
        }
      </div>
    `;
  }

  private renderSetup() {
    if (this.setupState === 'idle') return nothing;

    const cardStyle = 'border: 1px solid var(--border-color, #333); border-radius: 8px; padding: 16px; margin-bottom: 16px;';
    const inputStyle = 'width: 100%; padding: 8px 12px; border: 1px solid var(--border-color, #333); border-radius: 6px; background: var(--bg-input, #1a1a2e); color: inherit; font-size: 0.85rem; box-sizing: border-box;';
    const btnPrimary = 'padding: 8px 16px; border: none; border-radius: 6px; background: var(--brand, #6366f1); color: white; font-size: 0.85rem; cursor: pointer;';

    if (this.setupState === 'scanning') {
      return html`
        <div style="${cardStyle}">
          <div style="display: flex; flex-direction: column; gap: 12px;">
            <div style="display: flex; align-items: center; gap: 8px;">
              <span style="animation: spin 1s linear infinite; display: inline-block;">&#9696;</span>
              <span>Scanning for local cocoon setup server...</span>
            </div>
            <p style="margin: 0; color: var(--text-muted, #888); font-size: 0.85rem;">
              Run <code style="background: var(--bg-input, #1a1a2e); padding: 2px 6px; border-radius: 4px;">adi cocoon setup</code> on your machine to start the pairing server.
            </p>
          </div>
          <style>@keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }</style>
        </div>
      `;
    }

    if (this.setupState === 'found' && this.setupMachine) {
      return html`
        <div style="${cardStyle}">
          <div style="display: flex; flex-direction: column; gap: 12px;">
            <div style="display: flex; align-items: center; gap: 8px;">
              <span style="color: var(--text-success, #4ade80);">&#9679;</span>
              <span style="font-weight: 500;">Found: ${this.setupMachine.name}</span>
              ${this.setupMachine.version ? html`
                <span style="font-size: 0.75rem; color: var(--text-muted, #888);">v${this.setupMachine.version}</span>
              ` : nothing}
            </div>

            ${!this.setupMachine.signalingUrl ? html`
              <div>
                <label style="display: block; font-size: 0.8rem; color: var(--text-muted, #888); margin-bottom: 4px;">Signaling Server</label>
                ${this.signalingUrls.length > 0
                  ? html`
                    <select
                      style="${inputStyle}"
                      .value=${this.setupUrl}
                      @change=${(e: Event) => { this.setupUrl = (e.target as HTMLSelectElement).value; }}
                    >
                      ${this.signalingUrls.map(url => html`<option .value=${url}>${url}</option>`)}
                    </select>
                  `
                  : html`
                    <input
                      style="${inputStyle}"
                      placeholder="ws://localhost:8080/ws"
                      .value=${this.setupUrl}
                      @input=${(e: InputEvent) => { this.setupUrl = (e.target as HTMLInputElement).value; }}
                    />
                  `
                }
              </div>
            ` : nothing}

            ${this.setupError ? html`<p style="margin: 0; color: var(--text-error, #f87171); font-size: 0.85rem;">${this.setupError}</p>` : nothing}

            <div style="display: flex; justify-content: flex-end;">
              <button style="${btnPrimary}" @click=${() => this.connectSetup()}>Connect</button>
            </div>
          </div>
        </div>
      `;
    }

    if (this.setupState === 'connecting') {
      return html`
        <div style="${cardStyle}">
          <div style="display: flex; align-items: center; gap: 8px;">
            <span style="animation: spin 1s linear infinite; display: inline-block;">&#9696;</span>
            <span>Connecting ${this.setupMachine?.name ?? ''}...</span>
          </div>
          <style>@keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }</style>
        </div>
      `;
    }

    if (this.setupState === 'connected') {
      return html`
        <div style="${cardStyle}">
          <div style="display: flex; align-items: center; gap: 8px;">
            <span style="color: var(--text-success, #4ade80);">&#10003;</span>
            <span>${this.setupMachine?.name ?? 'Machine'} connected! Cocoon is starting...</span>
          </div>
        </div>
      `;
    }

    if (this.setupState === 'error') {
      return html`
        <div style="${cardStyle}">
          <div style="display: flex; flex-direction: column; gap: 8px;">
            <div style="display: flex; align-items: center; gap: 8px;">
              <span style="color: var(--text-error, #f87171);">&#10007;</span>
              <span>Setup failed: ${this.setupError}</span>
            </div>
            <div>
              <button style="${'padding: 6px 12px; border: 1px solid var(--border-color, #333); border-radius: 6px; background: transparent; color: inherit; font-size: 0.85rem; cursor: pointer;'}" @click=${() => this.startSetup()}>Retry</button>
            </div>
          </div>
        </div>
      `;
    }

    return nothing;
  }

  private renderCocoonCard(item: CocoonListItem) {
    const isExpanded = this.expanded === item.cocoonId;

    return html`
      <div style="border: 1px solid var(--border-color, #333); border-radius: 8px; overflow: hidden;">
        <div
          style="display: flex; align-items: center; justify-content: space-between; padding: 12px 16px; cursor: pointer; user-select: none;"
          @click=${() => this.toggleExpand(item.cocoonId)}
        >
          <div style="display: flex; align-items: center; gap: 8px;">
            <span style="font-size: 0.75rem; color: var(--text-success, #4ade80);">&#9679;</span>
            <span style="font-weight: 500;">${item.cocoonId}</span>
          </div>
          <div style="display: flex; align-items: center; gap: 12px;">
            <span style="font-size: 0.85rem; color: var(--text-muted, #888);">
              ${item.sessionCount} session${item.sessionCount !== 1 ? 's' : ''}
            </span>
            <span style="font-size: 0.75rem; color: var(--text-muted, #888);">${isExpanded ? '\u25B2' : '\u25BC'}</span>
          </div>
        </div>

        ${isExpanded ? html`
          <div style="border-top: 1px solid var(--border-color, #333); padding: 12px 16px;">
            ${item.sessions.length === 0
              ? html`<p style="margin: 0; color: var(--text-muted, #888); font-size: 0.85rem;">No active sessions.</p>`
              : html`
                <table style="width: 100%; border-collapse: collapse; font-size: 0.85rem;">
                  <thead>
                    <tr style="color: var(--text-muted, #888); text-align: left;">
                      <th style="padding: 4px 8px;">Session ID</th>
                      <th style="padding: 4px 8px;">CWD</th>
                      <th style="padding: 4px 8px;">Shell</th>
                      <th style="padding: 4px 8px;">Status</th>
                    </tr>
                  </thead>
                  <tbody>
                    ${item.sessions.map(s => html`
                      <tr>
                        <td style="padding: 4px 8px; font-family: monospace; font-size: 0.8rem;">${s.sessionId}</td>
                        <td style="padding: 4px 8px; font-family: monospace; font-size: 0.8rem;">${s.cwd}</td>
                        <td style="padding: 4px 8px;">${s.shell}</td>
                        <td style="padding: 4px 8px;">
                          <span style="color: ${s.closed ? 'var(--text-error, #f87171)' : 'var(--text-success, #4ade80)'};">
                            ${s.closed ? 'closed' : 'active'}
                          </span>
                        </td>
                      </tr>
                    `)}
                  </tbody>
                </table>
              `
            }
          </div>
        ` : nothing}
      </div>
    `;
  }
}
