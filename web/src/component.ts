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

export class AdiCocoonListElement extends LitElement {
  @state() cocoons: CocoonListItem[] = [];
  @state() private expanded: string | null = null;

  override createRenderRoot() { return this; }

  private toggleExpand(cocoonId: string): void {
    this.expanded = this.expanded === cocoonId ? null : cocoonId;
  }

  override render() {
    return html`
      <div style="padding: 16px;">
        <h2 style="margin: 0 0 16px; font-size: 1.25rem; font-weight: 600;">Cocoon Clients</h2>

        ${this.cocoons.length === 0
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
