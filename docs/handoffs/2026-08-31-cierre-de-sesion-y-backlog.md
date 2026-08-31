---
date: 2026-08-31
tags:
  - type/handoff
  - domain/performance
  - domain/infrastructure
  - priority/p1
status: active
---

# Cierre de sesión — qué quedó hecho y qué queda pendiente

> Sesión larga: arrancó investigando por qué el facilitador rendía mal con 5-20
> peticiones y terminó arreglando la máquina de desarrollo. Todo lo de acá está
> verificado contra AWS o contra el código, no inferido.

## LO QUE QUEDÓ EN PRODUCCIÓN

Cuatro commits en `main`, producción en **2.0.1**:

| Commit | Qué |
|---|---|
| `9e8a9ce7` | `totalSupply()` reemplaza ~20 `eth_call` secuenciales en `/identity/owner` |
| `ecfda12b` | Governor en las rutas de lectura que no tenían ninguno + log de worker threads |
| `30efd6f0` | Drift de infra planchado, la config deja de vivir solo en una máquina |
| `9d2af8c0` | Gate de `terraform plan` que avisa cuando `main` tiene infra sin aplicar |

Y aplicado directo en AWS, verificado con `describe-*`:

- **Autoescalado real**: política `ALBRequestCountPerTarget` (15 req/min/target)
  creada, política de CPU huérfana borrada, `min_capacity` 1 → 2.
- **Alertas propias**: topic `facilitator-production-alerts`, 36 alarmas, cola
  SQS de respaldo, schedule de 15 min para la Lambda de balances.
- **Permisos de CI** hasta `v5` de `facilitator-cicd-infra`.
- **RPC de Sui arreglado** (mainnet y testnet): el endpoint viejo dejó de servir
  JSON-RPC por completo.
- **Celo recargada**: estaba en cero y perdía 409 de 973 settlements en 24h.

## LO QUE APRENDIMOS Y NO HAY QUE VOLVER A DESCUBRIR

**El facilitador no se caía por carga.** 216 settles en 4 horas son 0,015 req/s.
El problema no era que no escalara a miles: no escalaba a uno.

**El silencio se nos disfrazó de evidencia cuatro veces**, y conviene tenerlo
presente porque va a volver a pasar:

1. Los logs del allocator de nonces son `debug!`/`trace!` y producción corre en
   `info` → "cero ocurrencias" significaba "no se emite", no "no ocurre".
2. Un RPC podado devolvía `null` para transacciones sí minadas → infló el "88%
   de pérdida de monad" a un valor falso; el real medido fue **19,5%**.
3. Un `describe-clusters` **sin `--include SETTINGS`** omite el campo entero →
   se leyó como "Container Insights apagado" y casi se apaga en producción sobre
   esa premisa.
4. Correr el gate de tests **mientras los agentes editaban** dio un rojo que no
   era real.

**Tres incidentes de la semana salieron del mismo agujero**: CI aplica Terraform
con `-target`, así que un cambio fuera de esa lista entra a `main`, CI da verde,
y nunca llega a AWS. El gate de `plan` (commit `9d2af8c0`) existe para eso.

## BACKLOG DEL FACILITADOR

### P0 — lo más caro que sigue abierto

| # | Qué | Dónde |
|---|---|---|
| 1 | **Avisarle a Execution Market.** Van a ~13x el ritmo de lookups que ya tumbó `/settle` en julio (INC-2026-07-06). Es gratis y no espera ningún deploy. El handoff está escrito en `docs/handoffs/2026-08-20-respuesta-a-execution-market.md` y **nunca se envió** | — |
| 2 | **Celo pierde el 59% de sus settles** y NO es la wallet sin gas (se descartó con tres pruebas). Dos episodios: nonce congelado 6h el 21-ago, y 6 reverts de `/feedback` → 6 nonces quemados el 22-ago | `docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md` |
| 3 | **68 transacciones se minaron mientras devolvíamos error al cliente**, medidas sobre 957 emisiones en 9 cadenas. Plata movida, error reportado | ídem |

### P1 — los fixes que quedaron diseñados

| # | Qué | Riesgo |
|---|---|---|
| 4 | **El guard de los 14 `send()` crudos.** 5 funciones de `handlers.rs` mandan sin estimar gas, sobre el mismo `PendingNonceManager` que `/settle`: una estimación que revierte deja el nonce quemado. **Anclado en el código** (`evm.rs` y `handlers.rs:4189`) con las líneas exactas | Alto — `run_evm_registration` tiene máquina de estados async y no hay tests de regresión de nonce |
| 5 | **Fix #2 del nonce**: que el hueco sane bajo tráfico. Hoy `NONCE_TRUST_CHAIN_AFTER` (120s) se reinicia con cada asignación, así que **cuanto más tráfico tiene una cadena, menos puede curarse** | Alto |
| 6 | **Failover de RPC** con `FallbackLayer`, `active_transport_count=1` (el default de 3 en paralelo rompería el resync de nonce) | Medio |
| 7 | **Token de servicios propios** para el rate limiting. Diseñado en `docs/plans/trusted-caller-rate-limit-design.md`. Necesita un secret nuevo. **Techo generoso, no infinito**: EM reintentó 39 veces por submission durante un bug nuestro | — |
| 8 | **DX402 fase 1** (streaming). Diseño en `docs/plans/dx402/04-STREAMING-EVIDENCE-HANDOFF.md`. La fase 0 ya está hecha | — |

### P2 — infra

- Los **dos ARNs de retención de logs** en el permiso de CI: hoy los log groups
  de la Lambda no los puede arreglar ni el pipeline.
- **`access_logs.s3` del ALB**: rollout de dos pasos, hoy inerte a propósito.
  Sin eso el HTTP 460 es invisible, y fueron 201 de 417 fallos reales.
- **Alarma `v1-traffic-sudden-drop`** sigue apagada a propósito: su umbral (5
  req/h) nunca se calibró contra el tráfico real (900-1200 req/h).
- **`task_cpu` 1024 → 512** (~$15/mes) está **bloqueado** hasta saber cuántos
  worker threads levanta tokio — el log nuevo lo contesta en el próximo deploy.

## BACKLOG DE LA MÁQUINA DE DESARROLLO

WSL se cayó cuatro veces durante la sesión. **Dos causas distintas**, las dos
arregladas, y una tercera abierta.

### Arreglado

1. **Discos fantasma**: `E:` ("CorruptedOriginal") y `G:` ("CorruptedUnused")
   apuntan a un disco físico que **ya no existe** (`Get-Disk` lista 0, 1 y 3;
   falta el 2). WSL los montaba por 9p y tocarlos colgaba la VM.
   → `~/fix-wsl-mounts.sh`: automount apagado, montaje por fstab de solo C, D, Z.
2. **El NAT de WSL no arranca**: no existe el adaptador `vEthernet (WSL)` y hay
   **cuatro adaptadores de túnel** (Tailscale con `RouteAll: true`, ExpressVPN
   TAP/TUN/OpenVPN). WSL intentaba NAT, fallaba, y tardaba 15,4s en el fallback
   — más de lo que Docker Desktop espera (`Wsl/Service/0x8007274c`).
   → `networkingMode=mirrored` en `~/.wslconfig`. Arranque: **15,4s → 4,8s**.

### PENDIENTE INMEDIATO

**`WslService` quedó en `StopPending`** (pid 8212) por un `Stop-Service -Force`
mío que ese servicio no admite. Mientras esté así, `wsl --shutdown` no responde.
Desde PowerShell **como administrador**:

```powershell
taskkill /PID 8212 /F
Start-Service WslService
```

Si el `taskkill` no lo mata, ahí sí el reinicio es el único camino.

### Después

- **Compactar el VHDX**: ocupa 82,84 GB y adentro se usan bastante menos. El
  script está en `C:\Users\lxhxr\compactar-ahora.ps1` (admin). **No usar
  `--set-sparse`**: Microsoft lo deshabilitó por riesgo de corrupción y esta
  máquina ya tiene errores de NTFS.
- **`C:` al 6% libre.** Se liberaron ~36 GB (target de Rust, caché de npm,
  imágenes dangling y build cache de Docker) pero sigue apretado.
- **Los discos E: y G:**: quitarles la letra en Windows, o averiguar por qué se
  desconectó ese disco. Los errores `Delayed Write Failed` sobre `$Mft` y
  `$BitMap` siguen apareciendo varias veces por noche.
- **`F:` en `Warning`** (SanDisk Extreme USB, 7,4 TB), sin investigar. Quedó
  desmontado de WSL a propósito.
- **`em-redis` fue destruido** (contenedor y volumen). Su AOF estaba corrupto
  **desde el 17 de agosto**, no por estos crashes. Backup de 25,62 MB en
  `C:\Users\lxhxr\redis-backup`. Cuando Execution Market lo necesite, se recrea.

## ARCHIVOS SIN COMMITEAR AL CERRAR

En la rama `fix/writer-lease-forwarding` (otra sesión trabajó en paralelo):

```
?? docs/plans/trusted-caller-rate-limit-design.md   <- vale la pena commitearlo
?? docs/plans/batch-settlement/
?? docs/marketing/reply-0xjeff-batching-2026-08-25.txt
```

Y tres commits locales sin pushear, de otra sesión (FHE/Zama): `32a404ec`,
`0feaed15`, `61bf47de`.
