# Gateway identity and authorization

The gateway is a single-organization access-control foundation. It provides issued
API identities, scoped sessions, revocation, rotation and local audit records.
SSO, individual Web user accounts, quotas, multi-instance coordination and an
external tamper-resistant audit sink are separate follow-up work.

## Bootstrap and credentials

Run `kyler configure` before starting a newly configured HTTP service. Setup
explicitly issues a registered local-service administrator credential and saves it
privately for service control, the HTTP CLI and the managed Web backend. It never
imports an old shared secret. A plain `serve` or a restart never provisions tokens.
A listener requires an active administrator or owner in its gateway database.

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
service identity, not separate human users. Use scoped API tokens for distinct
callers until individual Web accounts are implemented.

## Local administration

Local administration uses the selected service's session directory and requires
filesystem access to its private state. No model credentials or running HTTP
service are needed. Use the same `--config` for provisioning and serving.

```sh
kyler --config /path/to/config.toml token create --name operator --role owner
kyler --config /path/to/config.toml token list
kyler --config /path/to/config.toml token rotate <token-id>
kyler --config /path/to/config.toml token revoke <token-id>
kyler --config /path/to/config.toml gateway audit --limit 100
```

Creation and rotation display a secret once. Listing and audit never return it.
CLI tokens default to a 24-hour lifetime; use `--expires-in-seconds` (up to 365 days)
or explicitly choose `--no-expiry`. Setup's local-service credential is
non-expiring and can be revoked with the same administration commands.
Rotation atomically revokes the previous token and preserves the principal ID,
role, grants and original expiry. Repeated rotation of a revoked token fails.
Revocation is checked from persistent storage on every request and during SSE
subscriptions, including revocation by another process. It closes subscriptions
without automatically cancelling already admitted work. Expired credentials
cannot authenticate. Recovery after revoking all administrators requires local
host administration; there is no anonymous recovery endpoint.

## Roles and resource grants

| Role | Authority |
| --- | --- |
| Owner | Global administration and token issuance, rotation and revocation |
| Admin | Global session, service, connector and model administration; audit reading |
| Member | Execute within granted workspaces/sessions, choose permitted models, upload inputs, cancel own runs and answer own run questions |
| Viewer | Read owned or explicitly granted sessions and their retained results |

Privileged tokens cannot carry partial scopes. Members and viewers are denied
new or unclassified operations by default. Tool approval, permissions changes,
provider connection edits, global preferences, automation management and plugin
management require administrative authority.

```sh
kyler token create --name project-worker --role member \
  --workspace /absolute/path/to/project --model approved-profile
kyler token create --name reviewer --role viewer --session <session-id>
```

Workspace grants must be existing absolute directories and are canonicalized.
Model grants use catalog **profile IDs**. Empty model lists permit configured
models; empty workspace lists do not allow members to create sessions. An empty
session grant list does not expose other principals' sessions. Administrative
identities may explicitly delegate an existing session. Ordinary identities never
automatically acquire old sessions; ownership is recorded on creation.

Lists are filtered before pagination. Every session child route, including
history, attachments, artifacts, tool details and process output, checks the
parent session scope. Run reads and subscriptions resolve the owning session;
cancelling another principal's run is denied even in a shared session. Request
idempotency keys are namespaced by principal and survive token rotation.

Before member execution, filesystem authority is intersected with the execution
workspace. Unrelated host data roots are removed; host restrictions and required
runtime mounts remain. Members cannot use a Full access selection or a host mode
without native containment. Workspace grants never expand the host sandbox. Member MCP connections use a fresh
execution scope and are closed after the run, so they cannot reuse administrator
connector processes with broader filesystem authority.
Project workspaces still share files between their authorized collaborators.
Memory and installed integrations retain their existing project/host semantics;
this phase does not provide isolated organizational tenants or personal Memory.

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
`/v1/gateway/tokens/{id}/rotate`. Administrators may page audit using
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
