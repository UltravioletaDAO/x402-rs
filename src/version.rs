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

#[cfg(test)]
mod tests {
    use super::resolve;

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
