# Worker brief #19 — HOTFIX: plan page reload storm (v0.5.5)

Branch: `opencode/nineteenth-pass` (created from master;
check it out). Seth, live on v0.5.4 (2026-10-21): the
plan page reloads CONSTANTLY — every few milliseconds,
flickering, re-fetching every icon until his IP was
rate-limited by the Warframe image CDN. Browser Stop
pauses it briefly, then it resumes. Read addendum #61.
This is a hotfix: the WS push from pass 18 (§2) has a
self-reload loop in the connect/handshake path.

## §1 Fix the reload predicate

Find the exact client/server interaction that loops and
fix it so a push-triggered reload happens ONLY when the
server's generation genuinely differs from the
generation embedded in the loaded page. The connect-time
hello naming the current generation must be a no-op when
they match. State the client's reload predicate
precisely in your summary (inputs, comparison, action
for equal vs different).

## §2 Loop-proof the client regardless (fail-safe)

Even with §1 correct, no server behavior may ever again
be able to create a reload storm:

- A minimum interval between push-triggered reloads
  (seconds, not milliseconds).
- A circuit breaker: after a small number of
  push-triggered reloads inside a short window, the
  page disables WS reloading for the session and falls
  back to the 60s meta-refresh alone.
- The meta-refresh backstop stays as designed.

## §3 Tests

- Client predicate tested at the level it is factored:
  equal generation on hello -> NO reload; differing
  generation -> exactly one reload.
- Breaker tested: repeated differing pushes inside the
  window stop triggering reloads after the cap.
- Existing gates stay green: `cargo build` clean,
  `cargo test` green.

## Ship mechanics

- Bump Cargo.toml version to **0.5.5** (tray menu truth,
  item 57 discipline).
- Logical commits; PUSH `opencode/nineteenth-pass` and
  VERIFY with `git ls-remote`. Final summary to stdout
  with the §1 predicate stated.
