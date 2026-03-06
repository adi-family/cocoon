export type SilkRequest =
  | { type: 'create_session'; cwd?: string; env?: Record<string, string>; shell?: string }
  | { type: 'execute'; session_id: string; command: string; command_id: string; cols?: number; rows?: number; env?: Record<string, string> }
  | { type: 'input'; session_id: string; command_id: string; data: string }
  | { type: 'resize'; session_id: string; command_id: string; cols: number; rows: number }
  | { type: 'signal'; session_id: string; command_id: string; signal: SilkSignal }
  | { type: 'close_session'; session_id: string };

export type SilkSignal = 'interrupt' | 'terminate' | 'kill';

export type SilkStream = 'stdout' | 'stderr';

export interface SilkHtmlSpan {
  text: string;
  classes?: string[];
  styles?: Record<string, string>;
}

export type SilkResponse =
  | { type: 'session_created'; session_id: string; cwd: string; shell: string }
  | { type: 'command_started'; session_id: string; command_id: string; interactive: boolean }
  | { type: 'output'; session_id: string; command_id: string; stream: SilkStream; data: string; html?: SilkHtmlSpan[] }
  | { type: 'interactive_required'; session_id: string; command_id: string; reason: string; pty_session_id: string }
  | { type: 'pty_output'; session_id: string; command_id: string; pty_session_id: string; data: string }
  | { type: 'command_completed'; session_id: string; command_id: string; exit_code: number; cwd: string }
  | { type: 'session_closed'; session_id: string }
  | { type: 'error'; session_id?: string; command_id?: string; code: string; message: string };
