import type { SilkStream, SilkHtmlSpan, SilkSignal, SilkRequest } from './silk-types';

export interface SilkOutputEvent {
  stream: SilkStream;
  data: string;
  html?: SilkHtmlSpan[];
}

export interface SilkCompletedEvent {
  exitCode: number;
  cwd: string;
}

export interface SilkCommandError {
  code: string;
  message: string;
}

export interface SilkPtyOutputEvent {
  data: string;
}

export interface SilkInteractiveRequiredEvent {
  reason: string;
  ptySessionId: string;
}

export interface SilkSelectOption {
  label: string;
  description?: string;
  disabled: boolean;
}

export type SilkInputRequest =
  | { type: 'select'; id: string; prompt: string; options: SilkSelectOption[]; default?: number }
  | { type: 'multi_select'; id: string; prompt: string; options: SilkSelectOption[]; defaults: number[]; min?: number; max?: number }
  | { type: 'confirm'; id: string; prompt: string; default?: boolean }
  | { type: 'input'; id: string; prompt: string; default?: string; placeholder?: string }
  | { type: 'password'; id: string; prompt: string };

const INPUT_REQUEST_TYPES = new Set(['select', 'multi_select', 'confirm', 'input', 'password']);

function tryParseInputRequest(line: string): SilkInputRequest | null {
  const trimmed = line.trim();
  if (!trimmed.startsWith('{')) return null;
  try {
    const obj = JSON.parse(trimmed);
    if (obj && typeof obj.type === 'string' && INPUT_REQUEST_TYPES.has(obj.type) && typeof obj.id === 'string') {
      return obj as SilkInputRequest;
    }
  } catch { /* not JSON */ }
  return null;
}

type Listener<T> = (event: T) => void;
type SendFn = (request: SilkRequest) => void;

export class SilkCommand {
  readonly commandId: string;

  private readonly sessionId: string;
  private readonly send: SendFn;
  private readonly outputListeners: Listener<SilkOutputEvent>[] = [];
  private readonly ptyOutputListeners: Listener<SilkPtyOutputEvent>[] = [];
  private readonly interactiveListeners: Listener<SilkInteractiveRequiredEvent>[] = [];
  private readonly completedListeners: Listener<SilkCompletedEvent>[] = [];
  private readonly errorListeners: Listener<SilkCommandError>[] = [];
  private readonly inputRequestListeners: Listener<SilkInputRequest>[] = [];

  constructor(commandId: string, sessionId: string, send: SendFn) {
    this.commandId = commandId;
    this.sessionId = sessionId;
    this.send = send;
  }

  input(data: string): void {
    this.send({
      type: 'silk_input',
      session_id: this.sessionId,
      command_id: this.commandId,
      data,
    });
  }

  resize(cols: number, rows: number): void {
    this.send({
      type: 'silk_resize',
      session_id: this.sessionId,
      command_id: this.commandId,
      cols,
      rows,
    });
  }

  signal(signal: SilkSignal): void {
    this.send({
      type: 'silk_signal',
      session_id: this.sessionId,
      command_id: this.commandId,
      signal,
    });
  }

  respondToInput(response: string): void {
    this.input(response);
  }

  onOutput(fn: Listener<SilkOutputEvent>): () => void {
    this.outputListeners.push(fn);
    return () => { const i = this.outputListeners.indexOf(fn); if (i >= 0) this.outputListeners.splice(i, 1); };
  }

  onPtyOutput(fn: Listener<SilkPtyOutputEvent>): () => void {
    this.ptyOutputListeners.push(fn);
    return () => { const i = this.ptyOutputListeners.indexOf(fn); if (i >= 0) this.ptyOutputListeners.splice(i, 1); };
  }

  onInteractiveRequired(fn: Listener<SilkInteractiveRequiredEvent>): () => void {
    this.interactiveListeners.push(fn);
    return () => { const i = this.interactiveListeners.indexOf(fn); if (i >= 0) this.interactiveListeners.splice(i, 1); };
  }

  onCompleted(fn: Listener<SilkCompletedEvent>): () => void {
    this.completedListeners.push(fn);
    return () => { const i = this.completedListeners.indexOf(fn); if (i >= 0) this.completedListeners.splice(i, 1); };
  }

  onError(fn: Listener<SilkCommandError>): () => void {
    this.errorListeners.push(fn);
    return () => { const i = this.errorListeners.indexOf(fn); if (i >= 0) this.errorListeners.splice(i, 1); };
  }

  onInputRequest(fn: Listener<SilkInputRequest>): () => void {
    this.inputRequestListeners.push(fn);
    return () => { const i = this.inputRequestListeners.indexOf(fn); if (i >= 0) this.inputRequestListeners.splice(i, 1); };
  }

  /** @internal */
  _emitOutput(stream: SilkStream, data: string, html?: SilkHtmlSpan[]): void {
    if (stream === 'stdout' && this.inputRequestListeners.length > 0) {
      const lines = data.split('\n');
      const nonInputLines: string[] = [];
      for (const line of lines) {
        const req = tryParseInputRequest(line);
        if (req) {
          for (const fn of this.inputRequestListeners) fn(req);
        } else {
          nonInputLines.push(line);
        }
      }
      const remaining = nonInputLines.join('\n');
      if (remaining.trim()) {
        for (const fn of this.outputListeners) fn({ stream, data: remaining, html });
      }
      return;
    }
    for (const fn of this.outputListeners) fn({ stream, data, html });
  }

  /** @internal */
  _emitPtyOutput(data: string): void {
    for (const fn of this.ptyOutputListeners) fn({ data });
  }

  /** @internal */
  _emitInteractiveRequired(reason: string, ptySessionId: string): void {
    for (const fn of this.interactiveListeners) fn({ reason, ptySessionId });
  }

  /** @internal */
  _emitCompleted(exitCode: number, cwd: string): void {
    for (const fn of this.completedListeners) fn({ exitCode, cwd });
  }

  /** @internal */
  _emitError(code: string, message: string): void {
    for (const fn of this.errorListeners) fn({ code, message });
  }

  dispose(): void {
    this.outputListeners.length = 0;
    this.ptyOutputListeners.length = 0;
    this.interactiveListeners.length = 0;
    this.completedListeners.length = 0;
    this.errorListeners.length = 0;
    this.inputRequestListeners.length = 0;
  }
}
