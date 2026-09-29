//! `FacilitatorClient::settle` reads what a facilitator built from this
//! workspace answers: the settlement hash under `transaction`,
//! `transactionHash` and `transaction_hash` at once, plus `paymentId`.
//!
//! The body is produced by `SettleResponse`'s own `Serialize`, not typed by
//! hand, so the test follows whatever the facilitator writes.

use axum::{routing::post, Json, Router};
use x402_axum::facilitator_client::FacilitatorClient;
use x402_rs::network::Network;
use x402_rs::types::{MixedAddress, SettleRequest, SettleResponse, TransactionHash};

fn settled() -> SettleResponse {
    SettleResponse {
        success: true,
        error_reason: None,
        payer: MixedAddress::Evm(
            "0x1111111111111111111111111111111111111111"
                .parse()
                .unwrap(),
        ),
        transaction: Some(TransactionHash::Evm([0x5e; 32])),
        network: Network::Base,
        proof_of_payment: None,
        extensions: None,
    }
}

/// A v1 `exact` settle request. What it says does not matter to the stub.
fn request() -> SettleRequest {
    serde_json::from_value(serde_json::json!({
        "x402Version": 1,
        "paymentPayload": {
            "x402Version": 1,
            "scheme": "exact",
            "network": "base",
            "payload": {
                "signature": format!("0x{}", "11".repeat(65)),
                "authorization": {
                    "from": "0x1111111111111111111111111111111111111111",
                    "to": "0x2222222222222222222222222222222222222222",
                    "value": "1000",
                    "validAfter": "0",
                    "validBefore": "2000000000",
                    "nonce": format!("0x{}", "42".repeat(32)),
                },
            },
        },
        "paymentRequirements": {
            "scheme": "exact",
            "network": "base",
            "maxAmountRequired": "1000",
            "resource": "https://merchant.example/data",
            "description": "",
            "mimeType": "application/json",
            "payTo": "0x2222222222222222222222222222222222222222",
            "maxTimeoutSeconds": 60,
            "asset": "0x3333333333333333333333333333333333333333",
        },
    }))
    .expect("a v1 settle request")
}

#[tokio::test]
async fn settle_reads_the_response_the_facilitator_writes() {
    let answer = serde_json::to_value(settled()).unwrap();
    for key in [
        "transaction",
        "transactionHash",
        "transaction_hash",
        "paymentId",
    ] {
        assert!(
            answer.get(key).is_some(),
            "the stub must answer `{key}`: {answer}"
        );
    }
    let app = Router::new().route(
        "/settle",
        post(move || {
            let answer = answer.clone();
            async move { Json(answer) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let client = FacilitatorClient::try_from(base.as_str()).unwrap();
    let response = client
        .settle(&request())
        .await
        .expect("the client must read the facilitator's own settle response");
    assert!(response.success);
    assert_eq!(response.transaction, Some(TransactionHash::Evm([0x5e; 32])));
    assert_eq!(response.network, Network::Base);
}
