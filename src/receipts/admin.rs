//! `x402-rs receipts release-abandoned`: the operator path for admissions left
//! `unknown` with nothing prepared. In earlier releases a settlement that
//! ended between its reservation and its send (writer lease handover, a fill or
//! signing failure, a prepared save that failed) stranded its authorization:
//! no bytes to reconcile, no TTL, and every resend refused as in flight.
//!
//! Read-only unless `--write`. A record is closed exactly as the settle path
//! now closes one (`abandon`), with a CAS on the revision it was read at: a
//! settlement still running under that revision can no longer store prepared
//! bytes, so it never sends. The same request is then admitted again, and an
//! expired authorization simply fails verification.
use super::*;

const USAGE: &str = "usage: x402-rs receipts release-abandoned [--dry-run | --write] \
[--receipt-id <uuid>]... [--min-age-secs <seconds>]";

/// Far past the longest path from a reservation to its send, which is a
/// handful of RPC calls, each with its own timeout.
const DEFAULT_MIN_AGE_SECS: u64 = 900;

#[derive(Debug, PartialEq)]
pub struct Options {
    pub write: bool,
    /// Empty: every receipt record, by a table scan.
    pub receipt_ids: Vec<String>,
    /// An unexpired authorization is only released when its admission is at
    /// least this old.
    pub min_age_secs: u64,
}

pub fn parse(args: &[String]) -> std::result::Result<Options, String> {
    let mut args = args.iter();
    if args.next().map(String::as_str) != Some("release-abandoned") {
        return Err(USAGE.into());
    }
    let mut options = Options {
        write: false,
        receipt_ids: Vec::new(),
        min_age_secs: DEFAULT_MIN_AGE_SECS,
    };
    let mut dry_run = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--write" => options.write = true,
            "--receipt-id" => {
                let id = args.next().ok_or(USAGE)?;
                uuid::Uuid::parse_str(id).map_err(|_| format!("not a receipt id: {id}"))?;
                options.receipt_ids.push(id.clone());
            }
            "--min-age-secs" => {
                options.min_age_secs = args.next().and_then(|n| n.parse().ok()).ok_or(USAGE)?;
            }
            _ => return Err(USAGE.into()),
        }
    }
    if dry_run && options.write {
        return Err("--dry-run and --write exclude each other".into());
    }
    Ok(options)
}

/// What the command did, or would do, with one record.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub receipt_id: String,
    pub network: Option<String>,
    pub status: Option<String>,
    pub age_secs: Option<u64>,
    /// `None` where the record carries no expiry (native Hedera).
    pub authorization_expired: Option<bool>,
    pub action: &'static str,
}

/// Only an admission that provably sent nothing and that no live settlement
/// can still be holding.
fn eligible(record: &Record, options: &Options, now: u64) -> std::result::Result<(), &'static str> {
    if record.receipt.operation != "settle" {
        return Err("skip_not_settlement");
    }
    if record.receipt.status != "unknown" {
        return Err("skip_not_unknown");
    }
    if record.prepared.is_some() || record.receipt.settlement.is_some() {
        return Err("skip_transaction_prepared");
    }
    let expired = record.authorization_expires_at > 0 && now >= record.authorization_expires_at;
    if !expired && now.saturating_sub(record.receipt.issued_at) < options.min_age_secs {
        return Err("skip_too_recent");
    }
    Ok(())
}

/// The key that signed a record, from its JWS protected header.
fn signing_kid(record: &Record) -> Option<String> {
    let jws = record.receipt.proof.as_ref()?.get("jws")?.as_str()?;
    let protected = URL_SAFE_NO_PAD.decode(jws.split('.').next()?).ok()?;
    let header: Value = serde_json::from_slice(&protected).ok()?;
    header.get("kid")?.as_str().map(str::to_owned)
}

async fn release(service: &Service, record: &Record) -> &'static str {
    if let Some(kid) = signing_kid(record) {
        let ours = service
            .signing_key
            .as_ref()
            .map(|key| hash(key.verifying_key().as_bytes()));
        if ours.as_deref() != Some(kid.as_str()) {
            // Re-signing with another key, or none, breaks every client that
            // verifies the receipt against the issuer's published keys.
            return "skip_signing_key_mismatch";
        }
    }
    let diagnostic = match record.receipt.diagnostic_code.as_deref() {
        Some(code) if code != "operation_in_progress" => code.to_owned(),
        _ => "released_by_operator".to_owned(),
    };
    let mut closed = record.clone();
    abandon(
        &mut closed,
        &diagnostic,
        RETRY_AFTER_SECS,
        json!({ "error": diagnostic }),
    );
    match service.save(&mut closed).await {
        Ok(()) => "released",
        Err(error) if error == "receipt_revision_conflict" => "skip_revision_moved",
        Err(_) => "failed",
    }
}

pub async fn release_abandoned(
    service: &Service,
    options: &Options,
    now: u64,
) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let records = if options.receipt_ids.is_empty() {
        service.store.records().await?
    } else {
        let mut records = Vec::new();
        for id in &options.receipt_ids {
            match service.store.get(&format!("receipt:v1:{id}")).await? {
                Some(record) => records.push(record),
                None => entries.push(Entry {
                    receipt_id: id.clone(),
                    network: None,
                    status: None,
                    age_secs: None,
                    authorization_expired: None,
                    action: "skip_not_found",
                }),
            }
        }
        records
    };
    for record in records {
        let action = match eligible(&record, options, now) {
            Err(skip) => skip,
            Ok(()) if !options.write => "would_release",
            Ok(()) => release(service, &record).await,
        };
        entries.push(Entry {
            receipt_id: record.receipt.receipt_id.clone(),
            network: Some(record.receipt.network.clone()),
            status: Some(record.receipt.status.clone()),
            age_secs: Some(now.saturating_sub(record.receipt.issued_at)),
            authorization_expired: (record.authorization_expires_at > 0)
                .then_some(now >= record.authorization_expires_at),
            action,
        });
    }
    Ok(entries)
}

/// Runs the command and returns the process exit code. One JSON line per
/// record, then one summary line; nothing private (no payer, no capability).
pub async fn run(args: &[String]) -> i32 {
    let options = match parse(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let Some(store) = store::DynamoStore::from_env().await else {
        eprintln!("IDEMPOTENCY_TABLE_NAME is not set: no receipt store to read");
        return 2;
    };
    let signing_key = match signing_key_from_env() {
        Ok(key) => key,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let service = Service { store, signing_key };
    let entries = match release_abandoned(&service, &options, now()).await {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("receipts release-abandoned: {error}");
            return 1;
        }
    };
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for entry in &entries {
        println!("{}", serde_json::to_string(entry).unwrap_or_default());
        *counts.entry(entry.action).or_default() += 1;
    }
    println!(
        "{}",
        json!({"summary": counts, "write": options.write, "minAgeSecs": options.min_age_secs})
    );
    i32::from(counts.contains_key("failed"))
}
