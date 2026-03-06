export interface CocoonSessionCreatedEvent {
  cocoonId: string;
  sessionId: string;
  cwd: string;
  shell: string;
}

export interface CocoonSessionClosedEvent {
  cocoonId: string;
  sessionId: string;
}

export interface CocoonErrorEvent {
  cocoonId: string;
  code: string;
  message: string;
}

export enum CocoonBusKey {
  SessionCreated = 'cocoon:session-created',
  SessionClosed = 'cocoon:session-closed',
  Error = 'cocoon:error',
}
