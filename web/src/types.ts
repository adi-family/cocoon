/**
 * Auto-generated plugin types.
 * Import via: import '@adi-family/plugin-cocoon'
 * DO NOT EDIT.
 */

import type { CocoonPlugin } from './plugin';

export type { CocoonPlugin };
export * from './config';
export * from './generated';

declare module '@adi-family/sdk-plugin' {
  interface PluginApiRegistry {
    'adi.cocoon': CocoonPlugin['api'];
  }
}
