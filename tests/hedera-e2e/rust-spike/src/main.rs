//! Prints the phase-0 report over every vector in ../vectors.
//!
//!     cargo run --offline --bin hedera-spike
//!
//! `--json` emits the machine-readable form that the handoff quotes.
use anyhow::Result;
use hedera_spike::{load_vector, run, vector_paths, vectors_dir, Result3};

fn main() -> Result<()> {
    let json = std::env::args().any(|a| a == "--json");
    let dir = vectors_dir();
    let mut reports = Vec::new();

    for path in vector_paths(&dir)? {
        let vector = load_vector(&path)?;
        let report = run(&vector)?;
        if !json {
            println!("=== {} ({} variants)", vector.name, report.raw_variant_count);
            println!("    {}", vector.description);
            println!(
                "    cross-SDK bodies match ......... {}",
                if report.cross_sdk_bodies_match { "PASS" } else { "FAIL" }
            );
            println!("    facilitator policy ............. {}{}",
                report.facilitator_policy.mark(),
                report.facilitator_policy_error.as_ref().map(|e| format!("  ({e})")).unwrap_or_default());
            println!("    AnyTransaction::from_bytes ..... {}{}", report.sdk_decode.mark(),
                report.sdk_decode_error.as_ref().map(|e| format!("  ({e})")).unwrap_or_default());
            println!("    downcast to TransferTransaction  {}", report.downcast_transfer.mark());
            if let Some(n) = report.sdk_variant_count {
                println!("    node ids the SDK reports ....... {n}");
            }
            println!("    payer signature over all bodies  {}{}", report.payer_signature.mark(),
                report.payer_signature_error.as_ref().map(|e| format!("  ({e})")).unwrap_or_default());
            println!("    to_bytes round trip identical .. {}", report.roundtrip_identical.mark());
            println!("    bodies intact after co-sign .... {}", report.bodies_preserved_after_cosign.mark());
            println!("    prior signatures intact ........ {}", report.prior_signatures_preserved.mark());
            println!("    co-signature on every variant .. {}", report.cosignature_on_every_variant.mark());
            println!("    co-signature verifies .......... {}", report.cosignature_verifies.mark());
            if !report.sdk_getters_hide.is_empty() {
                println!("    protobuf fields the SDK getters cannot express:");
                for f in &report.sdk_getters_hide {
                    println!("      - {f}");
                }
            }
            for n in &report.notes {
                println!("    note: {n}");
            }
            if let Some(a) = &vector.adversarial {
                println!("    adversarial: {} -- {}", a.kind, a.must_be_rejected_because);
            }
            println!();
        }
        reports.push(report);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        let preserved = reports
            .iter()
            .filter(|r| r.bodies_preserved_after_cosign == Result3::Pass)
            .count();
        let cosigned = reports
            .iter()
            .filter(|r| r.cosignature_verifies == Result3::Pass)
            .count();
        let rejected = reports.iter().filter(|r| r.sdk_decode == Result3::Fail).count();
        println!(
            "{} vectors: {} co-signed with bodies intact, {} verified co-signatures, {} refused at decode",
            reports.len(),
            preserved,
            cosigned,
            rejected
        );
    }
    Ok(())
}
