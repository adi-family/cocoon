/**
 * Auto-generated models from TypeSpec.
 * DO NOT EDIT.
 */

import { CredentialType } from './enums';

export interface Credential {
  id: string;
  name: string;
  description?: string;
  credential_type: CredentialType;
  metadata: Record<string, unknown>;
  provider?: string;
  expires_at?: string;
  created_at: string;
  updated_at: string;
  last_used_at?: string;
}

export interface CredentialWithData {
  id: string;
  name: string;
  description?: string;
  credential_type: CredentialType;
  metadata: Record<string, unknown>;
  provider?: string;
  expires_at?: string;
  created_at: string;
  updated_at: string;
  last_used_at?: string;
  data: Record<string, unknown>;
}

export interface CredentialAccessLog {
  id: string;
  credential_id: string;
  user_id: string;
  action: string;
  ip_address?: string;
  user_agent?: string;
  details?: Record<string, unknown>;
  created_at: string;
}

export interface VerifyResult {
  valid: boolean;
  is_expired: boolean;
  expires_at?: string;
}

export interface DeleteResult {
  deleted: boolean;
}
