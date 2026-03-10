import { PLUGIN_ID } from './config';

import './bus';
export * from './bus';
export * from './config';
export * from './generated';
export * from './silk-types';
export * from './silk-command';
export * from './silk-session';
export * from './cocoon-client';
export * from './adi-frame';

import type { CocoonPlugin } from './plugin';
export { CocoonPlugin, CocoonPlugin as PluginShell } from './plugin';

declare module '@adi-family/sdk-plugin' {
  interface PluginApiRegistry {
    [PLUGIN_ID]: CocoonPlugin['api'];
  }
}
