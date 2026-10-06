//! The Zama `fhe-transfer` switch.
//!
//! Decision 171 (2026-10-06): off. The scheme only ever drew vulnerability
//! scanners, but the owner wanted it controllable rather than deleted, so the
//! proxy in [`crate::fhe_proxy`] stays compiled in and ONE environment
//! variable, [`ENV_ENABLED`], decides whether this facilitator offers it:
//!
//! - `/supported` lists it only while on, and with it `/networks.json`,
//!   `/accepts` and the MCP `x402_supported` tool, which all read that list.
//! - `POST /verify` and `/settle` answer an `fhe-transfer` payment with a 400
//!   `unsupported_scheme` while off; the FHE Lambda is never called.
//! - `/discovery` reads an `fhe-transfer` offer as `settleable: false`,
//!   [`SCHEME_NOT_SERVED`], while off.
//! - Every document compiled into this binary that names it -- the landing,
//!   `/networks`, `/x402`, `/bazaar`, the agent documents, `.well-known/x402`
//!   -- and the OpenAPI prose and MCP tool schemas go through [`surface`] or
//!   [`text`], which strip those passages while off.
//!
//! Off is the DEFAULT: unset, empty, `false`, `0` or anything unrecognised.
//! Only `true` (any case) or `1`, surrounded by any whitespace, turns it on.
//! On, every surface is byte for byte what it was before this switch existed:
//! [`surface`] and [`text`] hand their input back untouched.
//!
//! The Lambda itself is a separate Terraform state with a switch of the same
//! name (`terraform/environments/zama-testnet`). Turning it off, this one goes
//! first; turning it on, this one goes last -- see that stack's README.

use std::borrow::Cow;

use tracing::{info, warn};

/// The variable Terraform sets from `var.enable_zama`.
pub const ENV_ENABLED: &str = "ENABLE_ZAMA";

/// The scheme this switch governs, as it travels on the wire.
pub const SCHEME: &str = "fhe-transfer";

/// The `unsupportedReason` a `/discovery` listing gives an `fhe-transfer`
/// offer while the scheme is off: a scheme this build knows, not served here.
pub const SCHEME_NOT_SERVED: &str = "scheme-not-served";

/// Whether this facilitator offers `fhe-transfer`.
///
/// Read on every call rather than once, like `ENABLE_UPTO`: the environment
/// does not change in a running task, and reading it each time is what lets
/// the tests exercise both states in one process.
pub fn is_enabled() -> bool {
    std::env::var(ENV_ENABLED).is_ok_and(|raw| parse(&raw) == Some(true))
}

/// `Some(true)` / `Some(false)` for a value that says something, `None` for
/// one that does not. `None` is still OFF; it only decides whether startup
/// warns about it.
fn parse(raw: &str) -> Option<bool> {
    let value = raw.trim();
    if value.eq_ignore_ascii_case("true") || value == "1" {
        Some(true)
    } else if value.is_empty() || value.eq_ignore_ascii_case("false") || value == "0" {
        Some(false)
    } else {
        None
    }
}

/// One line at startup saying which way the switch is, and a warning when the
/// value reads as neither: `ENABLE_ZAMA=yes` is OFF, and somebody who typed it
/// expected the opposite.
pub fn log_startup_state() {
    match std::env::var(ENV_ENABLED) {
        Ok(raw) => match parse(&raw) {
            Some(true) => {
                info!("{ENV_ENABLED}=true: fhe-transfer is offered (proxied to the Zama Lambda)")
            }
            Some(false) => info!("{ENV_ENABLED} is off: fhe-transfer is not offered"),
            None => warn!(
                value = %raw,
                "{ENV_ENABLED} is neither true/1 nor false/0; fhe-transfer stays OFF"
            ),
        },
        Err(_) => info!("{ENV_ENABLED} unset: fhe-transfer is not offered"),
    }
}

/// `doc` as this process serves it: untouched while on, without every passage
/// that names the scheme while off.
///
/// For the documents compiled into the binary (`include_str!` constants, or a
/// string stamped once into a `OnceLock`), which is why it can hand back a
/// `&'static str`: the result for each document is computed once and kept, so
/// the precompressed-static cache (keyed by address) keeps working. The set is
/// bounded by what the binary embeds; two first requests that race leak one
/// extra copy, nothing more.
pub fn surface(doc: &'static str) -> &'static str {
    if is_enabled() {
        return doc;
    }
    static GATED: std::sync::OnceLock<dashmap::DashMap<(usize, usize), &'static str>> =
        std::sync::OnceLock::new();
    let cache = GATED.get_or_init(dashmap::DashMap::new);
    let key = (doc.as_ptr() as usize, doc.len());
    if let Some(hit) = cache.get(&key) {
        return *hit;
    }
    let gated: &'static str = match redact(doc) {
        Cow::Borrowed(_) => doc,
        Cow::Owned(stripped) => Box::leak(stripped.into_boxed_str()),
    };
    *cache.entry(key).or_insert(gated)
}

/// [`surface`] for a string built at runtime (OpenAPI prose, MCP tool
/// schemas). Not cached: callers build these once.
pub fn text(s: &str) -> Cow<'_, str> {
    if is_enabled() {
        Cow::Borrowed(s)
    } else {
        redact(s)
    }
}

/// How one passage is found and taken out.
enum Cut {
    /// Replace every occurrence of the first string with the second. A `\n`
    /// in either also matches `\r\n`, for a checkout with CRLF endings.
    Text(&'static str, &'static str),
    /// Drop every whole line that contains the needle.
    Line(&'static str),
    /// Drop the `<div` that opens at the needle through its matching
    /// `</div>`, as whole lines.
    Div(&'static str),
    /// Drop the CSS rule whose selector line contains the needle, through the
    /// line that is only `}`, and one blank line after it.
    CssRule(&'static str),
}

/// Every passage, on every surface, that names the scheme.
///
/// Each one is checked to still match something (`every_cut_still_matches`),
/// so editing a document under one of these turns a test red instead of
/// leaving the scheme quietly advertised; and the served surfaces are checked
/// for leftovers (`handlers::zama_switch_tests`). Add a row here, not a branch
/// in a handler, when a new document mentions the scheme.
const CUTS: &[Cut] = &[
    // The scheme lists: index.md, llms.txt, llms-full.txt.
    Cut::Text(", `fhe-transfer`", ""),
    // skill.md (and llms-full.txt, which concatenates it).
    Cut::Line("- **`fhe-transfer`** — experimental, one testnet."),
    // The MCP tool schemas and the /discovery prose in /docs.
    Cut::Text(" | fhe-transfer", ""),
    Cut::Text(", commerce or fhe-transfer", " or commerce"),
    // /docs: the /supported scheme list and the /settle failure prose.
    Cut::Line("- `fhe_transfer` - FHE encrypted transfer via Zama"),
    Cut::Text(
        " `fhe-transfer` settles on the FHE facilitator's side: its `502` carries\n\
         `retryable: false` unless the request provably never reached it or it answered\n\
         with a `4xx`.",
        "",
    ),
    // The landing: the card, its two CSS rules and its i18n string.
    Cut::Line("<!-- Zama FHE Special Card -->"),
    Cut::Div("<div class=\"network-badge zama\""),
    Cut::CssRule(".network-badge.zama"),
    Cut::Line("\"balances.zamaConfidential\":"),
    // /x402: the scheme's label, in both languages.
    Cut::Line("\"scheme.fhe-transfer\":"),
    // /networks: the sentence about it, in the page and in both dictionaries.
    Cut::Text(
        " <code>fhe-transfer</code> is confidential and experimental.",
        "",
    ),
    Cut::Text(
        " <code>fhe-transfer</code> es confidencial y experimental.",
        "",
    ),
    // /bazaar: the client-side list of schemes this facilitator knows.
    Cut::Text(", 'fhe-transfer'", ""),
    // /.well-known/x402: the scheme's entry, the last one in `schemes`.
    Cut::Text(
        ",\n      {\n        \"name\": \"fhe-transfer\",\n        \
         \"description\": \"Experimental FHE transfer scheme.\",\n        \
         \"networks\": [\n          \"ethereum-sepolia\"\n        ]\n      }",
        "",
    ),
    // /uv.css: two comments that name the landing card.
    Cut::Text("el de Zama (", "el de la tarjeta experimental ("),
    Cut::Text(", la tarjeta de Zama)", ")"),
];

/// `doc` with every [`CUTS`] row applied; borrowed when none matched.
fn redact(doc: &str) -> Cow<'_, str> {
    let mut out = Cow::Borrowed(doc);
    for cut in CUTS {
        if let Some(next) = cut.apply(&out) {
            out = Cow::Owned(next);
        }
    }
    out
}

impl Cut {
    /// The document with every match taken out, or `None` when nothing matched.
    fn apply(&self, doc: &str) -> Option<String> {
        match *self {
            Cut::Text(from, to) => {
                if doc.contains(from) {
                    return Some(doc.replace(from, to));
                }
                let (from, to) = (from.replace('\n', "\r\n"), to.replace('\n', "\r\n"));
                (from.contains('\r') && doc.contains(&from)).then(|| doc.replace(&from, &to))
            }
            Cut::Line(needle) => drop_spans(doc, needle, |doc, at| {
                Some((line_start(doc, at), line_end(doc, at)))
            }),
            Cut::Div(needle) => drop_spans(doc, needle, div_span),
            Cut::CssRule(needle) => drop_spans(doc, needle, css_rule_span),
        }
    }
}

/// Remove the span `span` picks around every occurrence of `needle`.
///
/// `None` when nothing was removed -- either no occurrence, or none whose span
/// could be resolved (an unbalanced `<div`, a rule with no closing brace),
/// which `every_cut_still_matches` reports.
fn drop_spans(
    doc: &str,
    needle: &str,
    span: fn(&str, usize) -> Option<(usize, usize)>,
) -> Option<String> {
    let mut out = String::with_capacity(doc.len());
    let mut kept = 0;
    let mut cursor = 0;
    let mut removed = false;
    while let Some(at) = doc[cursor..].find(needle).map(|i| i + cursor) {
        cursor = at + needle.len();
        let Some((start, end)) = span(doc, at) else {
            continue;
        };
        if start < kept {
            // Inside a span already removed.
            continue;
        }
        out.push_str(&doc[kept..start]);
        kept = end;
        cursor = cursor.max(end);
        removed = true;
    }
    removed.then(|| {
        out.push_str(&doc[kept..]);
        out
    })
}

/// Byte offset of the start of the line holding `at`.
fn line_start(doc: &str, at: usize) -> usize {
    doc[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// Byte offset just past the line break ending the line holding `at`.
fn line_end(doc: &str, at: usize) -> usize {
    doc[at..].find('\n').map_or(doc.len(), |i| at + i + 1)
}

/// The lines from the `<div` at `at` through its matching `</div>`.
fn div_span(doc: &str, at: usize) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut cursor = at;
    loop {
        let open = doc[cursor..].find("<div").map(|i| i + cursor);
        let close = doc[cursor..].find("</div>").map(|i| i + cursor);
        match (open, close) {
            (Some(open), Some(close)) if open < close => {
                depth += 1;
                cursor = open + "<div".len();
            }
            (_, Some(close)) => {
                depth = depth.checked_sub(1)?;
                cursor = close + "</div>".len();
                if depth == 0 {
                    return Some((line_start(doc, at), line_end(doc, close)));
                }
            }
            _ => return None,
        }
    }
}

/// The lines from the selector at `at` through the rule's closing `}`, plus
/// the blank line that separates it from the next rule.
fn css_rule_span(doc: &str, at: usize) -> Option<(usize, usize)> {
    let mut cursor = line_end(doc, at);
    while cursor < doc.len() {
        let end = line_end(doc, cursor);
        if doc[cursor..end].trim() == "}" {
            let after = line_end(doc, end);
            let end = if end < doc.len() && doc[end..after].trim().is_empty() {
                after
            } else {
                end
            };
            return Some((line_start(doc, at), end));
        }
        cursor = end;
    }
    None
}

/// Serialises the tests that set [`ENV_ENABLED`]. Shared by every module that
/// tests both states, so a parallel `cargo test` cannot read one test's value
/// in another (CI runs `--test-threads=1`; a laptop may not).
#[cfg(test)]
pub(crate) static TEST_FLAG: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `f` with [`ENV_ENABLED`] set to `value` (unset for `None`), restoring
/// whatever was there. Holds [`TEST_FLAG`] for the duration.
#[cfg(test)]
pub(crate) fn with_flag<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _guard = TEST_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    let previous = std::env::var(ENV_ENABLED).ok();
    match value {
        Some(v) => std::env::set_var(ENV_ENABLED, v),
        None => std::env::remove_var(ENV_ENABLED),
    }
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    match previous {
        Some(v) => std::env::set_var(ENV_ENABLED, v),
        None => std::env::remove_var(ENV_ENABLED),
    }
    out.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off unless it says on. The value Terraform writes is `tostring(bool)`,
    /// so `true`/`false`; the rest is what a hand edit of a task definition or
    /// a `.env` file produces.
    #[test]
    fn only_true_or_one_turns_it_on() {
        for on in ["true", "TRUE", "True", " true ", "\ttrue\n", "1", " 1 "] {
            assert!(with_flag(Some(on), is_enabled), "{on:?} must turn it on");
        }
        for off in [
            "false", "FALSE", "0", "", "  ", "yes", "on", "enabled", "ture", "2", "-1", "truee",
            "true1", "t r u e",
        ] {
            assert!(
                !with_flag(Some(off), is_enabled),
                "{off:?} must leave it off"
            );
        }
        assert!(!with_flag(None, is_enabled), "unset must be off");
    }

    /// Startup warns only for a value that reads as neither on nor off.
    #[test]
    fn a_value_that_says_nothing_is_told_apart_from_off() {
        assert_eq!(parse("false"), Some(false));
        assert_eq!(parse(" 0 "), Some(false));
        assert_eq!(parse(""), Some(false));
        assert_eq!(parse("yes"), None);
        assert_eq!(parse("TRUE"), Some(true));
    }

    /// On, nothing is touched: not a byte, and not a copy either.
    #[test]
    fn on_hands_every_document_back_untouched() {
        const DOC: &str = "- **Schemes:** `exact`, `upto`, `escrow`, `commerce`, `fhe-transfer`\n";
        let served = with_flag(Some("true"), || surface(DOC));
        assert!(std::ptr::eq(served, DOC));
        assert!(matches!(
            with_flag(Some("true"), || text(DOC)),
            Cow::Borrowed(_)
        ));
    }

    /// Off, a document that does not name the scheme is the same allocation.
    #[test]
    fn off_leaves_an_unrelated_document_alone() {
        const DOC: &str = "nothing about that scheme here\n";
        assert!(std::ptr::eq(with_flag(None, || surface(DOC)), DOC));
    }

    #[test]
    fn a_line_cut_takes_the_whole_line_and_only_it() {
        let doc = "a\n- **`fhe-transfer`** — experimental, one testnet.\nb\n";
        assert_eq!(redact(doc), "a\nb\n");
        let crlf = "a\r\n- **`fhe-transfer`** — experimental, one testnet.\r\nb\r\n";
        assert_eq!(redact(crlf), "a\r\nb\r\n");
    }

    #[test]
    fn a_div_cut_follows_nesting_to_the_matching_close() {
        let doc = "<div id=\"wall\">\n  <div class=\"network-badge zama\" x>\n    <div>\n      <span>inner</span>\n    </div>\n  </div>\n  <div class=\"network-badge base\"></div>\n</div>\n";
        assert_eq!(
            redact(doc),
            "<div id=\"wall\">\n  <div class=\"network-badge base\"></div>\n</div>\n"
        );
    }

    /// A card that never closes is left alone rather than eating the page.
    #[test]
    fn an_unbalanced_div_is_not_cut() {
        let doc = "<div class=\"network-badge zama\">\n  <div>\n";
        assert!(Cut::Div("<div class=\"network-badge zama\"")
            .apply(doc)
            .is_none());
    }

    #[test]
    fn a_css_cut_takes_the_rule_and_the_blank_line_after_it() {
        let doc = "  .a {\n    x: 1;\n  }\n\n  .network-badge.zama {\n    y: 2;\n  }\n\n  .network-badge.zama:hover {\n    z: 3;\n  }\n\n  .b {\n  }\n";
        assert_eq!(redact(doc), "  .a {\n    x: 1;\n  }\n\n  .b {\n  }\n");
    }

    /// The `\n` in a text cut also matches a CRLF checkout.
    #[test]
    fn a_multiline_text_cut_matches_crlf_too() {
        let lf = "x,\n      {\n        \"name\": \"fhe-transfer\",\n        \"description\": \"Experimental FHE transfer scheme.\",\n        \"networks\": [\n          \"ethereum-sepolia\"\n        ]\n      }\n    ]";
        assert_eq!(redact(lf), "x\n    ]");
        let crlf = lf.replace('\n', "\r\n");
        assert_eq!(redact(&crlf), "x\r\n    ]");
    }

    /// Every document a cut is written against: the static files the binary
    /// embeds, and the Rust sources that hold the OpenAPI prose and the MCP
    /// tool schemas (their strings are written there verbatim).
    const CORPUS: &[(&str, &str)] = &[
        ("static/index.html", include_str!("../static/index.html")),
        ("static/index.md", include_str!("../static/index.md")),
        ("static/llms.txt", include_str!("../static/llms.txt")),
        (
            "static/llms-full.txt",
            include_str!("../static/llms-full.txt"),
        ),
        ("static/skill.md", include_str!("../static/skill.md")),
        ("static/bazaar.html", include_str!("../static/bazaar.html")),
        (
            "static/networks.html",
            include_str!("../static/networks.html"),
        ),
        ("static/x402.html", include_str!("../static/x402.html")),
        ("static/uv.css", include_str!("../static/uv.css")),
        (
            "static/.well-known/x402",
            include_str!("../static/.well-known/x402"),
        ),
        ("src/openapi.rs", include_str!("openapi.rs")),
        ("src/mcp.rs", include_str!("mcp.rs")),
    ];

    /// A cut that matches nothing is a document edited under it, and the
    /// scheme it was written to remove is advertised again with the switch
    /// off. Red here instead of quietly back on the landing.
    #[test]
    fn every_cut_still_matches() {
        for (index, cut) in CUTS.iter().enumerate() {
            let hits: Vec<&str> = CORPUS
                .iter()
                .filter(|(_, doc)| cut.apply(doc).is_some())
                .map(|(name, _)| *name)
                .collect();
            assert!(
                !hits.is_empty(),
                "CUTS[{index}] matches no document any more: rewrite it against the new text"
            );
        }
    }

    /// Off, none of the embedded documents names the scheme or Zama any more.
    /// (The Rust sources in [`CORPUS`] are only there for their prose; the
    /// served OpenAPI and MCP documents have tests of their own.)
    #[test]
    fn off_no_embedded_document_names_the_scheme() {
        for (name, doc) in CORPUS
            .iter()
            .filter(|(name, _)| name.starts_with("static/"))
        {
            let gated = redact(doc).to_ascii_lowercase();
            for word in ["fhe-transfer", "fhe_transfer", "zama"] {
                assert!(!gated.contains(word), "{name} still names {word} when off");
            }
        }
    }
}
