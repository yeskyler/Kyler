# kyler-provider

Provider adaptation for Kyler, independent of Agent Runtime and host configuration.

- Resolves OpenAI, SiliconFlow and generic Chat Completions parameter policies.
- Describes documented model capabilities; unknown capabilities stay unknown.
- Discovers model IDs without inference, with bounded responses and safe errors.
- Shares streaming/non-streaming transport, reasoning decoding and error classification.

The host supplies credentials and projected requests. This crate does not read
configuration files, persist credentials, select models, execute tools, or access
sessions. Core schedules discovery, persists its credential-free cache, binds model
snapshots and validates session preferences. Shared client contracts live in
`kyler-protocol`.

See [configuration](../../../docs/configuration.md#providers-and-model-discovery).
