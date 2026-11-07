# Kyler Web UI

The Next.js frontend connects to the Rust backend. Model calls, tool execution,
sandbox enforcement, approvals, attachments, and conversation storage remain in
Kyler. Model and backend credentials stay on the Next.js server.

For installed usage, see [installation](../docs/installation.md). For the full
build workflow, see [development](../docs/development.md); packaging is covered
in [releasing](../docs/releasing.md).

## Development

With a configured backend running, execute from `web/`:

```sh
npm ci
npm run dev -- --hostname 127.0.0.1 --port 30002
```

Open http://127.0.0.1:30002. If the installed Web service owns this port, choose
another development port. The UI reads `~/.kyler/config.toml` on the Next.js
server. Workspace identity is provided by the backend, not the UI launch directory.

| Environment variable      | Purpose                                      |
| ------------------------- | -------------------------------------------- |
| `KYLER_CONFIG`            | Alternate configuration file                 |
| `KYLER_CLIENT_SERVER_URL` | Backend connection URL                       |
| `KYLER_CLIENT_TOKEN`      | Backend access token                         |
| `KYLER_WEB_BIND`          | Listen address used by Web access validation |
| `KYLER_WEB_TOKEN`         | Web access password                          |

No `.env` file is required. Backend and Web passwords are separate credentials.
Public Web bindings require authentication; see [security guidance](../SECURITY.md).

## Features

- Streamed Markdown responses and tool activity/results via Kyler SSE events.
  Assistant messages keep the final answer visible beneath an execution summary.
  Earlier commentary and tool activity are collapsed by default and expand in
  chronological order. Live runs show elapsed time; failed tool counts and run
  errors remain visible without opening the process. Tool output expands separately.
  Markdown tables use horizontal separators and scroll horizontally on narrow screens.
- Stop cancels the backend run; stream sequence IDs avoid duplicate replay.
- Upload images/documents or paste screenshots into the composer; files use the
  same session attachment API as the TUI (20 MB per file).
- Tool approvals show a preview with Allow once / Deny actions.
- The current session is remembered in browser local storage. Reload restores
  saved conversation text and reconnects to an active run without resubmitting it.
  Attachment references appear as names in saved-message replay.
- The sidebar places Plugins below New chat and groups saved chats
  by workspace. Empty projects can be removed without deleting local files.
  Workspace folders only expand to show history; selecting a chat restores its
  bound workspace. Search filters chats and workspaces. New chat retains its
  highlighted background; other navigation items have no selected background.
  The conversation header can collapse the sidebar on desktop; the top-left button expands
  it again. The layout preference persists across chats and reloads. On narrow
  screens the sidebar remains a drawer, closed with its button, backdrop, or Escape.
  Status icons refresh every three seconds. Rows offer deletion with confirmation;
  running sessions cannot be deleted.
- The Workspaces add button opens the local operating-system directory chooser,
  then asks for a project title. Project titles are saved independently of chats.
  The chooser does not list or prefill previously used directories.
  The picker requires a local Kyler service; macOS uses its system folder dialog,
  and Linux requires Zenity. Cancelling never creates a workspace.
- New chat precreates a backend draft session bound to the default workspace.
  Its temporary execution checkpoints stay out of history until the first model
  text reply. Empty or unanswered drafts do not publish a history entry; leaving
  an inactive draft attempts cleanup. Active draft runs can be recovered after a
  reload without duplicating the input. History uses the server's title, with the
  first user message as a temporary fallback. The server currently derives titles
  from persisted user text without an extra model request.
- New chats keep the composer vertically centered. The compact composer is about
  three quarters of its previous height. Model names wrap in full without icons
  or ellipsis. There is no user/profile footer in the sidebar. It docks at the bottom
  after the first message. The interface uses neutral light/dark palettes, follows
  the system preference initially, and remembers the appearance toggle in the upper-right corner locally.
- Model settings are inside the composer with URL, Model, and Key fields for
  OpenAI-compatible Chat Completions. Saves update the selected profile in the
  Kyler configuration and take effect on the next turn without restarting.
  URL accepts the API base URL or a complete `/chat/completions` endpoint.
  Key input is hidden; leaving it blank keeps the current key. Existing keys are
  never sent back to the browser. Config files are saved atomically with private
  permissions. Explicit startup overrides must be removed before editing the
  corresponding fields through the UI.
- Directory and sandbox controls sit below the composer. Changing the directory
  creates a new session; sandbox choices reflect the server's host policy.
- Plugins opens a management page with Skills and Connectors tabs.
  User added skills appear before System skills.
  System includes skill-creator and skill-installer. Add skill opens a new chat
  with @skill-creator selected; it does not send a message automatically.
  Users can review instructions and enable or disable packages.
  Plugin management and discovery connections are system-wide, independent of
  workspaces. Actual tool calls use separate connections under the session sandbox.
- Users can add MCP connectors using Remote connection (Streamable HTTP) or
  Upload package (ZIP). Added connectors are saved with private permissions in
  `<session-directory>/_connectors.json`, shared by the Web UI and TUI.
  The composer + menu offers Upload attachment and Plugins → Skills / Connectors.
  Selecting a skill inserts @skill-name. Selecting a connector establishes its
  system discovery connection, then inserts @connector:server_id. Session tools
  become available only when that session's sandbox allows the connection.
  Connectors remain disconnected until Connect or a composer selection is used. Resolved credential values
  are excluded from saved definitions; authenticated HTTP connectors use process
  environment references. Configured TOML definitions take precedence on restart.

The UI intentionally omits editing/regeneration and local branching: the backend
owns the conversation history. A new prompt adds one turn to the existing session.

## Production and stopping

```sh
npm run build
npm run start -- --hostname 127.0.0.1 --port 30002
```

Ctrl+C stops a manually launched Next.js process. `kyler service stop` stops
managed backend and Web services, not a manually launched development server.
Keep the UI bound to loopback. Public bindings require [web].access_token and use browser Basic authentication;
use a TLS reverse proxy for access across untrusted networks. This is a single-user application;
put it behind an authenticated gateway before exposing it to other users.

## Checks

```sh
npm run lint
npm test
npm run build
```

### Connector packages

Upload a ZIP with `connector.json` at its root and an included executable:

```json
{ "id": "my_connector", "command": "server", "args": ["${package}/config.json"] }
```

`command` is a relative file path inside the package. `${package}` in arguments
expands to its installed directory. Packages must be self-contained for the host
platform; Kyler does not run installation hooks or install dependencies. Uploaded
packages are limited to 20 MB compressed, 64 MB expanded, 512 entries, and 16 MB per
file. Symlinks, traversal paths, duplicate files, and existing IDs are rejected.
System discovery uses a dedicated sandbox with package/runtime read access and
outbound networking, without project access. Project connections retain their
session filesystem and network restrictions. Connect explicitly after installation.

## Session inspection

Token metrics use decimal `k`: `1000 tokens = 1k tokens`. Values keep up to three
decimal places, so `1024` displays as `1.024k`. Context occupancy and cumulative
session usage are separate metrics. Category values are estimates allocated to
the displayed input total. Rollout's readable view uses `k`; Download raw JSON
retains integer counts for diagnostics.

The composer footer keeps workspace and permissions on one line. Saved sessions
also show three session-specific icons: Rollout, Memory, and Context. New chat
drafts hide these icons and do not fetch inspector data until the first model
reply promotes them into history. They open a shared inspector,
as normal content pages in the session's main area, matching the Plugins page.
There are no inspector drawers, overlays, or modal dialogs. Back to chat sits in
the main top bar; titles, tabs, and details scroll in the body. Categories use a
single column, with one section per row. Returning to chat restores
its existing messages, composer draft, and execution state.
Switching sessions resets the inspector; inspection never calls models or tools.

- Rollout pages committed records in sequence order and polls at the live tail.
  Select an event to fetch its details. Tool results offer Original output and
  Model input views; secret argument keys are redacted and large strings bounded.
  Application-state values and complete session-created snapshots are not exposed.
- Memory shows the actual compressed session summary, all persistent facts in the
  resolved workspace/session scope, keys used in the latest request, and recent
  explicit memory tool attempts. It is read-only.
- Context shows the last prepared request with its bound model/window policy,
  captured before the provider request. Provider-reported input replaces the input
  estimate when available. Category allocations remain proportional estimates.
  Output reservation, safety margin, sliding-window target, compaction history,
  and cumulative session usage are displayed separately. Old sessions without a
  request snapshot show an empty state until their next turn.

These authenticated GET endpoints are scoped to a session:

```text
/v1/sessions/{id}/rollout?cursor=0&limit=60
/v1/sessions/{id}/rollout/{sequence}
/v1/sessions/{id}/context
/v1/sessions/{id}/memory/inspect
```

### Workspace networking

See [configuration](../docs/configuration.md) for sandbox networking defaults,
existing session restrictions, and host-policy updates.

## Provider-returned thinking

Chat Completions streams may return textual `reasoning_content` or `reasoning`.
Kyler forwards these as separate reasoning events and retains them in history.
The Web UI renders a Thinking disclosure only when non-empty content arrives;
it opens while streaming and collapses afterward. Ordinary text and tool activity
do not create thinking content. Working remains the overall run status. No thinking
mode or reasoning-effort parameter is enabled by this display feature. Retained
reasoning is excluded from subsequent model messages and summary projections.

## Artifact preview panel

Saved conversations expose a monitor-shaped Preview toggle above the composer, aligned left.
The monitor icon blinks gently during a run, without
additional status text. A plan control on the right expands the latest successful `update_plan` inside
the composer header, with step status and completion count. Session
inspection controls remain below the composer.
On desktop, opening Preview splits the session into 30% conversation and 70% preview.
The preview retains rounded borders and its browser-style toolbar. Narrow screens
use an overlay. It opens automatically when a session has a previewable
generated file. The window includes file-history navigation, refresh, a path bar,
download and close controls, an Inputs/Outputs tree, and a compact status bar.
Files and retained copies are isolated per session, even when conversations use
the same workspace. New previewable outputs open after a run finishes; manually
closing the window suppresses reopening for that run. Escape also closes it.

The file tree supports name and path search and reveals matching folders. It starts
collapsed on narrow screens. Drag the preview's left edge to resize it, or double-click
to restore the default width. Switching files keeps the chosen width. Refresh reloads
the file list and selected preview; failed loads offer a retry. Original files remain
available through Download. There are no maximize controls or Web/File switch.

Supported previews:

- HTML: live JavaScript and CSS inside an opaque sandboxed iframe. No browser
  server, Chromium executable, screenshot polling or additional service is needed.
- PDF: page navigation and zoom using PDF.js, without executing PDF actions.
- Raster images and SVG images: fit/zoom; SVG runs as an image, without scripts.
- CSV/TSV and XLSX: bounded tables; Excel workbooks support sheet selection.
- DOCX and PPTX: text and embedded-image content previews. Complex layout,
  charts, animations and advanced Office features are not reproduced.
- Audio/video: native playback when the user's browser supports the codec.
- Markdown, JSON and text/code: formatted content or source view.

Unknown or unsupported formats remain downloadable. Text is limited to 1 MiB;
HTML over that limit is not partially executed. Office archives are limited to
5 MiB with bounded decompression; workbook previews show up to 200 rows and 100
columns. Original downloads remain complete within the backend retention limits.

HTML deliverables retain up to 24 explicitly referenced local presentation assets
under the existing workspace permissions and snapshot limits. The collector does
not scan directories or fetch remote files. Resource snapshots remain tied to the
same session/execution and are hidden from the top-level Outputs list. Existing
HTML artifacts created before asset retention may need regeneration; files are
never silently read from a different execution to repair a preview.

Scripts are enabled for HTML previews, while `allow-same-origin`, top navigation,
popups and form submission remain disabled. The content policy blocks arbitrary
network connections and nested frames. Public script/style dependencies may load
anonymously from jsDelivr, cdnjs, unpkg and esm.sh; offline use requires bundled
assets. The preview does not display script/resource diagnostics in the product UI. Dynamic backend APIs, development servers and arbitrary
websites belong in external tools, not this deliverable panel. Prefer self-contained
HTML for reliable offline rendering; multi-module apps should be bundled first.

All files have a download action. External links in assistant messages open in
a separate browser tab. Agent web automation remains the responsibility of the
configured MCP browser tools.

HTML localStorage is virtualized per session/artifact and persisted through a
validated parent-frame bridge (64 KiB, 256 keys). sessionStorage is scoped to the
current frame. Neither grants access to application cookies or application storage.
