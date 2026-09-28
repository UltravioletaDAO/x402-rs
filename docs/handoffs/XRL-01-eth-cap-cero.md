# XRL-01 — Facilitador: 0 escrituras ERC-8004 por día en Ethereum mainnet

## Estado

- **Hecho:** `terraform/environments/production/main.tf`, entrada `ERC8004_DAILY_WRITE_CAP_ETHEREUM` del task def del
  facilitador: `value = "300"` → `value = "0"`, con el comentario reescrito (pausa por decisión del dueño del
  2026-09-28; los ratings pendientes de KarmaCadabra en Ethereum quedan pendientes; se levanta sólo con otra decisión
  del dueño, y borrar la entrada vuelve al built-in de 100). Nada más en el archivo.
- **Falta:** revisión de c0der, merge y deploy (los hace c0der; cada merge a `main` despliega a producción).
- **Próximo paso:** c0der mergea la PR. Para levantar la pausa: borrar la entrada (vuelve a 100) o poner otro valor,
  sólo con decisión del dueño.

## Medido antes de tocar (base `main` @ 6112bd8d)

- `main.tf:1134-1144`: el bloque TEMPORARY del 2026-09-26 con `value = "300"` — coincide con el encargo.
- `src/erc8004/daily_cap.rs:26-32`: `0` rechaza toda escritura en la red; built-in de Ethereum 100 (`:65-70`). El test
  `variables_override_the_built_in_limits` ya cubre que `0` hace fallar `reserve` (lo usa con `BASE_SEPOLIA`).
- `git grep -n 'ERC8004_DAILY_WRITE_CAP'`: ningún test ni script fija `"300"`. Sólo lo mencionan `CHANGELOG.md:48` y
  `docs/handoffs/X-TANDA-R425.md` (documentación histórica; no se tocan por el límite de un solo archivo de código).

## Verificación

Desde `terraform/environments/production`:

- `terraform fmt -check` → exit 3, lista `cicd-iam-policy.tf`, `cloudwatch-near-metrics.tf`, `observability.tf`,
  `variables.tf`: exactamente la misma lista (y exit) que sobre `main` sin el cambio, así que el cambio no agrega
  diferencias de formato. `terraform fmt -check main.tf` → exit 0.
- `TF_DATA_DIR=$HOME/tfdata-xrl01 terraform init -backend=false` y `terraform validate` →
  `Success! The configuration is valid.` (exit 0). Sin `plan` ni `apply`.
