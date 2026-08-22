# mcpg-inspector-tui

The terminal face of [`mcpg-inspector`](../server). Screens, keys, and the
port they read through — and nothing about the engine.

Everything displayed arrives via `api::InspectorApi`. Two things implement it:

- the server, over its own in-process engine
  (`apps/inspector/server/src/local_api.rs`);
- `api::RemoteApi`, over a running inspector's HTTP API, which is what
  `mcpg-inspector tui --attach <url>` uses.

That is why the same screens can show a target this process dialed and one
held open by an instance across the network — including its wire log, which a
directly-dialing terminal would never see.

`state` and `draw` are pure: state in, screen out, no terminal and no
network, which is what lets every screen be asserted against a `TestBackend`.
`lib.rs` is the IO shell around them — raw mode, the event loop, and the
async calls a keypress asks for.

## Screens

`targets` · `tools` · `resources` · `prompts` · `subs` · `pending` ·
`diagnose` · `wire`

`tab` (or `h`/`l`) moves, `j`/`k` selects, `a` edits, `enter` runs whatever
the screen is about. `?` lists the keys.
