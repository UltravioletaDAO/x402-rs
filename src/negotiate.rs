//! `Accept` negotiation, so one URL can answer a browser with HTML and an agent
//! with Markdown.
//!
//! WHY THIS IS A PARSER AND NOT `accept.contains("text/markdown")`
//!     A real Chrome sends
//!     `text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8`.
//!     A substring test for `text/markdown` misses on that one, but the mirror
//!     mistake -- testing whether the header *starts with* `text/html` -- is the
//!     one that actually ships, and it hands Markdown to every browser whose
//!     header happens to be ordered differently. The rules below are RFC 9110
//!     section 12.5.1, restated by <https://acceptmarkdown.com/guides/accept-parsing>:
//!     rank by `q`, break ties by specificity, honour `q=0` as a refusal.
//!
//! WHY 406 IS RARE HERE
//!     <https://acceptmarkdown.com/guides/returning-406> names the common bug:
//!     406 returned too eagerly. A missing `Accept`, or `*/*`, is *no
//!     constraint* -- it means "serve your default", not "nothing works". This
//!     module only reports [`Choice::NotAcceptable`] when every representation
//!     the caller was offered is either unmatched or explicitly refused with
//!     `q=0`.

/// What negotiation decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Choice<'a> {
    /// Serve this representation. Always one of the offers, by identity.
    Serve(&'a str),
    /// Every offer was unmatched or refused: answer `406`.
    NotAcceptable,
}

/// How well one `Accept` entry matches one offered media type.
///
/// Ordered so a fully specified type beats `text/*`, which beats `*/*` -- the
/// tie-break of RFC 9110 section 12.5.1 when two entries carry the same `q`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Specificity {
    AnyType,
    AnySubtype,
    Exact,
}

/// The quality of `offer` under `entry`, or `None` when `entry` does not cover it.
///
/// `q` is scaled to thousandths so the comparison stays integer: `q=0.8` is 800.
fn score(entry: &str, offer: &str) -> Option<(u32, Specificity)> {
    let mut parts = entry.split(';').map(str::trim);
    let media = parts.next()?;
    // Any parameter may carry the quality factor; `text/markdown;variant=GFM;q=0.9`
    // is legal, so this scans them all instead of assuming `q` comes first.
    let mut quality = 1000u32;
    for param in parts {
        let (name, value) = match param.split_once('=') {
            Some(pair) => pair,
            None => continue,
        };
        if !name.trim().eq_ignore_ascii_case("q") {
            continue;
        }
        quality = parse_quality(value.trim());
    }

    let (etype, esub) = media.split_once('/')?;
    let (otype, osub) = offer.split_once('/')?;
    let specificity = if etype == "*" && esub == "*" {
        Specificity::AnyType
    } else if esub == "*" && etype.eq_ignore_ascii_case(otype) {
        Specificity::AnySubtype
    } else if etype.eq_ignore_ascii_case(otype) && esub.eq_ignore_ascii_case(osub) {
        Specificity::Exact
    } else {
        return None;
    };
    Some((quality, specificity))
}

/// `q` as thousandths, saturating. An unparseable `q` is treated as absent
/// (`1.0`) rather than as a refusal: guessing "the client refuses this" from a
/// typo is the expensive direction to be wrong in.
fn parse_quality(raw: &str) -> u32 {
    match raw.parse::<f32>() {
        Ok(q) if (0.0..=1.0).contains(&q) => (q * 1000.0).round() as u32,
        _ => 1000,
    }
}

/// Pick the representation to serve.
///
/// `offers` is the server's own preference order, most preferred first: it
/// decides ties, so for `/` it reads `["text/html", "text/markdown"]` and a
/// caller sending `Accept: text/*` gets the HTML a browser came for.
pub fn choose<'a>(accept: Option<&str>, offers: &[&'a str]) -> Choice<'a> {
    let default = || match offers.first() {
        Some(first) => Choice::Serve(first),
        None => Choice::NotAcceptable,
    };
    let accept = match accept.map(str::trim) {
        Some(a) if !a.is_empty() => a,
        // No constraint. Not a refusal -- see the module docs.
        _ => return default(),
    };

    let mut best: Option<(u32, Specificity, usize)> = None;
    for (index, offer) in offers.iter().enumerate() {
        // The best entry covering this offer, by specificity within equal `q`.
        let mut covered: Option<(u32, Specificity)> = None;
        for entry in accept.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            if let Some(candidate) = score(entry, offer) {
                covered = Some(match covered {
                    Some(current) if current.1 >= candidate.1 => current,
                    _ => candidate,
                });
            }
        }
        // `q=0` is an explicit refusal, so an offer scoring zero is not a
        // candidate at all -- it is not merely the least preferred one.
        let (quality, specificity) = match covered {
            Some((q, s)) if q > 0 => (q, s),
            _ => continue,
        };
        let better = match best {
            None => true,
            // Earlier offers win ties: `offers` is the server's preference.
            Some((bq, bs, _)) => quality > bq || (quality == bq && specificity > bs),
        };
        if better {
            best = Some((quality, specificity, index));
        }
    }

    match best {
        Some((_, _, index)) => Choice::Serve(offers[index]),
        None => Choice::NotAcceptable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &[&str] = &["text/html", "text/markdown"];

    #[test]
    fn no_accept_header_serves_the_default() {
        assert_eq!(choose(None, ROOT), Choice::Serve("text/html"));
        assert_eq!(choose(Some(""), ROOT), Choice::Serve("text/html"));
        assert_eq!(choose(Some("  "), ROOT), Choice::Serve("text/html"));
    }

    #[test]
    fn a_wildcard_is_no_constraint_not_a_refusal() {
        assert_eq!(choose(Some("*/*"), ROOT), Choice::Serve("text/html"));
        // curl's default.
        assert_eq!(choose(Some("*/*;q=0.5"), ROOT), Choice::Serve("text/html"));
    }

    /// The header that breaks the naive implementations, verbatim from Chrome.
    #[test]
    fn a_real_browser_still_gets_html() {
        let chrome = "text/html,application/xhtml+xml,application/xml;q=0.9,\
                      image/avif,image/webp,image/apng,*/*;q=0.8,\
                      application/signed-exchange;v=b3;q=0.7";
        assert_eq!(choose(Some(chrome), ROOT), Choice::Serve("text/html"));
    }

    #[test]
    fn an_agent_asking_for_markdown_gets_markdown() {
        assert_eq!(
            choose(Some("text/markdown"), ROOT),
            Choice::Serve("text/markdown")
        );
        assert_eq!(
            choose(Some("text/markdown, text/html;q=0.8"), ROOT),
            Choice::Serve("text/markdown")
        );
        assert_eq!(
            choose(Some("text/markdown, text/plain;q=0.5, */*;q=0.1"), ROOT),
            Choice::Serve("text/markdown")
        );
    }

    /// Order in the header must not decide: `q` does.
    #[test]
    fn quality_beats_position() {
        assert_eq!(
            choose(Some("text/html;q=0.2, text/markdown;q=0.9"), ROOT),
            Choice::Serve("text/markdown")
        );
        assert_eq!(
            choose(Some("text/markdown;q=0.2, text/html;q=0.9"), ROOT),
            Choice::Serve("text/html")
        );
    }

    /// Equal `q`: the more specific entry wins, and only then server preference.
    #[test]
    fn specificity_breaks_a_tie_before_server_preference_does() {
        // Both offers sit under `text/*` at the same q -- server preference decides.
        assert_eq!(choose(Some("text/*"), ROOT), Choice::Serve("text/html"));
        // Markdown is named exactly, HTML only by the wildcard, same q.
        assert_eq!(
            choose(Some("text/*, text/markdown"), ROOT),
            Choice::Serve("text/markdown")
        );
    }

    #[test]
    fn q_zero_is_a_refusal_not_a_low_preference() {
        assert_eq!(
            choose(Some("text/html;q=0, text/markdown"), ROOT),
            Choice::Serve("text/markdown")
        );
        // Refusing everything on offer is the one case that earns a 406.
        assert_eq!(
            choose(Some("text/html;q=0, text/markdown;q=0"), ROOT),
            Choice::NotAcceptable
        );
        assert_eq!(choose(Some("application/pdf"), ROOT), Choice::NotAcceptable);
    }

    /// A single-representation surface. `text/html` names nothing it has, so
    /// that is a genuine 406 -- but no real browser sends that header without a
    /// trailing `*/*`, which is why the refusal stays unreachable in practice.
    #[test]
    fn a_single_representation_surface_refuses_only_what_it_cannot_serve() {
        const MD: &[&str] = &["text/markdown"];
        assert_eq!(choose(Some("text/html"), MD), Choice::NotAcceptable);
        assert_eq!(choose(Some("*/*"), MD), Choice::Serve("text/markdown"));
        assert_eq!(choose(None, MD), Choice::Serve("text/markdown"));
    }

    #[test]
    fn parameters_before_q_do_not_hide_it() {
        // RFC 7764's `variant` parameter sits between the type and `q`.
        assert_eq!(
            choose(
                Some("text/markdown;variant=GFM;q=0.1, text/html;q=0.9"),
                ROOT
            ),
            Choice::Serve("text/html")
        );
    }

    #[test]
    fn a_malformed_q_is_read_as_absent_not_as_a_refusal() {
        assert_eq!(
            choose(Some("text/markdown;q=banana"), ROOT),
            Choice::Serve("text/markdown")
        );
    }

    #[test]
    fn matching_is_case_insensitive_and_whitespace_tolerant() {
        assert_eq!(
            choose(Some("  TEXT/MARKDOWN ;  q=1.0  "), ROOT),
            Choice::Serve("text/markdown")
        );
    }
}
