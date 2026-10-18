# Worker brief #11 — Fracker eleventh pass (capture FD signature fix)

Branch: `opencode/eleventh-pass` (created from master AFTER
pass 10 merges; check it out then). Read FIRST:
`docs/verification-addendum.md` items 39, 40.

Small surgical pass. Seth ran v0.3.1 and hit Capture
Inventory. The good news: pass 9's instrumentation worked —
the tray status named the failure. The failure, verbatim
from his screenshot:

```
Capture: error (consent pending): ScreenCast remote fd:
Signature mismatch: got 'h', expected 'v'
```

## Diagnosis

`OpenPipeWireRemote` on the ScreenCast session returns a
bare Unix file descriptor — D-Bus signature `h`. The call
site in `src/screencast.rs` declares/expects a Variant (`v`)
somewhere in its reply handling, so zbus rejects the reply
before the FD is ever extracted. CreateSession / SelectSources
/ Start all return dicts and worked (consent appeared); the
FD-returning call is the odd one out.

## Fix

- Make the OpenPipeWireRemote reply type an FD end to end
  (zbus `OwnedFd`), and make sure the connection negotiates
  Unix FD passing. If the typed proxy path keeps fighting
  the signature, drop to the raw message call with the
  explicit `h` reply — working code over pretty code.
- Regression pin: a unit test asserting the reply-signature
  constant the call expects is exactly `h` (the box has no
  portal, so the signature is the testable seam), plus a
  comment at the call site naming the trap for the next
  reader.
- While there: the error status rendered "error (consent
  pending)" for a failure that happened AFTER consent. The
  error state must name the stage that FAILED (here:
  remote-fd), not the last state the machine was in.

## Gates and deliverables

- `cargo build` clean, `cargo test` green.
- One or two logical commits; PUSH `opencode/eleventh-pass`
  and VERIFY with `git ls-remote`.
- Final summary to stdout: the exact signature mismatch site
  (file/line), what the reply handling looks like now, and
  confirmation the tray error label names the failing stage.
