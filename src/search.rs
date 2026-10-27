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
//! is dropped (it discriminates nothing). The ENTIRE query is
//! compared as one string — it is never tokenised — and a name
//! matches iff the query is a prefix of ONE single token. So
//! "Silva Aegis" does not match "Silva & Aegis" (no token carries
//! it), while "Silva" and "Aegis" each do. Minimum query length
//! is [`MIN_QUERY_CHARS`]. Ranking: closest token fit first (a
//! token EQUAL to the query beats a longer token it prefixes —
//! "Bo" before "Boar"), then shorter name, then alphabetical, so
//! the single winner is deterministic.
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

/// Normalise a raw query for comparison: trimmed and lowercased,
/// otherwise WHOLE — the query is never tokenised, so an inner
/// space stays and can only fail to match (no token contains one).
pub fn normalise_query(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// How closely `name` fits the (already normalised) query: the
/// smallest `token.len() - query.len()` over tokens the query
/// prefixes — 0 when a token equals the query. `None` when the
/// query is under the minimum length or prefixes no token.
pub fn token_fit(name: &str, query: &str) -> Option<usize> {
    if query.chars().count() < MIN_QUERY_CHARS {
        return None;
    }
    name_tokens(name)
        .iter()
        .filter(|t| t.starts_with(query))
        .map(|t| t.len() - query.len())
        .min()
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
// Matching mirrors search::best_match: the whole query must be a
// prefix of one word token of the name ("Prime" tokens are not
// indexed); closest token fit wins, then shorter name, then
// alphabetical.
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
    var best = null;
    b.tokens.forEach(function (t) {
      if (t.indexOf(q) === 0) {
        var d = t.length - q.length;
        if (best === null || d < best) { best = d; }
      }
    });
    return best;
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

    /// Pinned acceptance case (Seth): the query is compared whole,
    /// never tokenised — "Silva Aegis" matches nothing even though
    /// both words are tokens of the name; each word alone matches.
    #[test]
    fn multi_word_query_never_matches() {
        let names = ["Silva & Aegis Prime"];
        assert_eq!(best_match(&names, "Silva Aegis"), None);
        assert_eq!(best_match(&names, "Silva"), Some("Silva & Aegis Prime"));
        assert_eq!(best_match(&names, "Aegis"), Some("Silva & Aegis Prime"));
        assert_eq!(best_match(&names, "sil"), Some("Silva & Aegis Prime"));
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
        assert!(js.contains("indexOf(q) === 0"), "token-prefix test: {js}");
        assert!(js.contains("'/search-index'"), "prefetch route: {js}");
        assert!(js.contains("'/unverified'"), "the ⍻ toggle path: {js}");
        assert!(js.contains("'Escape'"), "esc closes: {js}");
        assert!(js.contains("'blur'"), "blur closes: {js}");
        assert!(js.contains("ctrlKey"), "modifier combos ignored: {js}");
        assert!(js.contains("__frackerResync"), "generation chaining: {js}");
        assert!(!js.contains("__MIN_QUERY_CHARS__"), "tokens substituted");
    }
}
