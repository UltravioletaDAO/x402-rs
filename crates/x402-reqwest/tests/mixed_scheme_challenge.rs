//! A seller may advertise more schemes than this buyer can pay.
use x402_rs::types::PaymentRequiredResponse;

/// One `exact` offer this buyer can pay, and one `batch-settlement` it cannot.
const MIXED: &str = r#"{
  "x402Version": 1,
  "error": "Payment required",
  "accepts": [
    {
      "scheme": "batch-settlement",
      "network": "base",
      "maxAmountRequired": "30000",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "payTo": "0xe4dc963c56979E0260fc146b87eE24F18220e545",
      "resource": "https://api.example.com/thing",
      "description": "batched",
      "mimeType": "application/json",
      "maxTimeoutSeconds": 300
    },
    {
      "scheme": "exact",
      "network": "base",
      "maxAmountRequired": "10000",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "payTo": "0xe4dc963c56979E0260fc146b87eE24F18220e545",
      "resource": "https://api.example.com/thing",
      "description": "plain",
      "mimeType": "application/json",
      "maxTimeoutSeconds": 300
    }
  ]
}"#;

#[test]
fn a_scheme_we_cannot_pay_does_not_cost_us_the_one_we_can() {
    // The whole challenge used to fail to deserialize because ONE entry named a
    // scheme this build does not implement -- so a buyer lost the `exact` offer
    // sitting right beside it and could not pay a seller that was perfectly
    // payable. The same failure the catalog had in P0, one layer along.
    let parsed: PaymentRequiredResponse =
        serde_json::from_str(MIXED).expect("a challenge must survive an offer we cannot read");

    assert_eq!(parsed.accepts.len(), 1, "the readable offer survives");
    assert_eq!(parsed.accepts[0].max_amount_required.to_string(), "10000");
    assert_eq!(
        parsed.unreadable_offers.len(),
        1,
        "and the one we could not read is counted, not silently dropped"
    );
    assert_eq!(
        parsed.unreadable_offers[0].scheme.as_deref(),
        Some("batch-settlement")
    );
}
