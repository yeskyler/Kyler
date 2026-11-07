# Security

Kyler can execute commands and access workspace files under configured sandbox
and approval policies. Deploy it as a single-user application unless an operator
provides the additional authentication and isolation needed for their deployment.

## Reporting a vulnerability

A dedicated private reporting contact has not yet been documented. If the
repository provides GitHub's private vulnerability-reporting option, use it.
Otherwise, use a maintainer's published private contact when available, or request
a private contact without posting exploit details in a public issue.

Do not publish credentials, personal conversations, private workspace files,
benchmark data, or an exploit that exposes another user's information.

A private report should include the affected version and platform, redacted
configuration, a minimal synthetic reproduction, impact, diagnostic evidence,
and any suggested mitigation or patch.

No formal supported-release schedule or response-time commitment is defined here.
The maintainer should document these details when establishing the release policy.

## Operating guidance

- Keep services on loopback for local use. Public Web bindings require an access
  password; use TLS at a reverse proxy across untrusted networks.
- Keep backend tokens and model keys on the server. Restrict access to configuration,
  service records, logs, and conversation storage.
- Use workspace access or read-only permissions when they meet the task's needs.
  Managed sessions keep a private filesystem boundary; Full access is unavailable
  when it would bypass session isolation. Approval never exceeds host authority.
- Review skills and connector packages before enabling them. Remote services retain
  their own authorization and data-handling boundaries.
- Treat model and tool output as untrusted content when preparing shell commands,
  patches, or downloaded files.

See [configuration](docs/configuration.md) and [architecture](docs/architecture.md)
for the implemented controls. This policy does not claim a completed security audit.
