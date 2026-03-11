import type { EventBus } from '@adi-family/sdk-plugin';
import type { SyncDataSender } from './cocoon-client';

const SOURCE = 'cocoon-webrtc';
const CONNECT_TIMEOUT_MS = 30_000;

export interface WebRTCConfig {
  iceServers?: RTCIceServer[];
}

/** Manages a WebRTC peer connection to a cocoon device for silk and adi channel traffic. */
export class CocoonWebRTC {
  private pc: RTCPeerConnection | null = null;
  private silkDc: RTCDataChannel | null = null;
  private adiDc: RTCDataChannel | null = null;
  private readonly sessionId = typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : Array.from(crypto.getRandomValues(new Uint8Array(16)), b => b.toString(16).padStart(2, '0')).join('-');
  private readonly msgHandlers: ((msg: unknown) => void)[] = [];
  private readonly adiMsgHandlers: ((msg: unknown) => void)[] = [];
  private readonly adiBinaryHandlers: ((data: ArrayBuffer) => void)[] = [];
  private connectPromise: Promise<void> | null = null;
  private pendingIceCandidates: RTCIceCandidateInit[] = [];
  private signalUnsub: (() => void) | null = null;
  private answerResolve: (() => void) | null = null;
  private answerReject: ((err: Error) => void) | null = null;

  constructor(
    private readonly cocoonId: string,
    private readonly server: SyncDataSender,
    private readonly bus: EventBus,
    private readonly rtcConfig?: WebRTCConfig,
    private readonly userId?: string,
  ) {}

  get connected(): boolean {
    return this.silkDc?.readyState === 'open' && this.adiDc?.readyState === 'open';
  }

  connect(): Promise<void> {
    if (this.connectPromise) return this.connectPromise;
    this.connectPromise = this.doConnect();
    return this.connectPromise;
  }

  send(msg: unknown): void {
    const state = this.silkDc?.readyState;
    if (state === 'open') {
      const json = JSON.stringify(msg);
      console.log(`[CocoonWebRTC] send: ${json.slice(0, 200)} (${json.length} bytes) dcState=${state}`);
      this.silkDc!.send(json);
    } else {
      console.error(`[CocoonWebRTC] send FAILED: DC not open (state=${state})`);
    }
  }

  onMessage(handler: (msg: unknown) => void): () => void {
    this.msgHandlers.push(handler);
    return () => {
      const i = this.msgHandlers.indexOf(handler);
      if (i >= 0) this.msgHandlers.splice(i, 1);
    };
  }

  sendAdi(msg: unknown): void {
    const state = this.adiDc?.readyState;
    if (state === 'open') {
      const json = JSON.stringify(msg);
      console.log(`[CocoonWebRTC] sendAdi: ${json.slice(0, 200)} (${json.length} bytes) dcState=${state}`);
      this.adiDc!.send(json);
    } else {
      console.error(`[CocoonWebRTC] sendAdi FAILED: adi DC not open (state=${state})`);
    }
  }

  onAdiMessage(handler: (msg: unknown) => void): () => void {
    this.adiMsgHandlers.push(handler);
    return () => {
      const i = this.adiMsgHandlers.indexOf(handler);
      if (i >= 0) this.adiMsgHandlers.splice(i, 1);
    };
  }

  /** Send a binary frame on the adi data channel. */
  sendAdiBinary(data: ArrayBuffer): void {
    const state = this.adiDc?.readyState;
    if (state === 'open') {
      this.adiDc!.send(data);
    } else {
      console.error(`[CocoonWebRTC] sendAdiBinary FAILED: adi DC not open (state=${state})`);
    }
  }

  /** Register a handler for binary messages on the adi data channel. */
  onAdiBinaryMessage(handler: (data: ArrayBuffer) => void): () => void {
    this.adiBinaryHandlers.push(handler);
    return () => {
      const i = this.adiBinaryHandlers.indexOf(handler);
      if (i >= 0) this.adiBinaryHandlers.splice(i, 1);
    };
  }

  dispose(): void {
    this.signalUnsub?.();
    this.signalUnsub = null;
    this.answerReject?.(new Error('CocoonWebRTC disposed'));
    this.answerResolve = null;
    this.answerReject = null;
    this.adiDc?.close();
    this.silkDc?.close();
    this.pc?.close();
    this.adiDc = null;
    this.silkDc = null;
    this.pc = null;
    this.connectPromise = null;
  }

  private async doConnect(): Promise<void> {
    const iceServers = this.rtcConfig?.iceServers ?? [{ urls: 'stun:stun.l.google.com:19302' }];
    console.log(`[CocoonWebRTC] doConnect START session=${this.sessionId} cocoon=${this.cocoonId}`);
    console.log(`[CocoonWebRTC] ICE servers:`, iceServers);
    this.pc = new RTCPeerConnection({ iceServers });
    this.silkDc = this.pc.createDataChannel('silk');
    this.adiDc = this.pc.createDataChannel('adi');
    console.log(`[CocoonWebRTC] PC created, silk+adi DC created`);

    this.pc.onconnectionstatechange = () => {
      console.warn(`[CocoonWebRTC] PC connectionState=${this.pc!.connectionState} session=${this.sessionId}`);
    };
    this.pc.oniceconnectionstatechange = () => {
      console.warn(`[CocoonWebRTC] PC iceConnectionState=${this.pc!.iceConnectionState} session=${this.sessionId}`);
    };
    this.pc.onicegatheringstatechange = () => {
      console.log(`[CocoonWebRTC] PC iceGatheringState=${this.pc!.iceGatheringState} session=${this.sessionId}`);
    };
    this.pc.onsignalingstatechange = () => {
      console.log(`[CocoonWebRTC] PC signalingState=${this.pc!.signalingState} session=${this.sessionId}`);
    };

    this.silkDc.onopen = () => {
      console.warn(`[CocoonWebRTC] silk DC OPENED session=${this.sessionId}`);
    };
    this.silkDc.onclose = () => {
      console.warn(`[CocoonWebRTC] silk DC CLOSED session=${this.sessionId}`);
    };
    this.silkDc.onerror = (e) => {
      console.error(`[CocoonWebRTC] silk DC ERROR session=${this.sessionId}`, e);
    };

    this.silkDc.binaryType = 'arraybuffer';
    this.silkDc.onmessage = (e) => {
      try {
        const text = typeof e.data === 'string'
          ? e.data
          : new TextDecoder().decode(e.data as ArrayBuffer);
        const msg = JSON.parse(text) as unknown;
        for (const fn of this.msgHandlers) fn(msg);
      } catch {
        // ignore parse errors
      }
    };

    this.adiDc.onopen = () => {
      console.warn(`[CocoonWebRTC] adi DC OPENED session=${this.sessionId}`);
    };
    this.adiDc.onclose = () => {
      console.warn(`[CocoonWebRTC] adi DC CLOSED session=${this.sessionId}`);
    };
    this.adiDc.onerror = (e) => {
      console.error(`[CocoonWebRTC] adi DC ERROR session=${this.sessionId}`, e);
    };
    this.adiDc.binaryType = 'arraybuffer';
    this.adiDc.onmessage = (e) => {
      try {
        if (typeof e.data === 'string') {
          // Text frame: JSON discovery/subscription messages
          const msg = JSON.parse(e.data) as unknown;
          for (const fn of this.adiMsgHandlers) fn(msg);
        } else {
          // Binary frame: ADI binary protocol
          const buf = e.data as ArrayBuffer;
          for (const fn of this.adiBinaryHandlers) fn(buf);
        }
      } catch {
        // ignore parse errors
      }
    };

    // Buffer local ICE candidates until webrtc_start_session + webrtc_offer are sent.
    // onicecandidate fires as soon as setLocalDescription is called, which is before
    // the cocoon knows about this session — sending early causes "Session not found".
    const localCandidateQueue: unknown[] = [];
    let sessionStartSent = false;

    this.pc.onicecandidate = (e) => {
      if (!e.candidate) {
        console.log(`[CocoonWebRTC] ICE gathering complete (null candidate) session=${this.sessionId}`);
        return;
      }
      console.log(`[CocoonWebRTC] local ICE candidate: ${e.candidate.candidate.slice(0, 80)}... session=${this.sessionId}`);
      const msg = {
        to: this.cocoonId,
        data: {
          type: 'webrtc_ice_candidate',
          session_id: this.sessionId,
          candidate: e.candidate.candidate,
          sdp_mid: e.candidate.sdpMid ?? undefined,
          sdp_mline_index: e.candidate.sdpMLineIndex ?? undefined,
        },
      };
      if (sessionStartSent) {
        this.server.sendSyncData(msg);
      } else {
        localCandidateQueue.push(msg);
      }
    };

    // Subscribe to signaling messages for WebRTC answer + ICE from cocoon
    this.signalUnsub = this.bus.on(
      'adi.signaling:sync-data',
      ({ url, payload }: { url: string; payload: unknown }) => {
        if (url !== this.server.url) return;
        void this.handleSignalingMsg(payload);
      },
      SOURCE,
    );

    const offer = await this.pc.createOffer();
    await this.pc.setLocalDescription(offer);
    console.log(`[CocoonWebRTC] offer created & local description set, sdpLen=${offer.sdp?.length}`);

    // Notify cocoon to prepare WebRTC session
    console.log(`[CocoonWebRTC] sending webrtc_start_session to ${this.cocoonId}`);
    this.server.sendSyncData({
      to: this.cocoonId,
      data: {
        type: 'webrtc_start_session',
        session_id: this.sessionId,
        device_id: this.cocoonId,
        user_id: this.userId,
        data_channels: ['silk', 'adi'],
      },
    });

    // Send the offer
    console.log(`[CocoonWebRTC] sending webrtc_offer to ${this.cocoonId}`);
    this.server.sendSyncData({
      to: this.cocoonId,
      data: { type: 'webrtc_offer', session_id: this.sessionId, sdp: offer.sdp },
    });

    // Flush candidates buffered during offer creation; future ones go directly
    sessionStartSent = true;
    console.log(`[CocoonWebRTC] flushing ${localCandidateQueue.length} buffered ICE candidates`);
    for (const msg of localCandidateQueue) this.server.sendSyncData(msg);
    localCandidateQueue.length = 0;

    console.log(`[CocoonWebRTC] waiting for answer + DC open...`);
    await Promise.all([this.waitForAnswer(), this.waitForDcOpen()]);
    console.log(`[CocoonWebRTC] doConnect COMPLETE — answer received & DC open!`);
  }

  private waitForAnswer(): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.answerResolve = null;
        this.answerReject = null;
        reject(new Error('WebRTC answer timeout'));
      }, CONNECT_TIMEOUT_MS);

      this.answerResolve = () => { clearTimeout(timer); resolve(); };
      this.answerReject = (err) => { clearTimeout(timer); reject(err); };
    });
  }

  private waitForDcOpen(): Promise<void> {
    const waitForChannel = (dc: RTCDataChannel | null, name: string): Promise<void> =>
      new Promise<void>((resolve, reject) => {
        if (!dc) { reject(new Error(`No ${name} data channel`)); return; }
        if (dc.readyState === 'open') { resolve(); return; }

        const timer = setTimeout(() => reject(new Error(`${name} data channel open timeout`)), CONNECT_TIMEOUT_MS);
        dc.onopen = () => { clearTimeout(timer); resolve(); };
        dc.onerror = (e) => { clearTimeout(timer); reject(e); };
      });

    return Promise.all([
      waitForChannel(this.silkDc, 'silk'),
      waitForChannel(this.adiDc, 'adi'),
    ]).then(() => {});
  }

  private async handleSignalingMsg(payload: unknown): Promise<void> {
    if (!payload || typeof payload !== 'object') return;
    const msg = payload as Record<string, unknown>;
    if (msg['session_id'] !== this.sessionId) return;

    console.log(`[CocoonWebRTC] signaling msg: type=${msg['type']} session=${this.sessionId}`);

    switch (msg['type']) {
      case 'webrtc_answer': {
        console.log(`[CocoonWebRTC] received answer, setting remote description...`);
        await this.pc!.setRemoteDescription({ type: 'answer', sdp: msg['sdp'] as string });
        console.log(`[CocoonWebRTC] remote description set, flushing ${this.pendingIceCandidates.length} pending ICE candidates`);
        for (const c of this.pendingIceCandidates) await this.pc!.addIceCandidate(c);
        this.pendingIceCandidates = [];
        this.answerResolve?.();
        this.answerResolve = null;
        this.answerReject = null;
        break;
      }
      case 'webrtc_ice_candidate': {
        const candidate: RTCIceCandidateInit = {
          candidate: msg['candidate'] as string,
          sdpMid: (msg['sdp_mid'] as string | undefined) ?? null,
          sdpMLineIndex: (msg['sdp_mline_index'] as number | undefined) ?? null,
        };
        console.log(`[CocoonWebRTC] remote ICE candidate: ${candidate.candidate?.slice(0, 60)}...`);
        if (this.pc!.remoteDescription) {
          await this.pc!.addIceCandidate(candidate);
        } else {
          console.log(`[CocoonWebRTC] buffering ICE candidate (no remote desc yet)`);
          this.pendingIceCandidates.push(candidate);
        }
        break;
      }
      case 'webrtc_session_ended': {
        const reason = msg['reason'] as string | undefined;
        console.error(`[CocoonWebRTC] session ENDED by cocoon! reason=${reason} session=${this.sessionId}`);
        this.answerReject?.(new Error(`WebRTC session ended: ${reason ?? 'unknown'}`));
        this.answerResolve = null;
        this.answerReject = null;
        break;
      }
      case 'webrtc_error': {
        console.error(`[CocoonWebRTC] error from cocoon: ${msg['message']}`);
        this.answerReject?.(new Error(msg['message'] as string));
        this.answerResolve = null;
        this.answerReject = null;
        break;
      }
    }
  }
}
