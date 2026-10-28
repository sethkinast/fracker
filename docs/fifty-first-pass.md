# Fifty-first pass — Log tab, acquisition toast, chime

Verification addendum: #117.

## The ask

Three queue items from Seth, 2026-10-28 (~00:09–00:19), all
with Seth-set criteria:

1. **A Log tab** after Routine that logs state changes — when
   an item is detected, when an alert is seen completed, and
   when a relic crack occurs but no item was detected — "just
   so I can stop having to dig through logs by hand for the
   core events".
2. **An acquisition toast** on detection credits, built to his
   visual spec: bottom right of the screen (~50 px padding off
   the edges), standard blue background, lighter blue border,
   heavy four-sided box-shadow; two asymmetrical columns — the
   left column darker, holding the part's icon at 128×128 with
   generous padding; the right side shows "Relic Cracked" and
   the part name as two lines at larger type.
3. **A chime with the toast** — a soft chime so he doesn't
   have to look. Asset settled by Seth: Freesound #398496,
   "wind chimes - single 04.wav" by Anthousai (a single wind
   chime tube hit, 4.04 s, recorded on a Tascam DR-40, 30 July
   2017). License: **CC0**
   (http://creativecommons.org/publicdomain/zero/1.0/) — no
   attribution required. The same asset is queued as the
   pack-open capture signal in a later pass.

Out of scope: pack-open events in the Log (that pass is queued
separately), and any change to detection, crediting, or the
other tabs' content.

## 1. The event store (`src/events.rs`)

Append-only JSON Lines at `events.jsonl` in the data dir,
beside `stock.json`. One writer (the resident); the report
reads it. Each record:

```json
{"ts":"2026-10-28T05:02:14Z","kind":"credit","part":"Voruna Prime Chassis Blueprint","gear":"Voruna Prime"}
{"ts":"2026-10-28T05:01:03Z","kind":"alert_done","label":"RoadToTauEvent · SolNode175"}
{"ts":"2026-10-28T05:00:41Z","kind":"crack_miss","reason":"name not proven","roll":"Meso C11"}
```

- `ts` is UTC RFC 3339; rendering converts to local time.
- `kind` is one of `credit`, `alert_done`, `crack_miss`
  (serde snake_case). Fields a kind does not use are absent,
  never null.
- **credit** — reward detection credited a part. Written by
  the resident's reward task at the moment of the credit, with
  the part name and its parent item (the toast's icon key,
  resolved best-effort from the cached roster; absent when the
  roster does not know the part — the toast falls back to a
  monogram).
- **alert_done** — an alert seen completed. Written at the
  mission tracker's proof point, when a proven completion's
  armed mission `is_alert()`; the label is the alert's
  identity as known at arm time (event tag, with the node
  appended when known; else the node; else the mission name).
- **crack_miss** — a relic crack episode whose burst resolved
  to a miss. `reason` is a short plain class, never the raw
  diagnostic: `no bordered card`, `no reward screen`, or
  `name not proven` (derived from the burst verdict's own
  phrasing by `reward::miss_reason_class`), plus the resident's
  local classes `no frames captured` and `detection failed`
  for episodes that never reached the burst. `roll` carries
  Seth's roll name when the arm event resolved one (pass 49);
  it is omitted rather than shown as a raw path when it did
  not resolve.

The file is capped at the most recent 500 records: appends are
serialised behind one mutex, and only the append that crosses
the cap rewrites the file down to the tail. A malformed line
is skipped on load and never breaks rendering; a missing file
is an empty log. I/O failures on append are swallowed — the
sidecar discipline, same as the search-index sidecar.

## 2. The Log tab (`src/render.rs`, `src/server.rs`)

`TABS` gains a fifth entry — `("t5","log","Log")` after
Routine — and `FRAGMENT_IDS` a sixth — `p5` after `p4` — so
the Log rides the pass-41 fragment architecture (gen-hash
diff protocol) like every other page. The server fragment
tests' synthetic page grew the matching `p5` block.

Entries render newest-first, one line each: a local timestamp
(`%b %d %H:%M`, e.g. "Oct 28 01:12") and the event in plain
words from `EventRecord::line()`:

- `Relic cracked — Voruna Prime Chassis Blueprint`
- `Alert completed — RoadToTauEvent · SolNode175`
- `Relic crack — no item detected (name not proven) · roll: Meso C11`

Read-only. A fresh install shows the empty state: "No events
yet. Detection credits, alert completions, and missed cracks
will appear here."

## 3. The toast (`src/render.rs`)

The newest credit rides the Log fragment as a hidden beacon
(`#credit-beacon`, carrying the credit's `ts|part` identity,
the part name, the parent item's icon — the app's standing
discipline: part icon = parent item icon, via the same
`gear_icon_src` the cards use — and the credit's epoch). The
toast shell and its script live outside every fragment, like
the search overlay's.

- **Once per credit.** Seen ids persist in localStorage
  (`fracker.toastSeen`, capped at 20). On a live generation
  push, an unseen beacon toasts — it is by construction the
  credit that just landed. On a fresh page load, only a credit
  under ~30 s old toasts; an older unseen credit is marked
  seen silently, so re-renders and late loads never re-toast.
- **Auto-dismiss** after ~6 s with a fade; a newer credit
  replaces the toast's content and restarts the timer.
- **Credits only.** Manual unverified marks and the other two
  event kinds never toast.

Visual spec, point by point against Seth's:

| Spec | As built |
| --- | --- |
| Bottom right, ~50 px off the edges | `#toast{position:fixed;right:50px;bottom:50px}` |
| Standard blue background | `background:var(--panel)` (`#151a23`, the app panels' blue) |
| Lighter blue border | `border:1px solid var(--cy)` (`#aee3ef`, the accent) |
| Heavy box-shadow, all four sides | `box-shadow:0 0 0 1px rgba(174,227,239,.30),0 0 44px 12px rgba(0,0,0,.78)` |
| Left column darker, generous padding | `.ticon{background:#0d1117;padding:18px}` (the app's darkest inset) |
| Part icon 128×128 | `.ticon img{width:128px;height:128px;object-fit:contain}` |
| Right column, two lines, large type | `.tk` "Relic Cracked" — 15 px, letter-spaced, uppercase, accent; `.tp` part name — 700 27 px; column padding 20/28/20/24 px, 8 px line gap |

When the credit carries no parent item, the left column shows
the item's monogram initial at 64 px gold instead of an icon.

## 4. The chime (`assets/sounds/chime.mp3`, `src/server.rs`)

The settled asset is copied into the repo as
`assets/sounds/chime.mp3` (48 kHz stereo MP3, ~184 kb/s,
4.04 s, 94,176 bytes — the high-quality Freesound preview),
embedded in the binary with `include_bytes!`, and served by
the resident at `GET /chime.mp3` as `audio/mpeg` with
`Cache-Control: public, max-age=31536000, immutable` — the one
cacheable route; every other route stays `no-store`. The page
carries `<audio id="credit-chime" src="/chime.mp3"
preload="auto">`; when the toast fires, the script sets
`volume = 0.4`, rewinds, and plays. A rejected `play()` (the
browser's autoplay gate) is swallowed — the toast still shows.

**Browser constraint, recorded:** audio playback requires
prior page interaction (sticky activation). In practice this
is satisfied — Seth types and clicks on the page throughout a
session (the overlay alone guarantees it) — but a credit that
lands before the first interaction of a fresh page load will
toast silently.

## Gates

Suite: 422 passed / 0 failed (lib 394; golden 15; packaging
7; resident 5; tray_quit 1) — +13 tests over v0.5.45. New coverage: the events module (roundtrip +
schema shape — kind snake_case, no null fields; roll-carrying
miss line; malformed-line skipping incl. an unknown kind; the
500-record cap keeping the newest tail; missing file = empty);
`miss_reason_class` pinning all three burst tiers through real
`select_burst_pick` outputs; render tests for the Log tab
(newest-first order, beacon fields, toast shell, and CSS/JS
pins for the spec points above) and the empty state; a
byte-level server test for the chime route (200, `audio/mpeg`,
the immutable cache header, body == the embedded asset).
Census/reward gates untouched. Live verification owed: Seth
cracks a relic on this build and watches for the toast + chime
and the three Log lines after a crack, an alert, and a miss.
