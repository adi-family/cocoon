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

  constructor(commandId: string, sessionId: string, send: SendFn) {
    this.commandId = commandId;
    this.sessionId = sessionId;
    this.send = send;
  }

  input(data: string): void {
    this.send({
      type: 'input',
      session_id: this.sessionId,
      command_id: this.commandId,
      data,
    });
  }

  resize(cols: number, rows: number): void {
    this.send({
      type: 'resize',
      session_id: this.sessionId,
      command_id: this.commandId,
      cols,
      rows,
    });
  }

  signal(signal: SilkSignal): void {
    this.send({
      type: 'signal',
      session_id: this.sessionId,
      command_id: this.commandId,
      signal,
    });
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

  /** @internal */
  _emitOutput(stream: SilkStream, data: string, html?: SilkHtmlSpan[]): void {
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
  }
}
