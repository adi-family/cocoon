/**
 * Binary framing for ADI service protocol.
 *
 * Frame layout: `[header_len: u32 BE][JSON header][payload bytes]`
 *
 * The router reads only the JSON header for routing (plugin, method, request ID).
 * The payload is opaque bytes — each plugin decides its own serialization format.
 */

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export interface RequestHeader {
  v: number;
  id: string;
  plugin: string;
  method: string;
  stream?: boolean;
}

export type ResponseStatus =
  | 'success'
  | 'error'
  | 'plugin_not_found'
  | 'method_not_found'
  | 'stream_chunk'
  | 'stream_end'
  | 'invalid_request';

export interface ResponseHeader {
  v: number;
  id: string;
  status: ResponseStatus;
  seq: number;
}

export interface ParsedResponse {
  header: ResponseHeader;
  payload: Uint8Array;
}

/** Build a binary request frame from header fields and a JSON-serializable params object. */
export function buildRequestFrame(
  requestId: string,
  plugin: string,
  method: string,
  params?: unknown,
  stream = false,
): ArrayBuffer {
  const header: RequestHeader = { v: 1, id: requestId, plugin, method, stream };
  const headerBytes = encoder.encode(JSON.stringify(header));
  const payloadBytes = params != null ? encoder.encode(JSON.stringify(params)) : new Uint8Array(0);

  const frame = new ArrayBuffer(4 + headerBytes.length + payloadBytes.length);
  const view = new DataView(frame);
  view.setUint32(0, headerBytes.length, false); // big-endian
  new Uint8Array(frame, 4, headerBytes.length).set(headerBytes);
  new Uint8Array(frame, 4 + headerBytes.length, payloadBytes.length).set(payloadBytes);
  return frame;
}

/** Parse a binary response frame into header + payload. */
export function parseResponseFrame(data: ArrayBuffer): ParsedResponse {
  const view = new DataView(data);
  const headerLen = view.getUint32(0, false); // big-endian
  const headerJson = decoder.decode(new Uint8Array(data, 4, headerLen));
  const header = JSON.parse(headerJson) as ResponseHeader;
  const payload = new Uint8Array(data, 4 + headerLen);
  return { header, payload };
}

/** Decode a payload as JSON. */
export function decodePayloadJson<T>(payload: Uint8Array): T {
  if (payload.length === 0) return undefined as T;
  return JSON.parse(decoder.decode(payload)) as T;
}

/** Decode a payload as UTF-8 text. */
export function decodePayloadText(payload: Uint8Array): string {
  return decoder.decode(payload);
}
