# Forty-first pass — fix #2: kill full-page refresh (fragment swap)

Seth's complaint, in his words: the page does full refreshes, and
"every image blinks as it reloads from cache." Every update path
was a full document navigation — the WebSocket push carried only
`{"generation": token}` and the client answered with
`location.reload()` (a render test pinned "a push reloads the
page"), and the backstop was a 60 s meta-refresh doing the same.
Each cycle re-requested and repainted every icon and destroyed
scroll and focus. Seth's chosen shape (2026-10-26, "low-fork"):
load the data over the wire and update in place; the server
stays the only renderer; no client-side templating.

## Changes

### 1. Fragments — the page's swap units (`render.rs`)

The renderer now wraps the inner HTML of the page's five major
sections in inert comment markers (`<!--@frag:<id>-->` …
`<!--/@frag-->`): `header` (the `<header class="top">` block),
and the four tab pages `p1`–`p4`. The container tags themselves
are the static shell and are never swapped. The tab radios, the
tab nav, and the scripts carry no markers: they never change
between generations.

`render::extract_fragments` splits a rendered page back into
`(id, inner HTML)` in document order; `render::fragment_hash`
is the change-detection primitive. A markerless page (the
placeholder, an old demo export) extracts to nothing.

### 2. The endpoint (`server.rs`)

`GET /fragments?gen=<known>` returns JSON: the current
generation, the day, the tick payload, and only the fragments
whose rendered HTML changed since `<known>` — per-fragment hash
comparison against a small retained cache (last 4 generations).
A request naming the current generation returns an empty change
set. The cache is warmed by both the full-page serve and every
fragment answer, so a page's first push-driven call diffs
against the generation it was served. A forgotten generation is
answered with every fragment: a fuller swap, never a
navigation.

The tick payload is built by one shared helper
(`tick_payload_b64`) for `GET /` and `/fragments`, so the two
paths cannot drift — a test asserts byte-identity against the
served page's `data-fracker-ticks` attribute.

### 3. The client (`push.rs`, and the page script in `render.rs`)

The push guard's action for a differing generation is now
**Fetch**, not Reload: the page fetches the diff and assigns
each changed fragment to its container's `innerHTML`, then
adopts the payload's day/tick attributes and updates its
applied generation. Unchanged panels are never re-parsed, so
their images never repaint — the blink fix. A 60 s **poll** of
the same endpoint (a fetch, not a navigation) replaces the
meta-refresh as the convergence backstop for a dropped socket;
the meta-refresh tag is gone from the served page. (The
pre-first-report placeholder keeps its own 5 s refresh: it has
no scripts at all, and its refresh is the only way off it; it
carries no panels or images. That is the interstitial, not the
plan page.)

The only navigation left is a **last resort**: after 3
consecutive *HTTP* failures of the fragment fetch, one guarded
reload — the session breaker from the reload-guard era
(sessionStorage accounting, ≤3 reloads per 30 s window, ≥5 s
spacing, then disabled for the session). Network failures
deliberately never count: reloading into a dead server would
trade a good stale page for a browser error page. When the
breaker trips, only the reload is disabled — fragment sync
(push + poll) continues and can still heal the page.

Fragment swaps re-apply the page's dynamic state through one
re-entry point: the tick/filter/unverified script in
`render.rs` was factored into once-per-element bindings plus
`window.__frackerResync`, which re-reads the (possibly updated)
tick/day payload and re-paints ticks, re-binds and re-applies
the Hunt filter, and re-binds ⍻ rows on swapped-in DOM. Initial
page load runs the same pieces, so swapped panels behave
exactly as freshly loaded ones — same code path, not a mirror.
Tick painting keeps its load semantics verbatim (the server
store is authoritative when served; a box is painted on when
its id is in the store, never forced off).

### Decisions the brief left open

- **Fragment boundaries**: header + the four tab pages, at the
  section functions the page was already composed from. Finer
  boundaries (per-panel) would shrink payloads but multiply
  marker sites; a regen typically rewrites several panels
  anyway, and unchanged panels are already free. Payload size
  was not the defect; navigation was.
- **Markers, not a second render path**: the server still
  reads the one rendered report file; extraction is a pure
  split of what the renderer emitted. No persisted fragment
  store, no render-time plumbing through the resident.
- **Diff retention**: 4 generations. A browser tab left open
  across a leave/weekend still converges (worst case it gets
  every fragment once), because the poll asks every 60 s and
  any unknown generation gets the full set.

## Gates

- New tests: push guard (fetch decision + floor, applied-state
  no-op, failure budget, breaker cap/window/disable, script
  mirror); render (no meta-refresh in the served page, push
  fetches fragments, extraction round-trips all five fragments
  in order, hash identity); server (empty diff when current,
  only-changed-fragment diff, unknown generation → full set,
  tick-payload byte-identity, endpoint over real HTTP).
  Updated pins: the old "meta refresh backstop" and "a push
  reloads the page" assertions now pin the new behaviour.
- Full suite green (see addendum #106 for the counts);
  `cargo build --release --locked --offline` green pre-tag.
- Census gates untouched — no capture or planner changes.
- Live visual verification (no blink on regen) is Seth's,
  post-ship.
