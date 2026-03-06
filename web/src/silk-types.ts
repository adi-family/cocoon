// Re-export generated types as the single source of truth
export type { SilkStream, SilkSignal, SilkHtmlSpan } from './generated';
export type { SignalingMessage as CocoonMessage } from './generated';

// Extract silk-specific message subsets for typed usage
import type { SignalingMessage } from './generated';

type ExtractSilk<T extends SignalingMessage, P extends string> = T extends { type: `silk_${P}` } ? T : never;

export type SilkRequest =
  | ExtractSilk<SignalingMessage, 'create_session'>
  | ExtractSilk<SignalingMessage, 'execute'>
  | ExtractSilk<SignalingMessage, 'input'>
  | ExtractSilk<SignalingMessage, 'resize'>
  | ExtractSilk<SignalingMessage, 'signal'>
  | ExtractSilk<SignalingMessage, 'close_session'>;

export type SilkResponse =
  | ExtractSilk<SignalingMessage, 'create_session_response'>
  | ExtractSilk<SignalingMessage, 'command_started'>
  | ExtractSilk<SignalingMessage, 'output'>
  | ExtractSilk<SignalingMessage, 'interactive_required'>
  | ExtractSilk<SignalingMessage, 'pty_output'>
  | ExtractSilk<SignalingMessage, 'command_completed'>
  | ExtractSilk<SignalingMessage, 'session_closed'>
  | ExtractSilk<SignalingMessage, 'error'>;
