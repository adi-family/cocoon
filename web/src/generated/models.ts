/**
 * Auto-generated models from TypeSpec.
 * DO NOT EDIT.
 */

import { SilkStream, SilkSignal, QueryType } from './enums';

export interface SilkHtmlSpan {
  text: string;
  classes?: string[];
  styles?: Record<string, string>;
}

export interface AdiPluginCapabilities {
  subscriptions: boolean;
  notifications: boolean;
  streaming: boolean;
}

export interface AdiMethodInfo {
  name: string;
  description: string;
  streaming: boolean;
  params_schema?: unknown;
  result_schema?: unknown;
  deprecated?: boolean;
  deprecated_message?: string;
}

export interface AdiPluginInfo {
  id: string;
  name: string;
  version: string;
  description?: string;
  methods: AdiMethodInfo[];
  capabilities: AdiPluginCapabilities;
}

export interface PluginInstallResult {
  request_id: string;
  success: boolean;
  plugin_id: string;
  stdout: string;
  stderr: string;
}
