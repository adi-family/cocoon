import type { SilkRequest, SilkResponse } from './silk-types';
import type { SyncDataSender } from './cocoon-client';
import { SilkCommand } from './silk-command';

let commandCounter = 0;
const nextCommandId = (): string => `cmd-${Date.now()}-${++commandCounter}`;

type Listener<T> = (event: T) => void;

export class SilkSession {
  readonly sessionId: string;
  readonly cocoonId: string;
  readonly cwd: string;
  readonly shell: string;

  private readonly server: SyncDataSender;
  private readonly commands = new Map<string, SilkCommand>();
  private readonly closedListeners: Listener<void>[] = [];
  private _closed = false;

  constructor(
    sessionId: string,
    cocoonId: string,
    cwd: string,
    shell: string,
    server: SyncDataSender,
  ) {
    this.sessionId = sessionId;
    this.cocoonId = cocoonId;
    this.cwd = cwd;
    this.shell = shell;
    this.server = server;
  }

  get closed(): boolean {
    return this._closed;
  }

  execute(command: string, opts?: { commandId?: string; cols?: number; rows?: number; env?: Record<string, string> }): SilkCommand {
    const id = opts?.commandId ?? nextCommandId();
    const cmd = new SilkCommand(id, this.sessionId, (req) => this.sendSilk(req));
    this.commands.set(id, cmd);
    this.sendSilk({
      type: 'execute',
      session_id: this.sessionId,
      command,
      command_id: id,
      cols: opts?.cols,
      rows: opts?.rows,
      env: opts?.env,
    });
    return cmd;
  }

  close(): void {
    if (this._closed) return;
    this.sendSilk({
      type: 'close_session',
      session_id: this.sessionId,
    });
  }

  onClosed(fn: Listener<void>): () => void {
    this.closedListeners.push(fn);
    return () => { const i = this.closedListeners.indexOf(fn); if (i >= 0) this.closedListeners.splice(i, 1); };
  }

  /** @internal Called by CocoonClient to route responses into this session. */
  _handleResponse(response: SilkResponse): void {
    switch (response.type) {
      case 'output': {
        const cmd = this.commands.get(response.command_id);
        if (cmd) cmd._emitOutput(response.stream, response.data, response.html);
        break;
      }
      case 'pty_output': {
        const cmd = this.commands.get(response.command_id);
        if (cmd) cmd._emitPtyOutput(response.data);
        break;
      }
      case 'interactive_required': {
        const cmd = this.commands.get(response.command_id);
        if (cmd) cmd._emitInteractiveRequired(response.reason, response.pty_session_id);
        break;
      }
      case 'command_completed': {
        const cmd = this.commands.get(response.command_id);
        if (cmd) {
          cmd._emitCompleted(response.exit_code, response.cwd);
          cmd.dispose();
          this.commands.delete(response.command_id);
        }
        break;
      }
      case 'error': {
        if (response.command_id) {
          const cmd = this.commands.get(response.command_id);
          if (cmd) cmd._emitError(response.code, response.message);
        }
        break;
      }
      case 'session_closed':
        this._closed = true;
        for (const cmd of this.commands.values()) cmd.dispose();
        this.commands.clear();
        for (const fn of this.closedListeners) fn();
        break;
    }
  }

  dispose(): void {
    for (const cmd of this.commands.values()) cmd.dispose();
    this.commands.clear();
    this.closedListeners.length = 0;
  }

  private sendSilk(request: SilkRequest): void {
    this.server.sendSyncData(request);
  }
}
