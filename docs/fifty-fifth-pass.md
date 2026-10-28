# Fifty-fifth pass — warframestat.us in front of DE + 403/429 lockfile alert

Verification addendum: #121.

## The ask

Seth's settled criteria, 2026-10-28: warframestat.us/pc
first for worldstate only (5-minute cache; stale =
payload timestamp older than 15 minutes, a STARTING
value — "we'll iterate"); profile stays DE-direct;
source-distinguished worldstate labels; a 403/429 on
either DE endpoint writes one lockfile blocking both
DE endpoints, with a header alert and a clear button,
24-hour expiry, and no effect on the proxy path; the
User-Agent derived from the package version.

## 1. Proxy resolution order (`src/fetch.rs`)

`fetch_worldstate_resolved_at_with_urls` runs, per
generation: (a) a fresh proxy cache (`worldstate-wfs.json`,
mtime < 5 minutes AND payload timestamp fresh) serves
with no request; (b) a live `https://api.warframestat.us/pc`
attempt serves only when it succeeds AND its payload
timestamp is fresh (age ≤ 15 minutes), writing the
5-minute cache on fresh only; (c) anything else —
network failure, non-200, unparseable, missing
timestamp, stale timestamp — falls through to the DE
path exactly as in pass 54 (DE 15-minute cache → DE
live → failure fallback). A STALE proxy cache is NEVER
served: staleness is the reason for falling through,
not a value to display. The 5-minute cache bounds
warframestat.us to 12 requests/hour regardless of
regen count. The proxy attempt uses the same shared
client as every other fetch (see §5).

## 2. Payload mapping (`src/model.rs`, `fixtures/wfcd-pc-sample.json`)

WFCD's `/pc` JSON is the parsed/normalized form, NOT
DE's raw worldState.php shape, so `from_de_json` does
not apply. The mapping (`from_wfcd_json` + the private
`WfcdWorld` serde struct) was built against a live
sample captured 2026-10-28 (payload timestamp
`2026-10-28T20:49:56.000Z`, 179,120 bytes), committed
as `fixtures/wfcd-pc-sample.json`. Every consumed
section witnessed in that sample:

- Fissures: 33, tiers incl. Omnia and Requiem, 12
  storms, 13 hard (Steel Path). Storm fissures carry
  their mission type here while DE leaves it empty,
  so timing lookups (`minutes_for_batch`, `is_endless`,
  `base_minutes`) resolve for storms on this path —
  an accuracy gain, recorded as a deliberate source
  difference.
- Alerts: 3, ids in the same 24-hex format as DE
  `_id` (so EE.log `alertId` matching still works),
  counted payouts under `reward.countedItems`, tags.
- Sortie: 3 variants (node + type; boss not modelled).
- Archon: 3 missions — see the correction in §6.
- Nightwave: season 18, 10 active challenges, daily
  and elite flags, `standing` from the `reputation`
  alias the model already carried.
- Baro: singular `voidTrader` (Larunda Relay
  (Mercury) in the sample) falling back to the first
  of `voidTraders` (1); no `active` field, so the
  model's `is_live` falls back to the window check,
  same as the DE path.
- Vault trader: Varzia, 22 inventory items, 6
  containing `/Projections/` — the only ones
  `VarziaAvailability` counts, matching on the
  uniqueName tail after `/Projections/`.
- Events: 3, from WFCD `events` (the parser's Goals
  output — same underlying data as DE `Goals`).
  `WorldState.events` has no planner/render consumer
  (render's `ctx.events` is the events.jsonl log), so
  mapping it is parity, harmless.

Deliberate differences from the DE path, each
recorded so a later diff between sources is
explicable, not mysterious:

- `sol_node` stays EMPTY. WFCD's `nodeKey` is a
  display name ("Nimus (Eris)"), NOT a SolNode key,
  and the completion matcher (`elog.rs`
  `matches_alert`) skips its node check when
  `sol_node` is empty — mapping the display name in
  would make the guard refuse real corroborations.
- `VaultTrader.character` is WFCD's ("Varzia"); DE
  leaves it empty. Nothing consumes `character`.
- `WORLDSTATE_STALE_AFTER_MINUTES` = 10 (the stamp
  live threshold) is distinct from the 15-minute
  fetch freshness bar.

Strictness: `timestamp`, `fissures`, and `alerts`
are required, every fissure needs id/node/tier/expiry
and every alert needs id/expiry/mission-node —
anything less is `Err`, so the caller falls through
to DE rather than rendering degraded worldstate.
Other sections tolerate absence (empty), as the DE
path does with gaps. `report.rs` §1 adds one more
fall-through: a proxy payload that passes the
timestamp bar but fails section validation falls
through to a DE fetch and re-labels the stamp to the
DE source that actually served, rather than failing
the whole report.

## 3. The lockfile (`src/fetch.rs`)

Location: the cache dir (`<cache_dir>/de-block.lock`,
`DE_LOCK_NAME`) — the brief allowed the data dir or
the cache dir, whichever the fetch plumbing already
threads; the fetch plumbing threads the cache dir.
Contents: endpoint, status, observed time as unix
seconds, one per line.

The DE fetch functions are the single choke point:
the lock gate sits at the top of
`fetch_worldstate_de_at` and `fetch_profile_url_at`.
With a live lock held they make NO network request:
any-age disk cache serves as `CacheFallback` with
reason "DE requests paused (lockfile)", else the
existing unreachable error (the report layer renders
unavailable). The intercept runs before
`error_for_status` (worldstate) and via a new
`LiveFail::Blocked(u16)` variant (profile): a 403 or
429 writes the lock ("worldstate"/"profile" + status
+ now) and serves cache in the SAME generation, so
the header alert (§4) is already in place by render
time. A 403/429 from warframestat.us deliberately
does NOT write the lock — it just falls through to
DE. The profile 409 rule is untouched (still
`Definitive`, errors outright, never a lock).
Expiry: a lock older than 24 hours is ignored as if
absent AND deleted on sight during a generation that
reads it; a future-dated lock counts as age zero
(clock skew), live, not expired.

## 4. Alert + clear (`src/render.rs`, `src/server.rs`, `src/resident.rs`)

`RenderContext` gained `de_lock: Option<DeLockAlert>`
(endpoint, status, pre-formatted UTC observed time).
The alert renders inside the header fragment, after
the stamp div and before `FRAG_CLOSE`, so fragment
pushes update it: "DE requests paused: HTTP {status}
from {endpoint} at {observed} — the page is serving
cache/proxy data until this clears (or 24h pass).
[Clear block]". `report.rs` §1 reads the lock AFTER
both DE fetches and before render — the brief's
"read at generation start" is implemented as
read-before-render, which satisfies the observable
requirement: the alert renders in the SAME generation
that observed the block, and on every later
generation while the lock holds.

Clear: `POST /clear-de-lock`, following the `POST
/unverified` route + page-JS pattern. `DeLockClear`
carries the lockfile path and a regen hook; a
successful delete returns 200 and fires the hook
(the resident passes a `Trigger::Manual` send, so
the header re-renders without the alert promptly),
while deleting a missing lockfile is a 404 (the
button only renders while a live lock holds, so a
second clear means something else already cleared
it). The page script (`DE_LOCK_JS`) POSTs and hides
the alert on success; on a file:// page there is no
server, so a failed POST simply leaves the alert in
place. Clearing removes the BLOCK, not the TTL
caches: the next regen still serves a fresh-enough
cache before hitting DE again. The resident wires
the route with the lockfile path from
`cfg.cache_dir`; `serve()` gained a seventh
parameter for it, and the 11 test `serve()` call
sites plus the golden test's `RenderContext` were
updated for the new parameter/field.

## 5. User-Agent (`src/fetch.rs`)

`user_agent()` derives from the package version at
build time (`env!("CARGO_PKG_VERSION")`) so it never
goes stale again — it previously said `fracker/0.1`
while the app was at 0.5.49. After this pass:
`fracker/0.5.50 (warframe session planner)`. The
same shared client carries it on all endpoints.

## 6. Corrections made during this pass

- **Archon mapping.** The working note said
  `archonHunt` uses `variants` (DE LiteSorties uses
  `Missions`). The committed sample proves the
  reverse: WFCD `archonHunt` leaves `variants` empty
  and populates `missions` (3 items), while the daily
  sortie populates `variants` (3) and leaves
  `missions` empty. The mapper now reads both via a
  dedicated `WfcdArchon` struct, with `missions`
  winning when present and `variants` as fallback —
  and the gate test asserting 3 archon variants caught
  the original mis-mapping before it shipped.
- **`read_de_lock` corrupt-delete fix.** The function's
  doc comment promised a corrupt lock is "ignored AND
  deleted on sight", but its early `?` returns skipped
  the delete for short files, bad statuses, and bad
  timestamps. The parse now runs inside a closure so
  EVERY invalid outcome deletes the file; the
  expired/empty-endpoint cases keep their existing
  delete. Behaviour for valid and expired locks is
  unchanged.
- **Test-only fixes.** Two WFS cache tests captured
  `now` before writing the cache file, so the file's
  mtime postdated `now` and the TTL hit correctly
  failed closed; the tests now fetch with a fresh
  clock reading, ordering time the way a real
  generation does (the stale-cache test's fetch
  likewise uses a fresh reading, so payload staleness
  — not the mtime artifact — is what rejects the
  hit). The render alert test asserted on the bare
  `de-lock-alert` string, which the always-emitted
  CSS rule and clear script also contain; it now
  asserts on the alert div's class attribute.
- **Test seam.** `fetch_worldstate_resolved_at_with_urls`
  overrides BOTH the proxy and DE URLs (the
  single-URL variant delegates with the real DE URL),
  so no test touches the real endpoints.

## 7. Gates

- WFCD fixture parses and maps: every consumed
  section witnessed in the sample (fissures incl.
  tier / Steel Path / storm flags, alerts, sortie,
  nightwave, Baro, vault trader) — `model` tests
  assert exact counts (33 fissures, 3 alerts, 3
  sortie variants, 3 archon missions, season 18, 22
  vault items) plus the deliberate `sol_node`-empty
  rule; a refusal test proves missing timestamp /
  fissures, a tier-less fissure, and a node-less
  alert are all `Err`.
- WFS stale: a 20-minute-old payload timestamp falls
  through to the stubbed DE path (proxy hit 1, DE
  hit 1, origin De).
- WFS failures: non-200 (incl. a proxy 403, which
  writes NO lock), unparseable, missing timestamp,
  and a genuine network failure (connection-refused
  loopback URL) each fall through to DE.
- WFS fresh cache: served with ZERO requests
  (stub hits 0); a live WFS serve writes the cache
  and the next generation serves it with no further
  request.
- WFS stale cache: fresh mtime + stale payload
  timestamp is never served (origin De).
- Lock: simulated DE 403 (worldstate) and 429
  (profile) each write the lockfile with the right
  endpoint/status; while held, DE fetch attempts
  make ZERO requests and report the lockfile reason;
  the profile 429 lock blocks a worldstate fetch too
  (one lock, both endpoints); a 24-hour-old lock is
  ignored and removed, as is a corrupt one; the
  clear route deletes the lockfile, fires regen, and
  answers 404 on a second clear; a profile 409 still
  errors outright with no lock written.
- Labels: all five worldstate stamp states render
  distinctly — DE live, DE cached (Nm), proxy live
  ("worldstate live via warframestat.us (Ns)"),
  proxy cached ("worldstate cached via
  warframestat.us (Nm)"), STALE/Fallback.
- UA: the string carries the package version exactly.
- Full suite green (solo): 453 passed / 0 failed
  (lib 425, +16 over v0.5.49; golden 15; packaging 7;
  resident 5; tray_quit 1); `cargo build --release
  --locked --offline` green pre-tag, lock diff =
  version line only. All v0.5.49 behaviour otherwise
  unchanged.

## Live verification owed

Seth plays on this build and confirms: proxy-live
stamps in normal play, DE stamps when the proxy
fails or goes stale, daily caps still DE-direct;
a real DE 403/429 (hopefully never) produces the
header alert in the same generation and the clear
button removes it; no further DE blocks; the
installed app reports 0.5.50. The pass-54 residual
(installed app reports 0.5.49) is superseded by the
0.5.50 install check — the 0.5.49 asset question is
moot once 0.5.50 installs and reports its own
version.
