/**
 * Auto-generated protocol types from TypeSpec.
 * DO NOT EDIT.
 */

export enum SilkStream {
  Stdout = "stdout",
  Stderr = "stderr",
}

export enum SilkSignal {
  Interrupt = "interrupt",
  Terminate = "terminate",
  Kill = "kill",
}

export enum QueryType {
  ListTasks = "list_tasks",
  GetTaskStats = "get_task_stats",
  SearchTasks = "search_tasks",
  SearchKnowledgebase = "search_knowledgebase",
}

export interface SilkHtmlSpan {
  text: string;
  classes?: string[];
  styles?: Record<string, string>;
}

export interface AdiServiceCapabilities {
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

export interface AdiServiceInfo {
  id: string;
  name: string;
  version: string;
  description?: string;
  methods: AdiMethodInfo[];
  capabilities: AdiServiceCapabilities;
}

export interface PluginInstallResult {
  request_id: string;
  success: boolean;
  plugin_id: string;
  stdout: string;
  stderr: string;
}
