//! El gate del anchor contra un release de escrow REEJECUTADO en una fork.
//!
//! Por que existe: el escalon 2 (`verified`) exige un pago de menos de 900
//! segundos, y el riel de escrow puede pasar dias sin uno. Sin esto, la unica
//! forma de comprobar que el gate llega hasta el final es esperar a que alguien
//! compre algo -- y "no lo pude probar porque no habia trafico" no es una
//! verificacion.
//!
//! No es un mock. Se forkea la cadena de verdad un bloque antes de un release
//! real de Execution Market, se **reejecuta la misma transaccion** para que caiga
//! en un bloque con timestamp de ahora, y se corre `verify_anchor` contra eso.
//! El codigo del escrow y del operador son los desplegados; el `getHash` lo
//! calcula el contrato; la firma la produce el custodio real del payee. Lo unico
//! que la simulacion mueve es el reloj.
//!
//! Se salta solo cuando `DX402_SIM` no esta, asi que en CI no pide red ni anvil.
//! Para regenerarlo, ver `docs/plans/dx402/07-SIMULACION-ESCROW.md`.

use std::env;

use alloy::primitives::B256;
use alloy::providers::ProviderBuilder;
use x402_rs::dx402::gate::{verify_anchor, AnchorClaim, EscrowRelease};
use x402_rs::erc8004::ProofOfPayment;
use x402_rs::network::Network;
use x402_rs::types::MixedAddress;

#[tokio::test]
async fn a_replayed_escrow_release_reaches_the_final_rung() {
    let Ok(path) = env::var("DX402_SIM") else {
        eprintln!("DX402_SIM sin definir -- se salta (necesita una fork con anvil)");
        return;
    };
    let raw = std::fs::read_to_string(&path).expect("no se pudo leer DX402_SIM");
    let sim: serde_json::Value = serde_json::from_str(&raw).expect("DX402_SIM no es JSON");

    let proof: ProofOfPayment =
        serde_json::from_value(sim["proof"].clone()).expect("proofOfPayment");
    let release: EscrowRelease =
        serde_json::from_value(sim["escrowRelease"].clone()).expect("escrowRelease");
    // `MixedAddress` no implementa FromStr: se parsea la direccion EVM y se
    // convierte, igual que los tests del gate.
    let sealed_to: MixedAddress = sim["sealedTo"]
        .as_str()
        .unwrap()
        .parse::<alloy::primitives::Address>()
        .unwrap()
        .into();
    let payment_id: B256 = sim["pid"].as_str().unwrap().parse().unwrap();
    let content_hash: B256 = sim["contentHash"].as_str().unwrap().parse().unwrap();
    let pointer = sim["pointer"].as_str().unwrap().to_string();
    let signature = sim["sig"].as_str().unwrap().to_string();
    let rpc = sim["rpc"].as_str().unwrap();

    let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());

    let claim = AnchorClaim {
        network: Network::Monad,
        proof: Some(&proof),
        sealed_to: &sealed_to,
        payment_id,
        content_hash,
        pointer: &pointer,
        seller_signature: Some(&signature),
        escrow_release: Some(&release),
        chain_id: 143,
    };

    // Todo el gate, sin atajos: la prueba se lee de la cadena, el comprador se
    // resuelve por el escrow, y la firma tiene que ser del payee que la cadena
    // reporta.
    verify_anchor(&provider, &claim)
        .await
        .expect("un release de escrow reejecutado y firmado por su payee debe verificar");

    // Y el contraejemplo, en la misma corrida: sin la autorizacion del escrow el
    // mismo anclaje honesto no puede verificar, porque el `from` del Transfer es
    // el TokenStore. Es el bug que este cambio arreglo, reproducido a demanda.
    let sin_release = AnchorClaim {
        escrow_release: None,
        ..claim
    };
    let err = verify_anchor(&provider, &sin_release)
        .await
        .expect_err("sin escrowRelease no se puede saber quien financio el escrow");
    assert_eq!(err.as_str(), "dx402_escrow_release_missing");
}
