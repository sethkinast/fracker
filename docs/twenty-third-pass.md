# Worker brief #23 — Fracker twenty-third pass (capture: pod layout, session lifetime, buffer mapping)

Branch: `opencode/twenty-third-pass` (created from
master; check it out). Read FIRST:
`docs/verification-addendum.md` item 69 — an external
review (Claude, via Seth) whose claims Graf verified
against the code one by one. This pass implements the
verified substance. Ships as **v0.5.10**.

## §1 Fix the SPA Choice pod layout (item 69a)

`pod::choice` wraps its children in a Struct pod. A
real Choice body is `{kind: u32, flags: u32,
child_header: (size, type), raw values packed with NO
per-value headers}`. Rebuild `choice()` to that layout
(child header from the first child's header; each
child contributes only its raw payload). Enum choices
repeat the default as the first alternative (e.g.
formats `[BGRx, BGRx, BGRA, RGBx, RGBA]`, modifiers
`[LINEAR, LINEAR, INVALID]`). REWRITE the pinning test
`choice_pods_carry_their_kind_and_child_struct` to
expect the correct layout — it currently enshrines the
bug. This affects every choice in the EnumFormat offer
(format, size, framerate, modifier).

## §2 Keep the portal session alive (item 69b)

`negotiate()` drops its `zbus::Connection` on return;
`Negotiated` keeps only fd/node/token, orphaning the
session the portal tied to that connection. Carry the
Connection and the session object path in `Negotiated`
(and in whatever owns it through the capture), and call
`Session.Close` at capture teardown.

## §3 Map the buffers; drop the Format echo (items 69c, 69d)

- Nothing ever mmaps buffer memory, so `SpaData.data`
  is null for every portal buffer and no frame can
  convert. Implement buffer mapping: for MemFd/DmaBuf
  datas, mmap `data.fd` for the chunk's range and read
  pixels from the mapping (unmap on buffer release).
  Correct `SpaData.fd` to `i64` while there (it is i32
  today; current field reads survive only by alignment
  luck). Keep the negotiated-format-driven dimensions:
  width/height come from the negotiated format, NOT
  `stride / 4`.
- Remove the pass-21 Format echo
  (`pw_stream_update_params` with the Format pod). If
  buffer parameters are needed, answer with
  SPA_PARAM_Buffers/Meta like canonical consumers;
  otherwise send nothing. Keep the symbol in the
  manifest only if it remains used.
- Modifier stance: with mapping in place, state in
  your summary which modifiers the offer carries and
  why (LINEAR must be acceptable; if KWin still
  intersects nothing, the fallback is dropping the
  modifier prop so it picks MemFd).

## §4 Riders (item 69e)

- `pw_stream_new`: pass properties media.type=Video,
  media.category=Capture, media.role=Screen (OBS-style)
  instead of NULL.
- Framerate offer: a choice RANGE starting at 0/1, not
  the pinned fraction 30/1.
- Frame channel: bounded with drop-oldest semantics
  (OCR drains far slower than 30 fps production); no
  unbounded queue anywhere on the frame path.

## Gates and deliverables

- `cargo build` clean, `cargo test` green; bump
  Cargo.toml to **0.5.10**.
- Logical commits per section; PUSH
  `opencode/twenty-third-pass` and VERIFY with
  `git ls-remote`. Final summary per section, including
  the §3 modifier decision and its reasoning.
