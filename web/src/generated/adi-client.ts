/**
 * Auto-generated ADI service client from TypeSpec.
 * DO NOT EDIT.
 */
import type { Connection } from '@adi-family/cocoon-plugin-interface';
import type { PluginInstallResult } from './models.js';

const SVC = 'silk';

export const createSession = (c: Connection, params?: { cwd?: string; env?: Record<string, string>; shell?: string; }) =>
  c.request<unknown>(SVC, 'create_session', params ?? {});

const SVC = 'plugin';

export const installPlugin = (c: Connection, params: { request_id: string; plugin_id: string; registry?: string; version?: string; }) =>
  c.request<PluginInstallResult>(SVC, 'install_plugin', params);
