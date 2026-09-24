//! The release version this binary reports.
//!
//! # Why this is not `CARGO_PKG_VERSION`
//!
//! It used to be, and the version lived in `Cargo.toml`, bumped on every
//! release. That made the release bump edit a file the Docker build reads
//! before it compiles dependencies, so the layer cache was invalidated every
//! single time and CI recompiled the whole dependency tree — roughly 15 minutes
//! per deploy that bought nothing. It also meant `Cargo.lock` had to be
//! hand-edited to match, which broke a `--locked` build at least once.
//!
//! So the release version moved to the `VERSION` file at the repository root.
//! CI reads it, tags the image with it, and passes it to the build as
//! `FACILITATOR_VERSION`, which the image carries as an environment variable.
//! `Cargo.toml` now holds a frozen placeholder and stops changing between
//! releases, so the dependency layer survives.
//!
//! A build without `FACILITATOR_VERSION` set — `cargo run` on a workstation —
//! falls back to `CARGO_PKG_VERSION`, which is that frozen placeholder. That is
//! deliberate: a development build should not claim to be a release.

/// Decide the version from a raw `FACILITATOR_VERSION` value.
///
/// Split out from [`facilitator_version`] so the branches are testable: the
/// public function memoises in a `OnceLock`, which resolves once per process and
/// makes an env-driven test order-dependent.
fn resolve(raw: Option<String>) -> String {
    match raw {
        Some(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Release version reported by `/version`, the OpenAPI document, and telemetry.
///
/// Reads `FACILITATOR_VERSION` once. Empty or unset falls back to the compiled
/// placeholder rather than reporting an empty string.
pub fn facilitator_version() -> &'static str {
    use std::sync::OnceLock;
    static VERSION: OnceLock<String> = OnceLock::new();

    VERSION
        .get_or_init(|| resolve(std::env::var("FACILITATOR_VERSION").ok()))
        .as_str()
}

/// What a build that was not told its commit reports as one.
///
/// Seven zeros, the short form of a SHA no commit has: it satisfies the
/// `git_sha` shape the interop manifest requires (`^[0-9a-f]{7,40}$`) while
/// being impossible to mistake for a real commit -- the same choice the
/// frozen `0.0.0` version makes for a development build.
pub const UNKNOWN_GIT_SHA: &str = "0000000";

/// Decide the commit from a raw `FACILITATOR_GIT_SHA` value.
///
/// Anything that is not 7 to 40 hex digits falls back to [`UNKNOWN_GIT_SHA`]
/// rather than being published: the manifest that carries it is public, and a
/// build argument that picked up something else must not end up in it.
fn resolve_git_sha(raw: Option<String>) -> String {
    match raw.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if (7..=40).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_hexdigit()) => v,
        _ => UNKNOWN_GIT_SHA.to_string(),
    }
}

/// The commit this binary was built from, as the interop manifest publishes it
/// (`/.well-known/uvd-stack.json`, `git_sha`).
///
/// Reads `FACILITATOR_GIT_SHA` once. CI passes the pushed commit as a build
/// argument of the runtime stage, next to `FACILITATOR_VERSION`, and for the
/// same reason: declared in the builder stage it would key every compiled
/// layer on the commit.
pub fn facilitator_git_sha() -> &'static str {
    use std::sync::OnceLock;
    static SHA: OnceLock<String> = OnceLock::new();

    SHA.get_or_init(|| resolve_git_sha(std::env::var("FACILITATOR_GIT_SHA").ok()))
        .as_str()
}

#[cfg(test)]
mod tests {
    use super::{resolve, resolve_git_sha, UNKNOWN_GIT_SHA};

    /// What CI passes (`github.sha`, 40 hex), and a short one.
    #[test]
    fn a_commit_in_the_environment_is_reported() {
        let full = "8ee44114da322363095822919e9e51ffbe1f05d5";
        assert_eq!(resolve_git_sha(Some(full.to_string())), full);
        // Build arguments pick up whitespace and upper case easily.
        assert_eq!(resolve_git_sha(Some(" 8EE4411\n".to_string())), "8ee4411");
    }

    /// Unset, blank or not a commit: the placeholder, never the raw value.
    #[test]
    fn anything_that_is_not_a_commit_falls_back_to_the_placeholder() {
        for raw in [
            None,
            Some(""),
            Some("   "),
            Some("abc12"),
            Some("dev"),
            Some("8ee4411 extra"),
        ] {
            assert_eq!(
                resolve_git_sha(raw.map(str::to_string)),
                UNKNOWN_GIT_SHA,
                "{raw:?}"
            );
        }
        assert_eq!(resolve_git_sha(Some("a".repeat(41))), UNKNOWN_GIT_SHA);
    }

    /// What CI actually does: pass the contents of VERSION as the build arg,
    /// which the image carries as an environment variable.
    #[test]
    fn env_value_wins_over_the_placeholder() {
        assert_eq!(resolve(Some("1.74.0".to_string())), "1.74.0");
        // Docker build args and shell pipelines pick up stray whitespace easily,
        // and a version with a newline in it corrupts every consumer of /version.
        assert_eq!(resolve(Some("  1.74.0\n".to_string())), "1.74.0");
    }

    /// An empty or whitespace-only value is a misconfiguration, not a version.
    /// Reporting "" would make /version look broken in a way that reads like the
    /// service is broken.
    #[test]
    fn blank_env_falls_back_instead_of_reporting_empty() {
        assert_eq!(resolve(Some(String::new())), env!("CARGO_PKG_VERSION"));
        assert_eq!(resolve(Some("   ".to_string())), env!("CARGO_PKG_VERSION"));
        assert_eq!(resolve(None), env!("CARGO_PKG_VERSION"));
    }

    /// The fallback must never yield an empty version: an empty `/version` is
    /// harder to diagnose than an obviously-placeholder one.
    #[test]
    fn fallback_is_never_empty() {
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }

    /// The VERSION file is what CI tags and deploys with. A malformed one
    /// produces an image tag nothing can resolve, so keep it parseable.
    #[test]
    fn version_file_is_well_formed() {
        let raw = include_str!("../VERSION");
        let version = raw.trim();

        assert!(!version.is_empty(), "VERSION file is empty");
        assert_eq!(
            raw.matches('\n').count(),
            1,
            "VERSION must hold exactly one line, got {raw:?}"
        );

        let parts: Vec<&str> = version.split('.').collect();
        assert_eq!(
            parts.len(),
            3,
            "expected MAJOR.MINOR.PATCH, got {version:?}"
        );
        for part in parts {
            assert!(
                !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()),
                "non-numeric component in {version:?}"
            );
        }
    }
}
