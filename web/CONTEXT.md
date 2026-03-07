# Cocoon Web Plugin: Setup Flow with Auth

## What was done

1. **Cocoon list page** (`component.ts`) — LitElement component showing connected cocoons with expandable session details. Has a "Setup Cocoon" button that triggers the pairing flow.

2. **Cocoon debug section** (`debug-section.ts`) — Debug screen section with "Setup Manual Cocoon" button, same pairing flow as list page.

3. **Setup flow** — Browser polls `http://localhost:14730/health` to detect when `adi cocoon setup` is running on the local machine. When found, shows machine name and a "Connect" button. On connect, POSTs to `/connect` with `{ token, signaling_url }`.

4. **Subtoken auth** — The `token` field carries a short-lived JWT (10 min TTL) for cocoon ownership assignment:
   - Plugin (`plugin.ts`) provides `subtokenProvider` callback
   - Provider gets auth token via `adi.auth.getToken()`, then exchanges it for a subtoken via `POST /api/auth/subtoken`
   - Subtoken is passed as `token` in the `/connect` POST body
   - Setup server (`setup.rs`) forwards it as `COCOON_SETUP_TOKEN` env var
   - Cocoon sends it as `setup_token` tag in `DeviceRegister`
   - Signaling server validates JWT, extracts user ID, assigns device ownership

5. **Generated types** — TypeScript types generated from `cocoon.tsp` via `adi tsp-gen`. All silk/cocoon message types use `silk_` prefix (e.g., `silk_create_session`, `silk_output`).

## Key design decisions

- **No manual device ID entry** — Cocoons are added only via `adi cocoon setup` pairing flow.
- **Auth domain derivation** — Plugin derives auth domain from signaling server URL: `ws(s)://host/...` -> `http(s)://host/api/auth`.
- **Subtoken over direct JWT** — Short-lived subtoken (10 min) limits exposure. The cocoon only needs the token once for registration, not for ongoing auth.
- **Cocoon skips WS auth** — Cocoons connect to signaling via `?kind=cocoon` endpoint which skips the `AuthHello`/`AuthAuthenticate` handshake. Auth is via secret/HMAC device registration instead.

## Open questions

- **Auth domain derivation** may not match all deployments (assumes `/api/auth` path convention)
- **Peer auto-discovery** — after cocoon connects to signaling, the web plugin doesn't yet auto-create `CocoonClient` instances from `signaling:peer-connected` events; this is manual via `createClient()`
