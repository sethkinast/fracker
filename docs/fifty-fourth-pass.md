# Fifty-fourth pass — 15-minute TTL caches on the DE endpoints

Verification addendum: #120.

## The ask

Seth's decision, 2026-10-28: his IP was rate-limit-blocked
from DE's servers after his heaviest-ever mission day
with Fracker running, and the forum consensus is these
endpoints are heavily rate-limited. Start with 15-minute
caches on both api.warframe.com endpoints. Standing /
focus remaining may now be up to 15 minutes stale —
accepted by Seth, since the report dates a cached
snapshot by its true age.

## 1. The TTL (`src/fetch.rs`)

New `TTL_DE` (15 minutes) and a `ttl_hit` helper shared
by both endpoints: the cache file must be younger than
the TTL at call time AND its contents must pass the
endpoint's validity bar, or the call falls through to
a live fetch. The live and failure paths are untouched:
a live 200 still refreshes the cache, a live failure
still serves a disk cache of any age as CacheFallback,
and a live profile 409 still errors outright, never
masked by cache. The TTL check runs before any network
call, so it cannot mask an error that was never
observed.

Validity bars mirror what the live path already
enforces before caching: worldstate must be valid JSON
carrying a `Time`; a profile cache must parse as a
snapshot (`parse_profile`). A corrupt cache is never
served as a TTL hit — it falls through to live, and on
the failure path behaves exactly as before (worldstate
serves the raw cache as CacheFallback; profile errors
on parse).

Superseded history, kept in the comments: worldstate's
no-TTL rule (addendum #51) and the profile's TTL-free
rule (addendum #71, which existed because DE's own
~5-minute cache made a second 30-minute cache serve
pre-reset numbers after the 00:00 UTC reset). The
15-minute TTL replaces both for the fetch itself;
Xuerian (1 h) and WFCD (7 d) TTLs and all regen
triggers are untouched.

Effect: worst case becomes 4 requests per endpoint per
hour (8 DE requests/hour total), regardless of regen
count, down from 2 per regen unbounded.

## 2. Honest labels, three states distinct

New `FetchStatus::TtlCache { age }`, rendered by
`label()` as "cached (Nm)" (cache-file age in whole
minutes). It is distinct from CacheFallback's
"cached (<reason>)", which keeps its error reason.

The worldstate stamp gained a source enum
(`model::WorldstateSource`: Live / TtlCache /
Fallback) replacing the old `fetched_live` bool in
`worldstate_status_label`:

- Live: "worldstate live (Ns)" while the payload's own
  `Time` is within the 10-minute threshold, else
  "worldstate STALE (Nm)" — unchanged.
- TtlCache: "worldstate cached (Nm)" with the cache
  age — never STALE, even when the payload's `Time`
  is past the live threshold.
- Fallback (failure path or offline fixture):
  "worldstate STALE (Nm)" — unchanged, and STALE stays
  reserved for this path.

The profile status needs no new plumbing: report.rs
already renders `fetched.status.label()` into
"backlog N primes · profile <status>", so a TTL hit
renders "profile cached (Nm)" with no further change.

## 3. Testability without the network

The fetch functions split into thin public wrappers
(`fetch_worldstate`, `fetch_profile`, taking
`SystemTime::now()`) over `_at` variants taking an
explicit `now`, so stale behaviour is testable without
aging files. The gate tests use a dead-proxy client
(every request fails fast at 127.0.0.1:9): with that
client, a TtlCache status is proof the TTL branch ran,
because a bypassed TTL could only produce CacheFallback.

## Gates

- TTL-hit: a fresh cache is served with NO http
  request, for both endpoints, under the dead-proxy
  client.
- Stale-cache: a cache aged 20 minutes past the write
  is refetched (dead proxy → CacheFallback with the
  old text, never a TTL hit).
- Corrupt-cache: fresh-mtime garbage falls through to
  live (worldstate → CacheFallback with the raw cache;
  profile → error on parse).
- Failure: live failure still serves an old cache with
  the CacheFallback labelling, and the offline fixture
  view still renders STALE.
- Labels: live / cached / STALE render distinctly for
  worldstate (including a TTL hit over a payload past
  the live threshold); profile "cached (Nm)" renders;
  a TTL-hit snapshot's daily caps equal the same
  snapshot parsed live.
- TTL decision boundaries, pure: fresh+valid hits;
  stale, invalid, future-mtime, and missing files all
  miss.
- Suite 437 passed / 0 failed (lib 409, +6 over
  v0.5.48; golden 15; packaging 7; resident 5;
  tray_quit 1);
  `cargo build --release --locked --offline` green
  pre-tag, lock diff = version line only.

Live verification owed: Seth plays on this build and
confirms the DE request rate drops (no more blocks)
while the page shows "cached (Nm)" stamps between
regens and "live" stamps at most every 15 minutes;
standing / focus remaining may lag up to 15 minutes
behind the game, by design.
