# Fifty-seventh pass — cache-write-failure hold + lock attempt (fetch failsafe)

Verification addendum: #123.

## The ask

Seth's settled criteria, 2026-11-03 (after the
fetch-failure-mode analysis): the DE rate caps are
enforced by disk cache files (mtime + validity under
`ttl_hit`), and `write_cache` failed silently, so a
broken cache dir could silently uncap live fetching.
Any cache-write failure (worldstate, profile, or proxy
cache file — temp write or rename failing) must latch
an in-memory hold for the life of the process: no
further DE live fetches from it, serving stale cache /
STALE rendering exactly as today's failure path does.
When the hold latches, the process also attempts to
write `de-block.lock`, through the same write path, so
it only lands when the failure is file-specific rather
than directory-wide — but when it lands it protects
restarts too and raises the existing header alert.
Lock-write failure itself also latches the hold. The
failure must land as a line in `resident.log` (not
stdout-only — stdout is invisible from the desktop
tray). Restart clears the hold: accepted residual risk
for rapid restarts.

Explicitly NOT built, per the brief: no failure backoff
for transient outages, no cross-restart hold
persistence, no hold on proxy attempts, no profile
long-TTL direction.

## 1. The hold (`src/fetch.rs`)

`try_write_cache` replaces the old silent `write_cache`:
it returns `Result<(), String>`, naming the failed step
(create dir, tmp write, rename) and the paths involved.
Every caller now sees the error. The in-scope callers
latch on it; the out-of-scope static-data callers
(Xuerian `/api/load`, relictables bake, WFCD items /
wishlist via `fetch_static`) deliberately ignore it with
`let _ =`, exactly as before — those hosts are not DE
traffic, and the brief's trigger list is worldstate,
profile, and proxy cache files only.

Latch state is a process-wide map keyed per cache dir
(`FETCH_HOLDS`). Production uses exactly one cache dir,
so this is process-wide in practice; keying it per dir
(rather than one global flag) keeps tests isolated in
a shared test process, where a global latch from one
test would poison another test's live-fetch assertions.
No test-only reset is needed: every test uses its own
temp dir, so no test can latch another test's dir.

`latch_fetch_hold` runs exactly once per dir per
process: a second failure for the same dir changes
nothing and attempts no further lock write. On latch it
attempts one `de-block.lock` write through the same
write path, with endpoint naming the trigger
("worldstate", "profile", "proxy", or "lock") and
status 0 — a latch lock is not an HTTP block, so no
HTTP status exists to record, and the header renders
it as "HTTP 0 from <trigger>". A lock-write failure
during that attempt is already covered: the hold is
latched regardless, and the outcome is recorded in the
pending log line (see §3).

## 2. Where the hold gates

Both DE paths check the hold before any DE live fetch,
ahead of the lock gate, and the held branch mirrors the
fifty-fifth pass lock-gate branch line for line:

- Worldstate (`fetch_worldstate_de_at`): any-age disk
  cache serves as `CacheFallback` with reason "DE
  requests paused (fetch hold)"; without a readable
  cache it errors exactly as the lock-gate path does
  ("worldstate paused (fetch hold), no cache"). TTL
  hits are suppressed while held, as under the lock —
  the stamp deliberately downgrades to the failure
  rendering while DE is paused, so nothing looks live.
  (When held, the worldstate path skips the lock read, so
  an expired or corrupt lock is not deleted on sight by
  that fetch — `report.rs` still reads the lock after the
  fetches each generation, so cleanup still happens in
  the same generation.)
- Profile (`fetch_profile_url_at`): `profile_cache_fallback`
  with the same "fetch hold" reason — any-age cache
  serves as `CacheFallback`, else the same parse /
  no-cache error the lock path produces.

Latch trigger points, each still serving the current
generation's live data (the hold stops *further*
fetches, it does not discard data already fetched):

- DE worldstate live 200 with a `Time`: cache write
  fails → latch.
- DE profile live parse-OK: cache write fails → latch.
- Proxy live fresh payload: proxy cache write fails →
  latch (the proxy attempt itself already succeeded
  this generation, and proxy attempts continue
  unchanged afterwards — different host, different
  budget; only the DE fall-through is held).
- DE 403/429 on either endpoint: lock write fails →
  latch. Note the sequence is then two lock attempts
  for one latch (the original failed write, then the
  latch's own attempt); both fail the same way when
  the dir is broken, and the latch's attempt is the one
  that would land if the failure were file-specific.

Untouched, line for line:

- TTL behaviour: 15-minute DE TTL, 5-minute proxy TTL,
  and the proxy payload-timestamp bar are unchanged;
  a healthy cache still serves with no request.
- Proxy behaviour: proxy 403/429 still deliberately
  does NOT write the lock, and a proxy failure still
  falls through to DE — unless the hold is latched,
  in which case the fall-through serves stale cache
  with zero DE requests.
- The 409 path still errors outright without locking
  or latching (a 409 writes no cache, so it cannot
  trigger the hold).
- The live 200-without-`Time` worldstate path and the
  profile parse-fail path still cache nothing and
  therefore still latch nothing: the hold closes the
  *uncapped-fetch consequence* of a write failure,
  whatever the payload situation, but it does not
  itself detect invalid payloads. See addendum #123
  for the honest boundary here.

## 3. Observability: the line in `resident.log`

`fetch.rs` owns no logger, so the latch queues its line
in the hold entry's pending log
(`take_fetch_hold_log` drains it). The resident report
job (`spawn_report_job` in `src/resident.rs`) drains
that queue after each generation and records each line
into `resident.log` under area `fetch`, e.g.:

```
<timestamp> [fetch] DE fetch hold latched: worldstate cache write failed (write <cache>/worldstate.tmp: ...); de-block.lock written
```

No new render work: when the latch's lock write lands,
the existing header alert (read by `report.rs` after
the fetches, as in the fifty-fifth pass) already
renders. When the lock does not land, the in-process
hold is the protection and `resident.log` is the
record. Stdout carries nothing new — stdout is
invisible from the desktop tray, which is why the
silent failure was the worst part of the old behaviour.

## 4. Recovery and the accepted residual

Restart clears the hold (in-memory only, by design).
If the disk is fixed, normal operation resumes; if it
is still broken, the first regen fetches once and
re-latches — one live request per restart, not one per
regen. Rapid restarts stay as accepted residual risk:
nothing in-process can distinguish a legit cold start
from restart abuse, and a corrupt cache is unlikely.
Clearing the lock via the tray's Clear block button
does NOT clear the hold (the hold is not the lock);
only a restart does. This asymmetry is deliberate: the
lock is the cross-restart signal the user can clear,
the hold is the in-process failsafe that cannot be
talked out of stopping DE traffic while its own cache
writes are failing.

## 5. Tests

Seven new tests in `src/fetch.rs` (see the
"fifty-seventh pass" section there). Write failure is
forced robustly — tests may run as root, where chmod
read-only dirs still allow writes — by pointing the
cache dir at a regular file (directory-wide failure)
or pre-creating the cache tmp path as a directory
(file-specific failure). Stub hit counts prove zero
network requests on the held path:

- Worldstate tmp-blocked: first fetch serves Fresh,
  latches, lands the lock (endpoint "worldstate",
  status 0), logs one line; held fetches make zero
  requests and serve stale cache as CacheFallback;
  deleting the landed lock and fetching again does
  NOT re-attempt it (one lock attempt per latch).
- Cache dir is a file: fetch serves Fresh, latches,
  no lock can land, log records both the cache
  failure and the failed lock attempt; held fetch
  with no cache errors exactly as the lock-gate path
  does, with zero requests.
- 403 with a file cache dir: the failed lock write
  latches the hold; the next fetch makes zero
  requests.
- Profile tmp-blocked: latches, lands the lock
  (endpoint "profile"), and blocks BOTH endpoints —
  worldstate and profile alike serve stale cache
  with zero requests.
- Proxy tmp-blocked: latches the DE hold and lands
  the lock (endpoint "proxy"); the next generation
  still attempts the proxy (one hit) but its DE
  fall-through makes zero requests and serves stale
  cache.
- Success path: normal worldstate + profile writes do
  not latch, queue no log line, and write no lock.
- `resident.log`: a genuine latch, drained through
  the same helper the resident uses and recorded
  through a real `AppLog`, reads back out of
  `resident.log` as a `[fetch] DE fetch hold latched`
  line naming the failure — not a stdout assertion.

Full suite green; `cargo build --release --locked
--offline` green pre-commit (lock diff = version line
only). Version 0.5.52.
