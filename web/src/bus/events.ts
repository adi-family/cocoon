import type {
  CocoonErrorEvent,
  CocoonSessionClosedEvent,
  CocoonSessionCreatedEvent,
} from './types';

declare module '@adi-family/sdk-plugin/types' {
  interface EventRegistry {
    'cocoon:session-created': CocoonSessionCreatedEvent;
    'cocoon:session-closed': CocoonSessionClosedEvent;
    'cocoon:error': CocoonErrorEvent;
  }
}
