//! Item search overlay (forty-seventh pass) — Seth's spec.
//!
//! An Alfred-style overlay for checking item crack status DURING
//! the 15-second reward window: an icon in the header, or simply
//! starting to type anywhere on the page, pops a ~90%-dark overlay
//! with a big-font, fully-expanded result. Esc or the window losing
//! focus closes it. There is no submit, no selection, and no
//! navigation — typing filters, and exactly ONE best result shows.
//!
//! Matching (Seth, pinned): prefix match by word token, never
//! substring. A name is tokenised into words and the token "Prime"
//! is dropped (it discriminates nothing). A query matches iff
//! EVERY word of the query is a prefix of at least one token of
//! the name — the single-word matcher ANDed per word, order-free
//! (fiftieth pass, Seth: no artificial rules, the simplest
//! implementation). So "axi a2" matches "Axi A2", "Dual Zoren"
//! matches "Dual Zoren Prime", and — the accepted consequence —
//! "Silva Aegis" matches "Silva & Aegis" and reversed word
//! orders match too. A single-word query behaves exactly as it
//! always did: a prefix of ONE token. Minimum query length is
//! [`MIN_QUERY_CHARS`], applied to the query string as a whole
//! (no per-word minimum). Ranking: closest token fit first (a
//! token EQUAL to the query word beats a longer token it
//! prefixes — "Bo" before "Boar"; for a multi-word query the
//! fit is the SUM of the per-word fits), then shorter name,
//! then alphabetical, so the single winner is deterministic.
//!
//! The index itself (every relic and prime item as a fully
//! rendered result block) is rendered by the server once per plan
//! generation into a sidecar file (see [`search_index_path`]) and
//! served by `GET /search-index`; the client PREFETCHES it at page
//! load and refetches when the generation changes, then filters
//! locally — a keystroke during a reward window never waits on the
//! network, and the client never templates plan data (it only
//! shows the winning pre-rendered block). [`best_match`] is the
//! Rust statement of the ranking; [`client_script`] is its
//! JavaScript mirror, kept in step the way push.rs keeps its
//! guard's mirror.

use std::path::{Path, PathBuf};

/// The filter does not engage below this many query characters.
pub const MIN_QUERY_CHARS: usize = 2;

/// The one token dropped from every name's token set: every prime
/// item carries it, so it discriminates nothing (Seth's rule).
pub const EXCLUDED_TOKEN: &str = "prime";

/// Tokenise a display name into its searchable words: split on
/// non-alphanumerics, lowercase, drop the excluded token. "Silva &
/// Aegis Prime" -> ["silva", "aegis"]; "Meso C11" -> ["meso",
/// "c11"].
pub fn name_tokens(name: &str) -> Vec<String> {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .filter(|t| t != EXCLUDED_TOKEN)
        .collect()
}

/// Normalise a raw query for comparison: trimmed and lowercased.
/// The matcher splits the result into words; whitespace runs
/// between words carry no meaning.
pub fn normalise_query(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// How closely `name` fits the (already normalised) query: for
/// EACH word of the query, the smallest `token.len() -
/// word.len()` over the name's tokens the word prefixes — 0 when
/// a token equals the word — summed over the words. `None` when
/// the query is under the minimum length (measured on the whole
/// query string, spaces included) or any word prefixes no
/// token. A single-word query reduces to the original measure.
pub fn token_fit(name: &str, query: &str) -> Option<usize> {
    if query.chars().count() < MIN_QUERY_CHARS {
        return None;
    }
    let tokens = name_tokens(name);
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let mut total = 0usize;
    for word in words {
        let best = tokens
            .iter()
            .filter(|t| t.starts_with(word))
            .map(|t| t.len() - word.len())
            .min()?;
        total += best;
    }
    Some(total)
}

/// The deterministic rank of one matching name: closest token fit,
/// then shorter name, then alphabetical.
pub fn rank_key(name: &str, query: &str) -> Option<(usize, usize, String)> {
    token_fit(name, query).map(|fit| (fit, name.chars().count(), name.to_lowercase()))
}

/// The single best match for `raw_query` among `names`, or `None`
/// when nothing matches. This is the function the gates pin; the
/// client script mirrors it over the rendered blocks' data
/// attributes.
pub fn best_match<'a>(names: &[&'a str], raw_query: &str) -> Option<&'a str> {
    let query = normalise_query(raw_query);
    names
        .iter()
        .filter_map(|n| rank_key(n, &query).map(|k| (k, *n)))
        .min_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, n)| n)
}

/// Where the per-generation search index lives: a sidecar beside
/// the report file (`plan.html` -> `plan-search.html`), written by
/// the report pipeline in the same atomic publish as the page and
/// served by the loopback server's `GET /search-index`. A sidecar
/// (not a block inside the page) keeps the ~800 relic blocks out of
/// the plan page's own payload.
pub fn search_index_path(report_path: &Path) -> PathBuf {
    let stem = report_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("plan");
    report_path.with_file_name(format!("{stem}-search.html"))
}

/// The overlay's page script: triggers (header icon + global
/// typing), local filtering over the prefetched index, Esc/blur
/// close, and the ⍻ toggle binding for the shown result. The
/// matching constants are substituted from the Rust side so the
/// mirror cannot drift from [`best_match`]'s bars.
pub fn client_script() -> String {
    const TEMPLATE: &str = r#"// Item search overlay (forty-seventh pass): Alfred-style, built
// for a one-glance read during the 15 s reward window. An icon in
// the header opens it, and so does simply starting to type — the
// page has no text inputs, so any printable keypress (modifier
// combos ignored) pops the overlay seeded with that character.
// Esc closes it, and so does the window losing focus. There is no
// submit and no selection: typing filters the prefetched,
// server-rendered index LOCALLY (a keystroke never waits on the
// network) and exactly ONE best result shows, fully expanded.
// Matching mirrors search::best_match: every word of the query
// must be a prefix of one word token of the name ("Prime" tokens
// are not indexed); closest token fit (summed over the words)
// wins, then shorter name, then alphabetical.
(function () {
  var MINQ = __MIN_QUERY_CHARS__;
  var overlay = document.getElementById('search-overlay');
  if (!overlay || location.protocol !== 'http:') { return; }
  var queryEl = document.getElementById('search-query');
  var resultEl = document.getElementById('search-result');
  var hintEl = document.getElementById('search-hint');
  var indexHost = document.createElement('div');
  var blocks = [];
  var indexGen = null;
  var fetching = false;
  var query = '';
  var open = false;
  // Prefetch at page load: the overlay exists for the reward
  // window, so the first open must never wait on this fetch.
  // Refetch whenever the fragment sync lands a new generation, so
  // statuses (incl. a ⍻ toggle's regen) stay truthful.
  function fetchIndex() {
    if (fetching) { return; }
    fetching = true;
    var gen = document.body.getAttribute('data-generation') || '';
    fetch('/search-index')
      .then(function (r) { return r.ok ? r.text() : ''; })
      .then(function (html) {
        indexHost.innerHTML = html || '';
        blocks = [];
        indexHost.querySelectorAll('.sr').forEach(function (el) {
          blocks.push({
            el: el,
            name: el.getAttribute('data-name') || '',
            tokens: (el.getAttribute('data-tokens') || '')
              .split(' ').filter(Boolean)
          });
        });
        indexGen = gen;
        if (open) { paint(); }
      })
      .catch(function () {})
      .then(function () { fetching = false; });
  }
  function ensureIndex() {
    var gen = document.body.getAttribute('data-generation') || '';
    if (indexGen !== gen) { fetchIndex(); }
  }
  function fitOf(b, q) {
    var words = q.split(/\s+/).filter(Boolean);
    if (!words.length) { return null; }
    var total = 0;
    for (var i = 0; i < words.length; i++) {
      var w = words[i];
      var best = null;
      b.tokens.forEach(function (t) {
        if (t.indexOf(w) === 0) {
          var d = t.length - w.length;
          if (best === null || d < best) { best = d; }
        }
      });
      if (best === null) { return null; }
      total += best;
    }
    return total;
  }
  function winner(q) {
    if (q.length < MINQ) { return null; }
    var best = null;
    var bestKey = null;
    blocks.forEach(function (b) {
      var f = fitOf(b, q);
      if (f === null) { return; }
      var key = [f, b.name.length, b.name.toLowerCase()];
      if (!bestKey ||
          key[0] < bestKey[0] ||
          (key[0] === bestKey[0] && key[1] < bestKey[1]) ||
          (key[0] === bestKey[0] && key[1] === bestKey[1] &&
            key[2] < bestKey[2])) {
        bestKey = key;
        best = b;
      }
    });
    return best;
  }
  // The ⍻ toggle for the shown item result: the same POST the
  // Hunt rows use, under the same once-per-element guard flag, so
  // the page's own binder and this one can never double-bind a row.
  function bindUv(root) {
    root.querySelectorAll('[data-uv]').forEach(function (row) {
      if (row.__frackerUv) { return; }
      row.__frackerUv = true;
      row.addEventListener('click', function () {
        try {
          fetch('/unverified', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ name: row.getAttribute('data-uv') })
          });
        } catch (e) {}
      });
    });
  }
  function paint() {
    queryEl.textContent = query;
    resultEl.innerHTML = '';
    var q = query.replace(/^\s+|\s+$/g, '').toLowerCase();
    var w = winner(q);
    if (w) {
      var clone = w.el.cloneNode(true);
      clone.classList.add('on');
      resultEl.appendChild(clone);
      bindUv(resultEl);
      hintEl.textContent = '';
    } else {
      hintEl.textContent = q.length < MINQ
        ? 'type at least ' + MINQ + ' characters'
        : 'no match';
    }
  }
  function openOverlay(seed) {
    if (open) { return; }
    open = true;
    query = seed || '';
    overlay.hidden = false;
    ensureIndex();
    paint();
  }
  function closeOverlay() {
    if (!open) { return; }
    open = false;
    overlay.hidden = true;
    query = '';
  }
  document.addEventListener('keydown', function (e) {
    if (open) {
      if (e.key === 'Escape') { closeOverlay(); e.preventDefault(); return; }
      if (e.ctrlKey || e.metaKey || e.altKey) { return; }
      if (e.key === 'Backspace') {
        query = query.slice(0, -1);
        paint();
        e.preventDefault();
        return;
      }
      if (e.key && e.key.length === 1) {
        query += e.key;
        paint();
        e.preventDefault();
      }
      return;
    }
    if (e.ctrlKey || e.metaKey || e.altKey) { return; }
    if (!e.key || e.key.length !== 1) { return; }
    var t = e.target;
    if (t && (t.tagName === 'TEXTAREA' ||
        (t.tagName === 'INPUT' && t.type === 'text'))) { return; }
    openOverlay(e.key);
    e.preventDefault();
  });
  // The trigger icon lives in the header fragment (swapped on
  // regen), so its click is delegated from the document.
  document.addEventListener('click', function (e) {
    if (e.target && e.target.closest &&
        e.target.closest('#search-open')) {
      openOverlay('');
    }
  });
  window.addEventListener('blur', closeOverlay);
  // Chain onto the fragment-sync re-entry point: a new generation
  // invalidates the cached index.
  var prevResync = window.__frackerResync;
  window.__frackerResync = function () {
    if (prevResync) {
      try { prevResync(); } catch (e) {}
    }
    ensureIndex();
  };
  fetchIndex();
})();
"#;
    TEMPLATE.replace("__MIN_QUERY_CHARS__", &MIN_QUERY_CHARS.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned acceptance case (Seth): "Bo" (a melee weapon) sorts
    /// before "Boar" (a shotgun) — a token EQUAL to the query
    /// beats the longer token it prefixes.
    #[test]
    fn bo_sorts_before_boar() {
        let names = ["Boar Prime", "Bo Prime"];
        assert_eq!(best_match(&names, "Bo"), Some("Bo Prime"));
        assert_eq!(best_match(&names, "bo"), Some("Bo Prime"));
        assert_eq!(best_match(&names, "Boa"), Some("Boar Prime"));
    }

    /// Pinned acceptance cases (Seth, fiftieth pass): a query
    /// matches when EVERY word prefix-matches a name token —
    /// the two-word queries that were dead under the whole-query
    /// rule now land, including the relic era+code form.
    #[test]
    fn multi_word_query_matches_per_word() {
        let relics = ["Axi A2", "Meso C11", "Axi A23"];
        assert_eq!(best_match(&relics, "axi a2"), Some("Axi A2"));
        assert_eq!(best_match(&relics, "meso c11"), Some("Meso C11"));
        let items = ["Dual Zoren Prime", "Bo Prime"];
        assert_eq!(best_match(&items, "Dual Zoren"), Some("Dual Zoren Prime"));
        assert_eq!(best_match(&items, "dual zo"), Some("Dual Zoren Prime"));
        // One word that prefixes no token sinks the whole query.
        assert_eq!(best_match(&relics, "axi z9"), None);
        assert_eq!(best_match(&items, "Dual Boar"), None);
        // Single words behave exactly as before.
        assert_eq!(best_match(&items, "zoren"), Some("Dual Zoren Prime"));
    }

    /// Accepted consequences (Seth approved explicitly): the
    /// per-word rule is order-free, so "Silva Aegis" now matches
    /// "Silva & Aegis" and reversed word orders match too.
    #[test]
    fn multi_word_matching_is_order_free() {
        let names = ["Silva & Aegis Prime", "Axi A2"];
        assert_eq!(best_match(&names, "Silva Aegis"), Some("Silva & Aegis Prime"));
        assert_eq!(best_match(&names, "Aegis Silva"), Some("Silva & Aegis Prime"));
        assert_eq!(best_match(&names, "A2 Axi"), Some("Axi A2"));
        assert_eq!(best_match(&names, "Silva"), Some("Silva & Aegis Prime"));
        assert_eq!(best_match(&names, "Aegis"), Some("Silva & Aegis Prime"));
    }

    /// Multi-word ranking extends the closest-fit measure as the
    /// SUM of the per-word fits: exact tokens beat longer tokens
    /// they prefix, word by word.
    #[test]
    fn multi_word_ranking_sums_per_word_fit() {
        let names = ["Axi A23", "Axi A2"];
        assert_eq!(best_match(&names, "axi a2"), Some("Axi A2"),
            "fit 0+0 beats 0+1");
        let names = ["Meso C11", "Meso C1"];
        assert_eq!(best_match(&names, "meso c1"), Some("Meso C1"),
            "fit 0+0 beats 0+1");
        let names = ["Axi A2", "Meso A2"];
        assert_eq!(best_match(&names, "a2"), Some("Axi A2"),
            "single-word ranking unchanged: shorter name on equal fit");
    }

    /// "Prime" is excluded from the token set, so it matches
    /// nothing (it discriminates nothing).
    #[test]
    fn prime_matches_nothing() {
        let names = ["Bo Prime", "Meso C11", "Paris Prime"];
        assert_eq!(best_match(&names, "prime"), None);
        assert_eq!(best_match(&names, "Prime"), None);
        assert_eq!(best_match(&names, "pri"), None);
    }

    /// Below the two-character minimum the filter does not engage.
    #[test]
    fn one_character_query_engages_nothing() {
        let names = ["Bo Prime", "Axi A1"];
        assert_eq!(best_match(&names, "b"), None);
        assert_eq!(best_match(&names, "a"), None);
        assert_eq!(best_match(&names, ""), None);
    }

    /// Matching is case-insensitive on both sides.
    #[test]
    fn matching_is_case_insensitive() {
        let names = ["Mesa Prime", "Meso C11"];
        assert_eq!(best_match(&names, "MESA"), Some("Mesa Prime"));
        assert_eq!(best_match(&names, "mesa"), Some("Mesa Prime"));
        assert_eq!(best_match(&names, "MeSo"), Some("Meso C11"));
        assert_eq!(best_match(&names, "mes"), Some("Meso C11"),
            "equal fit on both: the shorter name wins");
    }

    /// Relic code tokens match like any other token, and an exact
    /// code beats the longer code it prefixes.
    #[test]
    fn relic_code_tokens_match() {
        let names = ["Meso C11", "Axi C1", "Meso A11", "Neo C1"];
        assert_eq!(best_match(&names, "C11"), Some("Meso C11"));
        assert_eq!(best_match(&names, "c11"), Some("Meso C11"));
        assert_eq!(best_match(&names, "C1"), Some("Axi C1"));
        assert_eq!(best_match(&names, "meso"), Some("Meso A11"));
    }

    /// Prefix-on-token only: a substring that is not a token
    /// prefix never matches.
    #[test]
    fn substring_never_matches() {
        let names = ["Boar Prime", "Carrier Prime"];
        assert_eq!(best_match(&names, "oar"), None);
        assert_eq!(best_match(&names, "arr"), None);
        assert_eq!(best_match(&names, "car"), Some("Carrier Prime"));
    }

    /// Ties beyond token fit are deterministic: shorter name,
    /// then alphabetical.
    #[test]
    fn ties_break_shorter_then_alphabetical() {
        // Same fit (token "ab" exact on both), shorter name wins.
        let names = ["Abc Def Prime", "Ab Prime"];
        assert_eq!(best_match(&names, "ab"), Some("Ab Prime"));
        // Same fit and same name length: alphabetical.
        let names = ["Neo Ab", "Axi Ab"];
        assert_eq!(best_match(&names, "ab"), Some("Axi Ab"));
    }

    /// The tokeniser: punctuation splits, case folds, "Prime"
    /// drops, codes survive whole.
    #[test]
    fn tokenisation_rules() {
        assert_eq!(name_tokens("Silva & Aegis Prime"), ["silva", "aegis"]);
        assert_eq!(name_tokens("Meso C11"), ["meso", "c11"]);
        assert_eq!(name_tokens("Axi Vanguard C1"), ["axi", "vanguard", "c1"]);
        assert_eq!(name_tokens("Bo Prime"), ["bo"]);
    }

    /// The sidecar path rides beside the report file.
    #[test]
    fn sidecar_path_sits_beside_the_report() {
        let p = search_index_path(Path::new("/data/fracker/plan.html"));
        assert_eq!(p, PathBuf::from("/data/fracker/plan-search.html"));
    }

    /// The embedded script mirrors the guard constants and the
    /// pinned semantics, so the shipped page cannot drift from
    /// the tested matcher.
    #[test]
    fn client_script_mirrors_the_matcher() {
        let js = client_script();
        assert!(js.contains(&format!("var MINQ = {MIN_QUERY_CHARS};")), "{js}");
        assert!(js.contains("split(/\\s+/)"), "per-word split: {js}");
        assert!(js.contains("indexOf(w) === 0"), "per-word token-prefix test: {js}");
        assert!(js.contains("'/search-index'"), "prefetch route: {js}");
        assert!(js.contains("'/unverified'"), "the ⍻ toggle path: {js}");
        assert!(js.contains("'Escape'"), "esc closes: {js}");
        assert!(js.contains("'blur'"), "blur closes: {js}");
        assert!(js.contains("ctrlKey"), "modifier combos ignored: {js}");
        assert!(js.contains("__frackerResync"), "generation chaining: {js}");
        assert!(!js.contains("__MIN_QUERY_CHARS__"), "tokens substituted");
    }
}
