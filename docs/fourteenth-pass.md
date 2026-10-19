# Worker brief #14 — Fracker fourteenth pass (DE-only worldstate, Quit, capture target, sortie, weeklies audit)

Branch: `opencode/fourteenth-pass` (created from master;
check it out). Read FIRST: `docs/verification-addendum.md`
items 48, 49, 50, 51 (incl. the audit + direction updates).

Seth released this round on 2026-10-19. Five sections; §1 is
the whale.

## §1 Worldstate: DE is the ONLY source (item 51)

Seth's rulings: no fallback chain (complexity), freshness
judged by the data, decodes baked in.

- Replace the warframestat worldstate fetch with DE's own
  feed: `https://api.warframe.com/cdn/worldState.php`
  (same host family as the profile endpoint already used).
  Parse DE's raw schema into the app's existing WorldState
  model: Alerts, ActiveMissions (fissures; tier from
  `Modifier` VoidT1..T6, Steel Path from the entry's own
  fields — map what exists, flag what doesn't), Events +
  Goals (incl. Road To Tau progress), PrimeVaultTraders
  (Varzia), VoidTraders (Baro), Sorties, LiteSorties
  (Archon Hunt), SeasonInfo (Nightwave). Sections the model
  carries today must not regress.
- NO second source and NO silent substitution: if the DE
  fetch fails, the report says worldstate is unavailable /
  stale (with the data's age) in the stamp — it never
  quietly renders old data as current. The disk cache may
  remain ONLY as an explicitly-labeled stale view.
- Freshness: DE's payload carries `Time` (unix seconds).
  If the payload is older than ~10 minutes at render, the
  stamp marks worldstate STALE with its age. (Live incident
  that motivated this: warframestat froze at 03:29:40Z for
  90+ minutes while DE was 69 seconds old.)
- Decodes are BAKED, in-repo, data-owned:
  `data/decode/nodes.json` (SolNode → name/system),
  `data/decode/nightwave.json` (challenge uniqueName →
  name/description), `data/decode/mission_types.json`
  (MT_* → display name). Read `data/decode/README.md` for
  provenance. Validate the MT display names against the
  recorded warframestat fixtures (they carry display
  names); complete/extend the tables from evidence in the
  repo, flag what you cannot verify — never guess a name.
  Node display = `name (system)` as today.
- Bonus this unlocks: EE.log and DE both speak SolNode
  keys — alert completion matching (pass 12 §4) should key
  on them exactly where the data allows.
- Remove the warframestat worldstate path (fetch, TTL,
  fallback) rather than leaving it dormant.
- Tests: DE-schema fixture(s) (record tonight's real DE
  payload shape — a captured sample is fine to synthesize
  from the field inventory in the addendum), freshness
  stamping (fresh vs stale), and the existing golden/
  resident suites adapted to the new source.

## §2 Tray Quit quits (item 48)

The tray menu's Quit item doesn't terminate the resident.
Find why the activation never exits the loop (or what
restarts/holds it) and fix it. A test or a clearly-reasoned
verification note is the gate — Seth will confirm live.

## §3 Capture: give the stream its target (item 49)

From Seth's v0.4.2 log: consent → FD → connect → stream
state 2 → error "target not found". `pw_stream_connect`
never received the node_id from the ScreenCast Start
reply's streams array. Parse it and connect to that
target. State-machine polish from the same log: "capturing"
begins at the first real frame (not at connect), and an
error supersedes any in-flight stage label.

## §4 Sortie placement (item 50a)

In Routine, the Sortie row moves out of the Today standing
block and in with the cadence checklist items.

## §5 Weeklies audit (item 50b)

One entry (Yonta) was a confident fabrication and is
already removed. Audit EVERY remaining entry in
`data/weeklies.toml` against the Warframe wiki
(wiki.warframe.com): vendor, ware, limit, cadence. Correct
the TOML and WEEKLIES.md where the record is wrong; in the
summary, give a per-entry verdict (verified / corrected /
removed) with the wiki fact that decided it. No entry
survives on vibes.

## Gates and deliverables

- `cargo build` clean, `cargo test` green; regenerate
  `docs/demo-plan.html`.
- Logical commits per section; PUSH
  `opencode/fourteenth-pass` and VERIFY with `git ls-remote`.
- Final summary to stdout: §1 source/freshness design +
  decode validation results (incl. anything unmapped),
  §5's per-entry verdict table, and per-section gate
  evidence.
