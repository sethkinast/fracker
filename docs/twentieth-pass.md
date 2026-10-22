# Worker brief #20 — capture direction constant + FFI value audit (v0.5.6)

Branch: `opencode/twentieth-pass` (created from master;
check it out). Read FIRST: `docs/verification-addendum.md`
item 63. The root cause of every capture failure since
pass 9 is identified and header-verified; this pass is
the fix plus the audit that closes the class.

## §1 The constant

`src/screencast.rs` hand-defines
`PW_DIRECTION_INPUT: c_int = 1`. Upstream headers say
`SPA_DIRECTION_INPUT = 0`, `SPA_DIRECTION_OUTPUT = 1`
(spa/utils/defs.h:95-96), and `pw_stream_connect` takes
`enum pw_direction` (stream.h:547). The app has been
connecting an OUTPUT stream and asking the session
manager to link it into the portal source node (no
input ports): "target not found", deterministically.
Fix the constant to 0. Also correct the pass-9 doc
comment block above `PW_STREAM_FLAGS` whose
private-remote theory rationalized the symptom — the
record should state the real mechanism.

## §2 Audit every hand-defined FFI value in the module

The audits that missed this checked symbol names and
pod shapes, never enum/flag VALUES. Go through every
hand-defined constant in the capture FFI surface
(directions, stream flags, stream states, format/media
enums and subtypes, any SPA/PipeWire numeric the code
declares for itself) and verify each against the
upstream headers, citing header + line in your summary
for every value checked. Fix any other mismatch the
same way. Known-good reference points (verified by
Graf against stream.h): AUTOCONNECT = 1<<0,
MAP_BUFFERS = 1<<2, DONT_RECONNECT = 1<<7;
PW_ID_ANY = 0xffffffff.

## Gates and deliverables

- `cargo build` clean, `cargo test` green.
- Bump Cargo.toml version to **0.5.6**.
- Logical commits; PUSH `opencode/twentieth-pass` and
  VERIFY with `git ls-remote`. Final summary: the §2
  audit table (constant, code value, header value,
  header citation) in full — that table is the
  deliverable that proves the class is closed.
