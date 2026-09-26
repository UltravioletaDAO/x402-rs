//! `GET /.well-known/uvd-stack.json`: the facilitator's stack interop manifest.
//!
//! One document that tells another app of the stack, in a single read, who
//! this service is, where its doors are and how they authenticate, whether it
//! charges, how hard it can be called and how to tell whether it is up. The
//! format is `uvd.stack/1`, fixed by the stack's interop specification
//! (`interop/01-descubrimiento.md` in `uvd-x402-sdk-python`); the schema is
//! vendored under `tests/fixtures/interop/` with the commit it came from in
//! `ORIGEN.json`, and the tests below validate this document against it.
//!
//! # Generated, not written
//!
//! Nothing here is a copy of a fact kept somewhere else:
//!
//! - `rate_limits` is every bucket the router mounted, read from
//!   [`crate::rate_policy::mounted`] -- the same [`crate::rate_policy::Limit`]
//!   that sizes each bucket and stamps `RateLimit-Policy` on its responses,
//!   and that only a budget of [`crate::rate_policy::BUDGETS`] can make. So
//!   it publishes the budgets `GET /config` does, with the same numbers. A
//!   bucket added to the router appears here without anyone editing this file.
//! - `version` and `git_sha` are what the running binary was built as
//!   ([`crate::version`]).
//! - every URL is a path the router serves. axum will not list its own routes,
//!   so `every_url_the_interop_manifest_publishes_is_served` (in `mcp.rs`'s
//!   tests, where the facilitator routers can be stood up over a stub) walks
//!   each one through the real routers and fails on a 404.
//!
//! # What it declares, and why
//!
//! - **`app: "facilitator"`** -- the id the stack's registry already uses for
//!   this service, so `UVD_FACILITATOR_URL` is the override the SDK directory
//!   reads (R1.10, R8.1).
//! - **`auth: ["none"]` on both doors** -- the facilitator authenticates no
//!   caller: the payer's signature inside the payload is the only authority.
//!   The admin routes' bearer tokens are for the operator, not a mode another
//!   service can use, and those routes answer 404 without one.
//! - **`identity.service_signer: null`** -- it signs no request as itself
//!   (R2.5). Its settlement wallets pay gas; they are not an ERC-8128 identity.
//! - **`payments.charges: false`** -- no route answers 402 (R1.7). Its
//!   `/.well-known/x402` exists to say exactly that, and is linked as
//!   `x402_discovery`, not as a price list.
//! - **`events` empty** -- `/events` is a lossy SSE stream in its own format,
//!   not a `uvd.event/1` feed; the interop spec treats it as "the chain is the
//!   cursor" (R5.15). It is linked as `events_stream`.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::rate_policy::{Door, Mount};

/// Where the manifest is served.
pub const MANIFEST_PATH: &str = "/.well-known/uvd-stack.json";

/// The manifest format this module writes.
pub const SCHEMA: &str = "uvd.stack/1";

/// This service's id in the stack (R1.10).
pub const APP_ID: &str = "facilitator";

/// The public origin every URL in the manifest is written against.
pub const PUBLIC_URL: &str = "https://facilitator.ultravioletadao.xyz";

/// `endpoints.mcp`: `POST /mcp`.
pub const MCP_PATH: &str = "/mcp";

/// `health.live`: a constant answer, no I/O (R7.5).
pub const HEALTH_LIVE_PATH: &str = "/health";

/// `health.ready`: RPC reachability and signer gas per chain (R7.5).
pub const HEALTH_READY_PATH: &str = "/health/ready";

/// `links`: the other surfaces an agent reads, by the names the manifest gives
/// them. Each is a route; see the module docs for how that is checked.
pub const LINKS: &[(&str, &str)] = &[
    ("llms_txt", "/llms.txt"),
    ("api_catalog", "/.well-known/api-catalog"),
    ("openapi", "/openapi.json"),
    ("agent_card", "/.well-known/agent-card.json"),
    ("mcp_server_card", "/.well-known/mcp/server-card.json"),
    ("skill", "/skill.md"),
    ("auth", "/auth.md"),
    ("x402_discovery", "/.well-known/x402"),
    ("supported", "/supported"),
    ("events_stream", "/events"),
];

/// Every path the manifest points at, for the test that walks them.
#[cfg(test)]
pub fn published_paths() -> Vec<&'static str> {
    let mut paths = vec![MANIFEST_PATH, MCP_PATH, HEALTH_LIVE_PATH, HEALTH_READY_PATH];
    paths.extend(LINKS.iter().map(|(_, path)| *path));
    paths
}

fn url(path: &str) -> String {
    format!("{PUBLIC_URL}{path}")
}

/// `rate_limits`: one entry per limit and door, identical entries once.
///
/// The manifest's entry carries no name and no route, so two limiters with the
/// same numbers on the same door are the same statement; which one governs a
/// given response is what `RateLimit-Policy` on that response says.
fn rate_limits(mounts: &[Mount]) -> Vec<Value> {
    let mut seen: Vec<(Door, u64, u64)> = Vec::new();
    let mut out = Vec::new();
    for mount in mounts {
        let key = (mount.door, mount.limit.quota(), mount.limit.window_s());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(json!({
            "scope": "ip",
            "applies_to": mount.door.as_str(),
            "limit": key.1,
            "window_s": key.2,
        }));
    }
    out
}

/// The manifest, from the limiters `mounts` lists and the build's identity.
///
/// Pure, so the tests can build it from a known set of limiters; the served
/// document is [`served_document`].
pub fn manifest(mounts: &[Mount], version: &str, git_sha: &str, generated_at: &str) -> Value {
    let links: serde_json::Map<String, Value> = LINKS
        .iter()
        .map(|(name, path)| ((*name).to_string(), Value::String(url(path))))
        .collect();
    json!({
        "schema": SCHEMA,
        "app": APP_ID,
        "name": "Ultravioleta DAO x402 Facilitator",
        "version": version,
        "git_sha": git_sha,
        "generated_at": generated_at,
        "endpoints": {
            "api": { "url": PUBLIC_URL, "auth": ["none"] },
            "mcp": { "url": url(MCP_PATH), "auth": ["none"] },
        },
        "identity": { "service_signer": null },
        "payments": { "charges": false },
        "rate_limits": rate_limits(mounts),
        "events": { "emits": [], "consumes": [], "envelopes": [] },
        "health": {
            "live": url(HEALTH_LIVE_PATH),
            "ready": url(HEALTH_READY_PATH),
        },
        "links": links,
    })
}

/// RFC 3339, UTC, whole seconds, with `Z` -- the one timestamp shape the
/// manifest schema accepts.
pub fn rfc3339_utc(at: SystemTime) -> String {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default() as i64;
    let (y, m, d) = crate::transaction_store::civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// The document `GET /.well-known/uvd-stack.json` serves, built on the first
/// request and kept for the life of the process.
///
/// By the first request `main` has mounted every limiter -- the router is
/// assembled before the listener accepts anything -- so the snapshot is
/// complete. `generated_at` is that moment.
pub fn served_document() -> &'static str {
    static DOCUMENT: OnceLock<String> = OnceLock::new();
    DOCUMENT.get_or_init(|| {
        let doc = manifest(
            &crate::rate_policy::mounted(),
            crate::version::facilitator_version(),
            crate::version::facilitator_git_sha(),
            &rfc3339_utc(SystemTime::now()),
        );
        serde_json::to_string_pretty(&doc).expect("the manifest is plain JSON")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate_policy::{BUDGETS, VERIFY_SETTLE};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn vendored() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/interop")
    }

    fn read_json(path: &Path) -> Value {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
    }

    fn schema() -> Value {
        read_json(&vendored().join("schemas/manifest.schema.json"))
    }

    fn validator() -> jsonschema::Validator {
        jsonschema::validator_for(&schema()).expect("the vendored schema compiles")
    }

    /// Each error reduced the way the spec's README compares them across
    /// languages: keyword, instance pointer and, for `required`, the missing
    /// property; the keywords that only wrap another's verdict dropped.
    ///
    /// `propertyNames` is one of those wrappers, and this crate nests the
    /// verdict it wraps inside its own error instead of reporting it next to
    /// it, as Python's `jsonschema` does. So a `propertyNames` error is read as
    /// the keyword it wraps, at the object's pointer -- which is exactly what
    /// the reference implementation reports (`erc8004-red-sin-caip2.json`).
    fn portable_errors(validator: &jsonschema::Validator, doc: &Value) -> BTreeSet<String> {
        const WRAPPERS: [&str; 8] = [
            "if",
            "then",
            "else",
            "allOf",
            "anyOf",
            "oneOf",
            "$ref",
            "propertyNames",
        ];
        use jsonschema::error::ValidationErrorKind;
        fn wrapped(kind: &ValidationErrorKind) -> &ValidationErrorKind {
            match kind {
                ValidationErrorKind::PropertyNames { error } => wrapped(error.kind()),
                other => other,
            }
        }
        validator
            .iter_errors(doc)
            .filter_map(|e| {
                let kind = wrapped(e.kind());
                if WRAPPERS.contains(&kind.keyword()) {
                    return None;
                }
                let property = match kind {
                    ValidationErrorKind::Required { property } => {
                        property.as_str().unwrap_or_default().to_string()
                    }
                    _ => String::new(),
                };
                Some(format!(
                    "{}|{}|{}",
                    kind.keyword(),
                    e.instance_path(),
                    property
                ))
            })
            .collect()
    }

    /// The buckets `main.rs` and `handlers` mount, at their defaults, on their
    /// doors: every budget on the API, and verify/settle's on MCP too.
    fn production_like_mounts() -> Vec<Mount> {
        let mut mounts: Vec<Mount> = BUDGETS
            .iter()
            .map(|budget| Mount {
                limit: budget.default_limit(),
                door: Door::Api,
            })
            .collect();
        mounts.push(Mount {
            limit: VERIFY_SETTLE.default_limit(),
            door: Door::Mcp,
        });
        mounts
    }

    fn built() -> Value {
        manifest(
            &production_like_mounts(),
            "2.40.0",
            "8ee44114da322363095822919e9e51ffbe1f05d5",
            "2026-09-24T12:00:00Z",
        )
    }

    /// The document validates against the schema this repo vendored, with
    /// every error listed if it does not.
    #[test]
    fn the_manifest_validates_against_the_vendored_schema() {
        let errors = portable_errors(&validator(), &built());
        assert!(errors.is_empty(), "the manifest is invalid: {errors:#?}");
    }

    /// `doc` with the value at `pointer` replaced, added, or (`None`) removed.
    fn patch(doc: &mut Value, pointer: &str, value: Option<Value>) {
        let (parent, key) = pointer.rsplit_once('/').expect("a JSON pointer");
        match (doc.pointer_mut(parent), value) {
            (Some(Value::Object(map)), Some(v)) => {
                map.insert(key.to_string(), v);
            }
            (Some(Value::Object(map)), None) => {
                map.remove(key);
            }
            (Some(Value::Array(items)), Some(v)) => items[key.parse::<usize>().unwrap()] = v,
            _ => panic!("cannot patch {pointer}"),
        }
    }

    /// And the check can go red: each of these breaks one rule the schema
    /// enforces, and each is refused. A validator that accepted everything
    /// would pass the test above; it cannot pass this one.
    #[test]
    fn a_broken_manifest_is_refused() {
        let validator = validator();
        let cases: Vec<(&str, &str, Option<Value>)> = vec![
            ("no git_sha", "/git_sha", None),
            ("an upper-case git_sha", "/git_sha", Some(json!("8EE4411"))),
            (
                "a git_sha with a newline",
                "/git_sha",
                Some(json!("8ee4411\n")),
            ),
            (
                "plain http",
                "/endpoints/api/url",
                Some(json!("http://facilitator.ultravioletadao.xyz")),
            ),
            (
                "an internal route",
                "/health/ready",
                Some(json!(
                    "https://facilitator.ultravioletadao.xyz/internal/ready"
                )),
            ),
            (
                "an auth mode outside the vocabulary",
                "/endpoints/mcp/auth",
                Some(json!(["bearer"])),
            ),
            (
                "charges without an x402 document",
                "/payments",
                Some(json!({ "charges": true })),
            ),
            ("a zero window", "/rate_limits/0/window_s", Some(json!(0))),
            (
                "a limit on an unknown door",
                "/rate_limits/0/applies_to",
                Some(json!("admin")),
            ),
            (
                "an unknown key",
                "/rate_limit_policy",
                Some(json!("verify-settle")),
            ),
            ("no service_signer at all", "/identity", Some(json!({}))),
            (
                "an app id with a capital",
                "/app",
                Some(json!("Facilitator")),
            ),
            (
                "an offset instead of Z",
                "/generated_at",
                Some(json!("2026-09-24T12:00:00+00:00")),
            ),
        ];
        for (what, pointer, value) in cases {
            let mut doc = built();
            patch(&mut doc, pointer, value);
            assert!(
                !validator.is_valid(&doc),
                "a manifest with {what} still validates"
            );
        }
    }

    /// The Rust validator reads the schema the way the reference suite does:
    /// every valid fixture passes, and every invalid one fails with EXACTLY
    /// the errors `cases.json` lists (keyword, pointer, missing property), no
    /// more and no fewer -- the portable comparison of the spec's README. A
    /// regex dialect or a keyword this crate read differently from Python's
    /// `jsonschema` and Ajv would show up here, not in production.
    #[test]
    fn the_validator_agrees_with_every_vendored_fixture() {
        let validator = validator();
        let root = vendored().join("fixtures");
        let cases = read_json(&root.join("cases.json"));
        let mut ran = (0, 0);
        for case in cases["cases"].as_array().unwrap() {
            if case["schema"] != "manifest" {
                continue;
            }
            let file = case["file"].as_str().unwrap();
            let doc = read_json(&root.join(file));
            let got = portable_errors(&validator, &doc);
            if case["valid"] == true {
                assert!(got.is_empty(), "{file} must validate: {got:#?}");
                ran.0 += 1;
            } else {
                let want: BTreeSet<String> = case["errors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| {
                        format!(
                            "{}|{}|{}",
                            e["keyword"].as_str().unwrap(),
                            e["instance_path"].as_str().unwrap(),
                            e["property"].as_str().unwrap_or_default()
                        )
                    })
                    .collect();
                assert_eq!(got, want, "{file}: errors differ from cases.json");
                ran.1 += 1;
            }
        }
        // 4 valid and 94 invalid at the vendored commit. A copy that lost
        // files would otherwise pass by running nothing.
        assert_eq!(ran, (4, 94), "(valid, invalid) manifest fixtures run");
    }

    /// The vendored copy is byte for byte what `ORIGEN.json` says was copied
    /// from the SDK's `main`. Edited by hand, it is no longer the contract the
    /// other implementations test against; re-copy from a new commit instead.
    #[test]
    fn the_vendored_interop_files_match_origen() {
        let origen = read_json(&vendored().join("ORIGEN.json"));
        assert_eq!(origen["repo"], "UltravioletaDAO/uvd-x402-sdk-python");
        assert_eq!(origen["path"], "interop");
        let commit = origen["commit"].as_str().unwrap();
        assert!(
            commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "ORIGEN.json must pin a full commit, got {commit:?}"
        );
        let files = origen["files"].as_object().unwrap();
        let mut on_disk = BTreeSet::new();
        let mut stack = vec![vendored()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let rel = path.strip_prefix(vendored()).unwrap();
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if rel != "ORIGEN.json" {
                        on_disk.insert(rel);
                    }
                }
            }
        }
        let listed: BTreeSet<String> = files.keys().cloned().collect();
        assert_eq!(on_disk, listed, "files on disk and in ORIGEN.json differ");
        for (rel, want) in files {
            let bytes = std::fs::read(vendored().join(rel)).unwrap();
            let got = format!("{:x}", Sha256::digest(&bytes));
            assert_eq!(
                &got,
                want.as_str().unwrap(),
                "{rel} was edited after vendoring"
            );
        }
    }

    /// The rules the schema cannot express (the runner's R1.10 and R5.5), on
    /// this manifest: an emitted type starts with the app id, and the
    /// generation date exists.
    #[test]
    fn the_rules_beyond_the_schema_hold() {
        let doc = built();
        for emitted in doc["events"]["emits"].as_array().unwrap() {
            assert!(emitted.as_str().unwrap().starts_with(&format!("{APP_ID}.")));
        }
        assert!(doc["endpoints"]["api"].get("erc8128").is_none());

        // generated_at comes from a real clock, formatted by hand: check the
        // formatter against dates whose answer is known.
        let at = |secs: u64| rfc3339_utc(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_790_294_399), "2026-09-24T23:59:59Z");
        let now = rfc3339_utc(SystemTime::now());
        let mut doc = built();
        doc["generated_at"] = json!(now);
        assert!(
            validator().is_valid(&doc),
            "{now} is not a manifest timestamp"
        );
    }

    /// Every limit mounted is published on the door it guards, and a bucket
    /// shared by two doors is published on both.
    #[test]
    fn every_mounted_limit_is_published_on_its_door() {
        let doc = built();
        let limits = doc["rate_limits"].as_array().unwrap();
        let has = |door: &str, limit: u64, window: u64| {
            limits
                .iter()
                .any(|l| l["applies_to"] == door && l["limit"] == limit && l["window_s"] == window)
        };
        assert!(has("api", 30, 60), "verify/settle");
        assert!(has("mcp", 30, 60), "/mcp shares verify/settle's bucket");
        assert!(has("api", 250, 3000), "discovery register");
        assert!(has("api", 120, 24), "bazaar reads");
        assert!(has("api", 10, 20), "/events");
        assert!(has("api", 30, 360), "ERC-8004 writes");
        // identity-read and human-pages are both 60 per 30s by default: one
        // entry, because the manifest's entry has no name to tell them apart.
        let sixty = limits
            .iter()
            .filter(|l| l["limit"] == 60 && l["window_s"] == 30)
            .count();
        assert_eq!(sixty, 1);
        assert!(limits.iter().all(|l| l["scope"] == "ip"));
    }

    /// Every URL points at this service, and is one of the paths the router
    /// test walks (`published_paths`).
    #[test]
    fn every_url_is_on_the_public_origin_and_listed_for_the_router_test() {
        let doc = built();
        let mut urls = vec![
            doc["endpoints"]["api"]["url"].as_str().unwrap().to_string(),
            doc["endpoints"]["mcp"]["url"].as_str().unwrap().to_string(),
            doc["health"]["live"].as_str().unwrap().to_string(),
            doc["health"]["ready"].as_str().unwrap().to_string(),
        ];
        urls.extend(
            doc["links"]
                .as_object()
                .unwrap()
                .values()
                .map(|v| v.as_str().unwrap().to_string()),
        );
        let paths = published_paths();
        for u in urls {
            let path = u.strip_prefix(PUBLIC_URL).expect("on the public origin");
            assert!(
                path.is_empty() || paths.contains(&path),
                "{u} is not walked"
            );
        }
    }

    /// The served document is the manifest, and it validates -- whatever
    /// limiters this test process happened to mount before it was built.
    #[test]
    fn the_served_document_validates() {
        let doc: Value = serde_json::from_str(served_document()).unwrap();
        let errors = portable_errors(&validator(), &doc);
        assert!(
            errors.is_empty(),
            "the served manifest is invalid: {errors:#?}"
        );
        assert_eq!(doc["version"], crate::version::facilitator_version());
        assert_eq!(doc["git_sha"], crate::version::facilitator_git_sha());
    }

    /// The SERVED manifest publishes what the router mounted: at least one
    /// limit, and every entry is the (door, q, w) of a bucket in
    /// [`crate::rate_policy::mounted`]. The schema accepts `rate_limits: []`,
    /// so validating the served document alone would pass a manifest that
    /// dropped every limit.
    #[test]
    fn the_served_manifest_publishes_the_mounted_buckets() {
        use crate::rate_policy::{config, RatePolicy};
        // At least one bucket is mounted in this process before the manifest
        // is first built, as `main` mounts every one before it serves.
        let _ = axum::Router::<()>::new()
            .layer(RatePolicy::none().layer(&config(VERIFY_SETTLE.limit())));
        let doc: Value = serde_json::from_str(served_document()).unwrap();
        let published = doc["rate_limits"].as_array().unwrap();
        assert!(
            !published.is_empty(),
            "the served manifest publishes no limit"
        );
        let mounted: Vec<(String, u64, u64)> = crate::rate_policy::mounted()
            .iter()
            .map(|m| {
                (
                    m.door.as_str().to_string(),
                    m.limit.quota(),
                    m.limit.window_s(),
                )
            })
            .collect();
        for entry in published {
            let key = (
                entry["applies_to"].as_str().unwrap().to_string(),
                entry["limit"].as_u64().unwrap(),
                entry["window_s"].as_u64().unwrap(),
            );
            assert!(
                mounted.contains(&key),
                "the served manifest publishes {key:?}, which no mounted bucket is"
            );
        }
    }
}
