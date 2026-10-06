# X402-LAMBDA-ESTUDIO — estudio Lambda (solo docs)

## Estado

- Hecho: medición en código y `docs/estudios/lambda.md` (veredicto: viable con cambios; híbrido
  lecturas-en-Lambda / escrituras-en-Fargate primero).
- Falta: nada del encargo; la implementación queda para PRs aparte (§8 paso 0 del estudio).
- Próximo paso: que c0der decida si sigue con el híbrido A o descarta Lambda por el ahorro.

## Rojos de base

- Ninguno con el comando del CI: `cargo test --locked -p x402-rs --features
  solana,near,stellar,algorand,sui,xrpl,hedera -- --test-threads=1` sale 0 en `c3b694b0`
  (lib: 1711 passed y 1778 passed en los dos binarios con tests de unidad).
- Con el comando que daba `CLAUDE.md` (sin `hedera`) fallan 6 en `main` limpio:
  `facilitator_local::testnet_chain_id_supported_diff_tests::moving_the_two_testnets_changes_no_other_entry_of_supported`,
  `networks_json::tests::{a_served_network_without_metadata_keeps_its_row,bsc_sui_and_hedera_read_as_served,the_rows_are_the_networks_supported_names}`,
  `receipts::tests::{capability_lists_exactly_the_supported_networks,shared_python_typescript_vectors_validate_in_rust}`.
  Este PR corrige el comando de `CLAUDE.md` (regla 12: la lección va al reglamento del repo).
- Entorno: el host necesitó `pkg-config` y `protobuf-compiler` (`protoc`, lo pide `hiero-sdk-proto`).

## Refutaciones al encargo

- "Clientes RPC de 85 redes": el enum tiene 43 variantes con todas las features
  (`src/network.rs:398`) y solo se construyen las que tienen RPC configurado.
- "Writer lease en el SG": el lease vive en DynamoDB (`src/writer_lease.rs:159-198`); el SG solo
  abre 8080 entre tareas para el reenvío (`terraform/environments/production/main.tf:210-218`).

## Notas para quien retome

- En Lambda el writer lease se abstiene por falta de metadata de ECS y el proceso queda como
  escritor standalone (`src/writer_lease.rs:228-234`, `:699-716`): no desplegar settles EVM en
  Lambda sin el asignador en DynamoDB o el pool con lease por EOA.
