/**
 * Auto-generated protocol messages from TypeSpec.
 * DO NOT EDIT.
 */

import type { AdiServiceInfo, QueryType, SilkHtmlSpan, SilkSignal, SilkStream } from './types';

export type SignalingMessage =
  // ── silk ──
  | { type: 'silk_create_session'; cwd?: string; env?: Record<string, string>; shell?: string }
  | { type: 'silk_create_session_response'; session_id: string; cwd: string; shell: string }
  | { type: 'silk_execute'; session_id: string; command: string; command_id: string; cols?: number; rows?: number; env?: Record<string, string> }
  | { type: 'silk_input'; session_id: string; command_id: string; data: string }
  | { type: 'silk_resize'; session_id: string; command_id: string; cols: number; rows: number }
  | { type: 'silk_signal'; session_id: string; command_id: string; signal: SilkSignal }
  | { type: 'silk_close_session'; session_id: string }
  | { type: 'silk_command_started'; session_id: string; command_id: string; interactive: boolean }
  | { type: 'silk_output'; session_id: string; command_id: string; stream: SilkStream; data: string; html?: SilkHtmlSpan[] }
  | { type: 'silk_interactive_required'; session_id: string; command_id: string; reason: string; pty_session_id: string }
  | { type: 'silk_pty_output'; session_id: string; command_id: string; pty_session_id: string; data: string }
  | { type: 'silk_command_completed'; session_id: string; command_id: string; exit_code: number; cwd: string }
  | { type: 'silk_session_closed'; session_id: string }
  | { type: 'silk_error'; session_id?: string; command_id?: string; code: string; message: string }

  // ── adi ──
  | { type: 'adi_request'; request_id: string; service: string; method: string; params: unknown }
  | { type: 'adi_success'; request_id: string; service: string; method: string; data: unknown }
  | { type: 'adi_request_error'; request_id: string; service: string; method: string; code: string; message: string }
  | { type: 'adi_service_not_found'; request_id: string; service: string }
  | { type: 'adi_method_not_found'; request_id: string; service: string; method: string; available_methods: string[] }
  | { type: 'adi_stream_chunk'; request_id: string; service: string; method: string; data: unknown; done: boolean }
  | { type: 'adi_list_services'; request_id: string }
  | { type: 'adi_services_list'; request_id: string; services: AdiServiceInfo[] }
  | { type: 'adi_services_changed'; added: string[]; removed: string[]; updated: string[] }
  | { type: 'adi_subscribe'; request_id: string; service: string; event: string; filter?: unknown }
  | { type: 'adi_subscribed'; request_id: string; subscription_id: string; service: string; event: string }
  | { type: 'adi_unsubscribe'; subscription_id: string }
  | { type: 'adi_unsubscribed'; subscription_id: string }
  | { type: 'adi_subscription_event'; subscription_id: string; event: string; data: unknown }
  | { type: 'adi_subscription_error'; request_id: string; code: string; message: string }

  // ── plugin ──
  | { type: 'plugin_install_plugin'; request_id: string; plugin_id: string; registry?: string; version?: string }
  | { type: 'plugin_install_plugin_response'; request_id: string; success: boolean; plugin_id: string; stdout: string; stderr: string }
  | { type: 'plugin_install_error'; request_id: string; plugin_id: string; code: string; message: string }

  // ── webrtc ──
  | { type: 'webrtc_start_session'; session_id: string; device_id: string; user_id?: string; data_channels?: string[] }
  | { type: 'webrtc_offer'; session_id: string; sdp: string }
  | { type: 'webrtc_answer'; session_id: string; sdp: string }
  | { type: 'webrtc_ice_candidate'; session_id: string; candidate: string; sdp_mid?: string; sdp_mline_index?: number }
  | { type: 'webrtc_session_ended'; session_id: string; reason?: string }
  | { type: 'webrtc_data'; session_id: string; channel: string; data: string; binary: boolean }
  | { type: 'webrtc_error'; session_id: string; code: string; message: string }

  // ── query ──
  | { type: 'query_query_local'; query_id: string; query_type: QueryType; params: unknown }
  | { type: 'query_query_result'; query_id: string; data: unknown; is_final: boolean };
