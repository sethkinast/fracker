# Worker brief #13 — Fracker thirteenth pass (PipeWire symbol audit, tab persistence)

Branch: `opencode/thirteenth-pass` (created from master
AFTER pass 12 merges; check it out). Read FIRST:
`docs/verification-addendum.md` items 46, 47.

## §1 Capture: audit the dlopen'd PipeWire symbols (item 47)

Pass 11 fixed the portal FD. Seth's v0.4.0 live test then
got one stage further — consent → FD → "connecting" — and
died with (tray status, his screenshot):

```
Capture: error (connecting): capture: libpipewire missing
symbol pw_thread_loop_get_context …
```

The capture consumer resolves libpipewire symbols by name
at runtime (dlopen, because the build box has no
libpipewire). A wrong symbol name compiles clean and only
fails on a real desktop — which is what happened:
`pw_thread_loop_get_context` is not part of PipeWire's
public thread-loop API (verify this yourself against the
headers; do not take the brief's word for it either).

- Audit EVERY dlsym'd name in the capture code against the
  real PipeWire public headers (fetch the header set for a
  current PipeWire release; Bazzite ships a current 1.x).
  Any name not in the headers is a bug by definition.
- Where the construction sequence relied on a nonexistent
  call, rebuild the sequence from real API: context →
  connect via the portal FD → core → stream on a thread
  loop, using only exported symbols.
- Pin it: a checked-in manifest (data file or test constant)
  listing every symbol the consumer requires, with a test
  asserting the code's dlsym list and the manifest agree —
  so a future rename breaks the build, not Seth's evening.
- Keep the lifecycle behavior from passes 9/11: failures
  still surface in the tray with the failing stage named.

## §2 Refresh keeps the current tab (item 46)

The served plan page's 60s meta-refresh reloads it and the
CSS-radio tab state resets to First Crack. Persist the
active tab so a refresh lands back where he was: the page
is served by the app (pass 9), so remember the tab
server-side (tab selections as real links the server
records, rendered checked on subsequent serves) or an
equally durable mechanism that survives regenerations and
app restarts. Manual ticks and all other page behavior
unchanged.

## Gates and deliverables

- `cargo build` clean, `cargo test` green (incl. the symbol
  manifest test and a tab-persistence test).
- Logical commits per section; PUSH
  `opencode/thirteenth-pass` and VERIFY with `git ls-remote`.
- Final summary to stdout: the symbol audit table (every
  dlsym name → in-headers yes/no → correction if any) and
  the tab-persistence mechanism chosen.
