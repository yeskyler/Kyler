# Gateway identity and authorization

The gateway is a single-organization access-control foundation. It provides issued
Owner credentials, revocation, rotation and local audit records.
SSO, individual Web user accounts, quotas, multi-instance coordination and an
external tamper-resistant audit sink are separate follow-up work.

## Bootstrap and credentials

Run `kyler configure` before starting a newly configured HTTP service. Setup
explicitly issues a registered local-service Owner credential and saves it
privately for service control, the HTTP CLI and the managed Web backend. It never
imports an old shared secret. A plain `serve` or a restart never provisions tokens.
A listener requires an active Owner credential in its gateway database.

Every business endpoint requires `Authorization: Bearer <issued-token>`, including
loopback listeners. Only `/healthz` and CORS preflight are public. Old shared
server tokens and `KYLER_SERVER_TOKEN` are not accepted. `KYLER_GATEWAY_TOKEN`
configures the service client credential; `KYLER_CLIENT_TOKEN` remains an explicit
remote-client override. `[server].access_token` stores the managed client's issued
credential; it is not the server's authentication authority. Authentication always
checks the gateway database.

The Web login token (`kyler service token`) is a different credential. A browser
exchanges it for an opaque, seven-day browser session. The Web server stores
session digests beside the selected configuration. Sign-out revokes the current
browser session. Token reset invalidates prior sessions and closes affected Web
event subscriptions on revalidation. Inference runs continue independently.
If Web uses the `KYLER_WEB_TOKEN` override, update that environment value and
restart Web to rotate the effective token.
Provider keys and the gateway credential remain
in the Web server process. The current shared Web login represents that managed
service identity, not separate human users. Separate Owner tokens allow revocation and audit attribution per caller.

## Local administration

Local administration uses the selected service's session directory and requires
filesystem access to its private state. No model credentials or running HTTP
service are needed. Use the same `--config` for provisioning and serving.

```sh
kyler --config /path/to/config.toml token create --name operator
kyler --config /path/to/config.toml token list
kyler --config /path/to/config.toml token rotate <token-id>
kyler --config /path/to/config.toml token revoke <token-id>
kyler --config /path/to/config.toml gateway audit --limit 100
```

Creation and rotation display a secret once. Listing and audit never return it.
CLI tokens default to a 24-hour lifetime; use `--expires-in-seconds` (up to 365 days)
or explicitly choose `--no-expiry`. Setup's local-service credential is
non-expiring and can be revoked with the same administration commands.
Rotation atomically revokes the previous token and preserves the principal ID
and original expiry. Repeated rotation of a revoked token fails.
Revocation is checked from persistent storage on every request and during SSE
subscriptions, including revocation by another process. It closes subscriptions
without automatically cancelling already admitted work. Expired credentials
cannot authenticate. Recovery after revoking all Owners requires local
host administration; there is no anonymous recovery endpoint.

## Owner access

Owner is the gateway login role. It can manage the service, sessions, models,
channels and gateway tokens. Host workspace restrictions and sandbox policy
still apply when running the agent.

Existing Owner tokens continue to work. Admin, Member and Viewer tokens from
previous versions are rejected for gateway access. If the managed service used
an Admin token, run `kyler configure` with the service's configuration to
provision its Owner credential. Update any separately configured gateway
credential overrides with an issued Owner token.

Paired channels use internal workspace-scoped credentials that cannot sign in to
the gateway. Existing channel bindings continue to use that limited authority.

## Storage, audit and operations

`session.directory/_gateway.sqlite` contains token digests, resource ownership and
audit records, with WAL and SHM sidecars. Only SHA-256 digests of 256-bit random
secrets are stored. Token comparison is constant-time. The database is private
(mode 0600 on Unix); the runtime protects control-plane files from workspace tools.
Back up this database together with session state. Deleting it is not a token
reset mechanism: it removes authorization and attribution state.

HTTP audit starts before dispatch and finishes with the handler's status. A record
without a final status means the outcome is unknown; it must not be interpreted as
success or automatically replayed. Audit stores request IDs, principal/token IDs,
matched route patterns, resource IDs, timestamps and status. It excludes query
strings, request bodies, bearer secrets and provider response bodies. Model failures
retain classified diagnostics instead of provider bodies in both events and saved
execution errors. Only known provider codes and request-field roots are public. Token
administration is audited in the same database transaction as its state change.

`GET /v1/gateway/identity` returns the current identity. Owner-only token endpoints
are `/v1/gateway/tokens`, `/v1/gateway/tokens/{id}` and
`/v1/gateway/tokens/{id}/rotate`. Owners may page audit using
`GET /v1/gateway/audit?limit=100&before=<record-id>`, continuing with `next_before`.
Authenticated responses carry `x-request-id`; business responses are not cached.
Storage/authentication failures fail closed; there is no shared-token fallback.
Web proxy authentication failures are reported as backend configuration errors
(502), not as a request to repeat browser login.

Local TUI and stdio ACP use host-authorized in-process operations, not an anonymous
HTTP exemption. The local OS account and trusted embedding APIs remain privileged.
Audit is local operational evidence, not immutable compliance storage. Configure
backup/export and disk monitoring for deployment; retention/export automation,
rate limits and per-identity usage quotas are follow-up work.

For remote deployments terminate HTTPS at a reverse proxy and keep the backend
listener private. Plain HTTP model/MCP loopback endpoints bypass ambient OS proxy
discovery; explicitly configured sandbox proxies and deny policies still win.
Plain HTTP transports skip native TLS-root loading when no TLS proxy is needed.
Model HTTP redirects are rejected; configure the final endpoint URL explicitly.
