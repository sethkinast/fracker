# Worker brief #1 — Fracker first pass (platform-independent core)

You are building the first pass of **Fracker**, a Warframe session
planner (Rust). The full product spec is `BRIEF.md` in this repo —
read it end to end before writing code. `DESIGN.md` is the short
summary; `WEEKLIES.md` is the weekly-activity reference. Hard rules
from BRIEF §2 apply to everything you write: clean public data only
(no memory reading, no session nonces, no inventory endpoint, no
credentials anywhere), read-only toward the game and all accounts.

## Scope of THIS pass

The testable core, runnable on this (headless aarch64) box:

1. **Crate scaffold.** Binary crate `fracker`, module layout per
   BRIEF §5 (`watch`, `elog`, `fetch`, `state`, `plan`, `render`,
   `tray`, `portal`). Modules outside this pass's scope exist as
   documented stubs, not dead code with TODOs scattered — one clear
   stub note per module.
2. **Config** per BRIEF §9: TOML, XDG paths, watched reload not
   needed yet; parsing + defaults + unit tests.
3. **Fetchers** per BRIEF §6: world state, public profile, Xuerian
   `/api/load`, Xuerian `relictables.json` + `wishlist.json`, WFCD
   items dataset. On-disk cache with the §6 TTLs, fail-soft to
   cache. HTTP via `reqwest` (rustls). No token is available to you:
   the Xuerian fetcher must work with an empty token by falling back
   to the seed in `data/targets.json`, and say so in its status.
4. **Planner** as a pure function per BRIEF §10 + §17:
   - Pruning of wants against profile XPInfo (roster parentage from
     `data/cache/wishlist.json`).
   - Goal cards + action derivation (§17): scored crack matching
     (chance is a property of the (part, relic) pair — never blind
     name matching), refinement + Void Trace advice (stock-aware
     tier selection), batch quantity, fissure selection cascade
     (expiry → minutes-per-crack for the batch → goal fit → SP
     tiebreak), radshare verdict WITH the last-two-releases gate.
   - Chunk tiers 1–3 (expiring event/alert value, target fissures,
     relic acquisition with co-yield farm scoring). Tier 4/5 may be
     skeletal (sortie chunk + labeled filler) if time-boxed — say
     which in your summary.
   - Prime Resurgence check: `vaultTrader` from world state
     un-parks vaulted goals in the live rotation (§17 addendum).
   - Golden tests: record ONE live world-state response
     (`https://api.warframestat.us/pc/`) into `fixtures/` at build
     time and test the planner against it + the real seed targets.
5. **`elog`**: integrate the upstream `wf-log` crate from
   warframe-lite as a Cargo **git dependency** (pin a rev). If it
   genuinely won't integrate, implement the §7.2 marker catalog
   locally (with attribution comment) and report that you did.
   Session state machine (§7.3) + account-ID candidate ranking
   (§7.4 steps 1–2; the profile-confirmation step needs a real ID
   and is out of scope — structure the code so it slots in).
6. **Renderer + demo mode.** `fracker --demo` renders the checklist
   page from fixture inputs (recorded world state + seed targets +
   an EMPTY profile and UNKNOWN stock) into a single HTML file
   implementing BRIEF §4.2 sections and the §19 art direction
   (dark palette, Rajdhani + system stack, cards not tables,
   chamfered corners, pips/bars, root background + `color-scheme:
   dark` so refreshes never white-flash). Commit the rendered demo
   output at `docs/demo-plan.html` so a human can open it and judge
   the look. The demo is the visual deliverable of this pass —
   make it genuinely presentable, not lorem-ipsum: use the real 49
   wants and the recorded world state.

## Explicitly OUT of scope this pass

Tray/ksni runtime behavior, the EE.log watch loop running against a
live game, Flatpak packaging, OCR, Xuerian write-back, account-ID
profile confirmation. Do not half-build them.

## Environment notes

- This box is aarch64 Ubuntu. If no Rust toolchain is present,
  install rustup (stable) under the opencode user's home and note
  it in your summary.
- Network access to the §6 endpoints is expected; if any endpoint
  is unreachable from this box, build against the committed seed
  data (`data/cache/`) and report which fetches failed.
- EE.log fixtures do NOT exist yet — do not invent log files that
  claim to be real. Parser tests use synthetic lines built from
  the §7.2 documented markers, labeled synthetic.

## Gates and deliverables

- `cargo build` clean (warnings OK if explained), `cargo test`
  green. Do not push a red tree.
- ONE logical commit on branch `opencode/first-pass` (never push
  master), then push.
- Final output: a summary covering what landed per numbered item
  above, what you stubbed or cut and why, the wf-log integration
  outcome, fetcher reachability from this box, and the pre-filled
  PR compare link (branch vs master).
