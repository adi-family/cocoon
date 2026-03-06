# Cocoon Web Plugin: Setup Flow with Auth

## What was done

1. **Cocoon list page** (`component.ts`) — LitElement component showing connected cocoons with expandable session details. Has a "Setup Cocoon" button that triggers the pairing flow.

2. **Setup flow** — Browser polls `http://localhost:14730/health` to detect when `adi cocoon setup` is running on the local machine. When found, shows machine name and a "Connect" button. On connect, POSTs to `/connect` with `{ token, auth_token, signaling_url }`.

3. **Auth token passthrough** — The signaling server requires auth (`auth_requirement: "required"`). The web app is already authenticated, so:
   - Plugin (`plugin.ts`) provides `authTokenProvider` callback using `auth:get-token` -> `auth:token-resolved` bus pattern
   - Component calls it during connect, passes token in POST body
   - Setup server (`setup.rs`) forwards it as `COCOON_AUTH_TOKEN` env var
   - Cocoon core (`core.rs`) now waits for `AuthHello`, sends `AuthAuthenticate` with the token, waits for `AuthHelloAuthed`, then sends `DeviceRegister`

4. **Generated types** — TypeScript types generated from `cocoon.tsp` via `adi tsp-gen`. All silk/cocoon message types use `silk_` prefix (e.g., `silk_create_session`, `silk_output`). Updated `silk-session.ts`, `silk-command.ts`, `cocoon-client.ts` to match.

## Key design decisions

- **No manual device ID entry** — User explicitly rejected this. Cocoons are added only via `adi cocoon setup` pairing flow.
- **Auth domain derivation** — Plugin derives auth domain from signaling server URL: `ws(s)://host/...` -> `http(s)://host/api/auth`. This assumes same-origin auth endpoint.
- **Handshake before registration** — Cocoon core now has a proper handshake loop: `AuthHello` -> `AuthAuthenticate` -> `AuthHelloAuthed` -> `DeviceRegister`, instead of immediately sending `DeviceRegister`.

## Files changed

| File | Change |
|------|--------|
| `web/src/component.ts` | Setup flow UI, auth token provider, polling |
| `web/src/plugin.ts` | `getAuthToken()` via bus, `authTokenProvider` wiring |
| `web/src/silk-types.ts` | Re-export bridge from generated types |
| `web/src/silk-session.ts` | `silk_` prefix for all message types |
| `web/src/silk-command.ts` | `silk_` prefix for message types |
| `web/src/cocoon-client.ts` | `silk_` prefix for message types |
| `web/src/index.ts` | Added `export * from './generated'` |
| `web/src/generated/` | Auto-generated from cocoon.tsp |
| `core/src/core.rs` | Auth handshake before DeviceRegister, `COCOON_AUTH_TOKEN` env var |
| `core/src/setup.rs` | `auth_token` field in ConnectRequest, forwarded as env var |

## Open questions for investigation

- **Auth domain derivation** may not match all deployments (assumes `/api/auth` path convention)
- **Token expiry** — the access token passed to the cocoon has a limited lifetime; long-running cocoons may need token refresh
- **Peer auto-discovery** — after cocoon connects to signaling, the web plugin doesn't yet auto-create `CocoonClient` instances from `signaling:peer-connected` events; this is manual via `createClient()`
