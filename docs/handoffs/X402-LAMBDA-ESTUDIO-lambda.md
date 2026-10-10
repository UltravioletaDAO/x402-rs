# X402-LAMBDA-ESTUDIO / X402-LAMBDA-PLAN — estudio Lambda (solo docs)

## Estado (ronda 1 de REF-X402-116, 2026-10-10): qué está hecho, qué falta, próximo paso

- Hecho, en `docs/estudios/lambda.md`, los 7 puntos de la ronda:
  - 0.6 deja el grant EVM en no tenido (fail-closed, nunca standalone), sin llaves de firma en la
    función de lecturas (`lambda-reads`) y sin esos secretos en su rol IAM. La 1.3 depende de eso.
  - 1.2 fija en Fargate las escrituras del Bazaar, `/register/status/*` y las rutas que usan llaves
    de atestación, `/verify` incluida, porque firma recibos (pregunta abierta a c0der). El gate del bug «alta del bazar 201 se pierde entre réplicas» queda en la 1.3 y
    en §5, y la 2.5 nueva saca esas rutas de Fargate antes de la 3.2.
  - 2.1 baja el recibo a 540 s, por debajo de `alb_idle_timeout = 600`.
  - 0.5 falla cerrado también cuando el init de DynamoDB falla.
  - El calendario suma las ventanas del plan: 107 días, con un piso de ~18 semanas.
  - §12 manda el código en tandas y los pesos en applies programados.
  - Quedaron también la línea de estado bajo el título, #121 en §10.4 y en la 1.1, y la medición de
    c0der del 6-oct en §10.5.
- Falta: la consulta de Athena de la tarea 0.8 y la duración de los loops (§10.3). Este PR no toca
  AWS.
- Próximo paso: el mismo de antes, que el dueño decida si arranca la fase 0; y c0der corrige la
  cifra de calendario de la decisión 170.

## Estado (X402-LAMBDA-PLAN, 2026-10-06): qué está hecho, qué falta, próximo paso

- Hecho: `docs/estudios/lambda.md` suma el resumen de 5 líneas para el dueño (arriba), §10 coste
  (hoy 137,13 / fase 1 50,64 / Lambda 7,41-37,80 USD/mes en las filas que cambian; x10 incluido),
  §11 rendimiento por paso y pico, §12 plan por fases 0-3 con archivo, criterio, prueba, rollback y talla.
- Falta: nada del encargo. Las cifras pesimista/central se cierran con la consulta de solo lectura
  de §10.5 (`RequestCount` × `TargetResponseTime` por target group) y la de Athena de la tarea 0.8,
  que este PR no corre (no toca AWS).
- Próximo paso: que el dueño decida si arranca la fase 0 (recomendado ya, mejora el Fargate de hoy).

## Refutaciones al encargo X402-LAMBDA-PLAN

- "Function URL o API Gateway HTTP (1 USD/M)": Function URL no tiene cargo propio y API Gateway
  corta a 30 s (§3), inviable para `/settle`. El plan usa un target group `lambda` del ALB
  existente (0 USD de integración, +0,53 USD/mes de LCU).
- "Hoy: 2 tareas": es el piso (`production.auto.tfvars:73`); la auditoría midió 2,862 de promedio
  y el pico de 5.913 req/h pide 7 tareas al autoscaling (techo 3). El coste usa 2 para no inflar
  el ahorro; §10.5 da la sensibilidad.
- "EventBridge de los 12 loops": de las 12 filas que dejan de ser `tokio::spawn`, 9 necesitan
  schedule y 3 pasan a perezosas en el request (§10.3).
- "NAT ~31": por fórmula son 32,85 de horas + 8,75 de datos (agosto) + 3,65 de la EIP.
- "Fargate ~36 tras la fase 1": correcto, pero B5 de #115 da una IPv4 pública por tarea
  (+7,30 USD/mes), que la fase 1 suma y Lambda no.
- "p50/p90/p99 medidos por paso": no existen por ruta; solo el ALB entero y el p99 por target
  group. §11 lo dice y la tarea 0.8 los mide.
- "Plan ejecutable con ahorro": con 10x tráfico y el caso pesimista de espera de settle, Lambda
  cuesta más que la fase 1 (221 contra 72 USD/mes).

## Estado (X402-LAMBDA-ESTUDIO, ronda anterior)

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
