# Worker brief #18 — Fracker eighteenth pass (tray version, WebSocket push)

Branch: `opencode/eighteenth-pass` (created from master;
check it out). Read FIRST: `docs/verification-addendum.md`
items 57 and 58. This pass ships as **v0.5.4**.

## §1 Version in the tray menu (item 57)

Add a DISABLED (non-interactive) menu item to the tray
menu showing the running build, e.g. "Fracker v0.5.4".
Source of truth is the crate version
(`env!("CARGO_PKG_VERSION")`). Note the crate version
has never been bumped — Cargo.toml still says 0.1.0
while releases reached v0.5.3. Set the Cargo.toml
version to **0.5.4** in this pass so the menu tells the
truth from this release on (release merges keep it in
step with the tag after that).

## §2 WebSocket push for plan updates (item 58)

The loopback plan server (:8791) pushes a new
generation to open pages the moment a regen lands;
pages stop discovering updates only at the 60s
meta-refresh. Design constraints from the addendum:

- Reload-on-push is the mechanism: ticks live
  server-side and tab scope is generation-aware
  (item 52), so a pushed reload lands on First Crack
  like a manual regen — no client state to migrate.
- Keep the meta-refresh as the BACKSTOP for a dropped
  socket, with client reconnect; push is primary.
- Push fires when a new generation is published (the
  same points that today bump the generation token).
- Stay in the codebase's minimal style: the server is
  hand-rolled; a minimal RFC 6455 implementation
  (handshake + text frames + close) is in keeping. If
  you add crates (e.g. sha1/base64), they MUST be
  vendored into the flatpak offline cargo sources like
  every other dependency — the packaging agreement test
  must stay green.

## Gates and deliverables

- `cargo build` clean, `cargo test` green (add tests:
  WS handshake/frame handling at whatever level is
  headless-testable, and a generation-push trigger
  test); regenerate `docs/demo-plan.html` if the served
  page markup/script changed.
- Logical commits per section; PUSH
  `opencode/eighteenth-pass` and VERIFY with
  `git ls-remote`.
- Final summary to stdout: per-section evidence,
  including how the version string reaches the menu
  and where generation publication fires the push.
