import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';

export interface CocoonDebugInfo {
  cocoonId: string;
  sessions: number;
}

@customElement('adi-cocoon-debug')
export class AdiCocoonDebugElement extends LitElement {
  @state() cocoons: CocoonDebugInfo[] = [];

  override createRenderRoot() {
    return this;
  }

  override render() {
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
}
