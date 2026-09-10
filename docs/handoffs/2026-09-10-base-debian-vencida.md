---
date: 2026-09-10
tags:
  - type/handoff
  - domain/build
  - domain/deploy
  - priority/p0
status: active
---

# La base Debian del facilitador venció y se llevó el despliegue con ella

**Versión:** 2.17.0 · **Detectado por:** c0der, 2026-09-10 04:28Z, tras dos
despliegues fallidos seguidos del merge de `#28`.

`2.17.0` estaba en `main` desde las 04:07:42Z y no llegaba a producción, que seguía
sirviendo `2.16.0`. El build moría siempre en el mismo paso:

```
E: Release file for http://deb.debian.org/debian-security/dists/bullseye-security/InRelease
   is expired (invalid since 2d 7h 25min 40s).
ERROR: process "/bin/sh -c apt-get update && apt-get install -y ..." exit code: 100
```

No era transitorio. Debian 11 pasó a **oldoldstable** y dejó de re-firmar su `Release`
de seguridad: la última firma es del **31-ago-2026** y venció el **7-sep-2026
21:13:04 UTC**. `apt` rechaza por diseño un índice vencido, así que a partir de esa
fecha **todo** build del facilitador falla, para siempre, sin que nadie haya tocado
el repositorio.

La comparación que confirma que no es una fecha corta cualquiera: `bookworm-security`
respondía `Valid-Until: 16-sep-2026`, pero con `Date: 09-sep-2026 20:42:36 UTC` — o
sea, **re-firmada el día anterior**. Una suite viva renueva su `Release` cada semana y
por eso su ventana siempre se ve corta. La diferencia no es la fecha: es que bullseye
dejó de renovarla y bookworm no.

## Eran DOS etapas, no una

El `Dockerfile` tenía la base vencida en los dos extremos:

| Línea | Etapa | Base |
|---|---|---|
| 1 | `builder` | `rust:bullseye` |
| 86 | runtime | `debian:bullseye-slim` |

El fallo se veía solo en la 94 (el `apt-get` del runtime) porque la capa equivalente
del builder (línea 13) seguía cacheada en el runner. En un runner frío —o en cuanto
esa capa se invalide— el build muere en la 13 con el mismo error y sin relación
aparente con el arreglo. Verificado, no supuesto:

```
$ docker run --rm rust:bullseye sh -c 'cat /etc/apt/sources.list; apt-get update'
deb http://deb.debian.org/debian-security bullseye-security main
...
E: Release file for .../bullseye-security/InRelease is expired (invalid since 2d 7h 26min 23s).
```

**Y hay una segunda razón, más dura, para que las dos suban juntas.** `openssl-sys` es
el único crate del `Cargo.lock` que enlaza una librería *del sistema*, y no está
vendorizado. El binario queda atado al `SONAME` de la base donde se compiló:

```
$ docker run --rm --entrypoint sh facilitator:2.17.0-bookworm -c 'ldd /usr/local/bin/x402-rs'
    libssl.so.3    => /lib/x86_64-linux-gnu/libssl.so.3
    libcrypto.so.3 => /lib/x86_64-linux-gnu/libcrypto.so.3
    libc.so.6      => /lib/x86_64-linux-gnu/libc.so.6
```

bullseye trae `libssl.so.1.1`; bookworm trae `libssl.so.3`. Subir **una sola** etapa —en
cualquiera de las dos direcciones— produce una imagen que **construye limpia y después
no arranca**, porque el enlazador dinámico no encuentra el `SONAME` que el binario
pide. Ese es exactamente el modo de fallo contra el que protege el criterio de "el
contenedor levanta y `/health` responde 200", y por eso acá no es una formalidad.

## Qué elegí y qué descarté

**Elegido: subir la base en las dos etapas a bookworm** (`rust:bookworm` +
`debian:bookworm-slim`). Es el único de los tres caminos que vuelve a traer parches de
seguridad, y quedó medido en el propio build: la etapa de runtime instaló
`ca-certificates 20250419~deb12u1` **desde `bookworm-security`**, o sea el canal vivo
haciendo su trabajo en el primer build.

**Descartado — `archive.debian.org`.** Mantiene el diff en una línea y congela la
seguridad para siempre: bullseye no va a recibir un parche más, y la imagen de
producción quedaría anclada a un set de paquetes que ya nadie audita. Cambia un fallo
ruidoso por una deuda silenciosa.

**Descartado — `-o Acquire::Check-Valid-Until=false`.** Es el más chico y el peor.
Instala desde un índice vencido y **apaga la señal** que acaba de avisar que la base
murió: el próximo que llegue no tiene forma de enterarse, porque el build pasa verde.

**Considerado y pospuesto — trixie.** `trixie` es hoy la stable vigente (Debian 13.6;
bookworm ya es `oldstable`) y da más pista: bookworm llega hasta el EOL de LTS de
Debian 12, ~jun-2028. Lo dejé afuera a propósito. Este es un arreglo de incidente con
**un solo push disponible** y cero margen de `gh run rerun`, y trixie apila dos
incógnitas más sobre el mismo cambio: glibc 2.41 y openssl 3.5 contra el
`openssl-sys 0.9.111` del lock. bookworm/openssl 3.0 es la combinación más ejercitada
del ecosistema Rust hoy. Verifiqué que `rust:trixie` y `debian:trixie-slim` existen y
están frescas (ambas reconstruidas el 09-sep-2026), así que el salto a trixie es un
cambio de mantenimiento agendable, de las mismas dos líneas, no un incidente.

Dato que ayuda a decidir eso después: `rust:bullseye` quedó congelada en Rust 1.98.0
(última construcción 25-ago-2026), mientras `rust:bookworm` y `rust:trixie` se
reconstruyeron el 09-sep-2026 con 1.98.1. La imagen base de la que veníamos ya no
recibía ni al compilador.

## Verificación local

Build completo, sin caché de una base previa, en WSL sobre ext4:

```
#10 [builder  3/17] RUN apt-get update && apt-get install ... pkg-config libssl-dev   DONE 10.1s
#8  [stage-1 2/7]   RUN apt-get update && apt-get install ... ca-certificates curl    DONE 102.9s
#19 [builder 12/17] RUN cargo build --release --features solana,near,...              DONE 526.7s
#25 [builder 16/17] RUN cargo build --release --features solana,near,...              DONE 124.5s
#26 [builder 17/17] RUN grep -aq 'Ultravioleta' target/release/x402-rs                DONE 2.7s
#30 exporting to image                                                                DONE 4.2s
=> naming to docker.io/library/facilitator:2.17.0-bookworm
```

El paso `#10` es el `apt-get` del builder: con bullseye ese paso es el que moría en un
runner frío. El `#26` es la guarda que impide shippear el binario stub — pasó, así que
la landing quedó compilada adentro.

Imagen resultante: **183 MB**.

Contenedor levantado con una llave EVM aleatoria descartable y sin RPC configurados:

```
$ curl -w 'HTTP %{http_code}' http://localhost:18080/health
HTTP 200  {"status":"healthy"}

$ curl -w 'HTTP %{http_code}' http://localhost:18080/version
HTTP 200  {"version":"2.17.0"}

$ curl -w 'HTTP %{http_code} bytes=%{size_download}' http://localhost:18080/
HTTP 200 bytes=249073        # 9 ocurrencias de "Ultravioleta"
```

`/version` sirviendo `2.17.0` confirma además que el `--build-arg FACILITATOR_VERSION`
sigue llegando al binario después del cambio de base.

**Un matiz honesto:** en esa corrida `/supported` devuelve 6 identificadores, no los ~78
de producción. No es una regresión de la imagen: es que no configuré ningún `RPC_URL_*`
local, y el arranque saltea toda red sin RPC (`no RPC URL configured, skipping`). En
producción esas URLs llegan de `facilitator-rpc-mainnet` / `-testnet`. El conteo real
solo se puede leer contra el despliegue.

## Lo que no toqué

- `VERSION` sigue en `2.17.0`. El objetivo es que llegue a producción esa versión, no
  una nueva; subirla rompería el propio criterio de cierre.
- `Cargo.toml`, `Cargo.lock` y `rust-toolchain.toml`: sin cambios. El canal sigue siendo
  `stable`, así que rustup baja el mismo compilador en cualquiera de las bases; lo que
  cambió es glibc y libssl, no el `rustc`.
- `scripts/fast-build.sh` y el resto del tooling: no hay otra referencia a `bullseye` en
  el repo. Un `grep -rn` sobre `*.sh`, `*.yaml`, `*.toml`, `*.tf` y `Dockerfile*`
  devuelve solo las dos líneas del `Dockerfile`.
- `docs/CHANGELOG.md`: no lo toqué, ya venía atrasado respecto del release.

## Para c0der

**Camino elegido:** opción 1, subir la base — pero en **las dos** etapas
(`rust:bullseye` → `rust:bookworm` y `debian:bullseye-slim` → `debian:bookworm-slim`).
Descarté `archive.debian.org` (congela la seguridad para siempre) y
`Check-Valid-Until=false` (instala desde un índice vencido y apaga la señal). Pospuse
trixie a propósito: es la stable vigente y da más pista, pero apila glibc 2.41 y openssl
3.5 sobre un cambio que tiene un solo push; queda como mantenimiento agendable, mismas
dos líneas.

**El hallazgo que sube el peso del criterio 2:** el binario enlaza `libssl.so.3`
dinámicamente. Subir una sola etapa da una imagen que construye limpia y **no arranca**.
La verificación de `/health` no es una formalidad acá, y por eso la corrí de verdad.

**Build local:** completo, `#19` 526.7s + `#25` 124.5s, guarda `Ultravioleta` en verde,
imagen de 183 MB. Salida arriba.

**`/health` en la imagen nueva:** `HTTP 200 {"status":"healthy"}`. `/version` sirve
`2.17.0` y la landing responde 249.073 bytes.

**Lo que solo se puede verificar después del despliegue:**

1. `curl -s https://facilitator.ultravioletadao.xyz/version` → tiene que decir `2.17.0`
   (hoy dice `2.16.0`). Esa es la prueba, no el verde del workflow.
2. El conteo de `/supported` contra los RPC reales — local dio 6 por falta de
   `RPC_URL_*`, no por la imagen.
3. Que el runner de CI construya en frío sin la caché de bullseye. Localmente el
   `apt-get` del builder ya corrió contra bookworm (`#10`), así que el paso está
   probado; lo que no probé es el runner de GitHub en sí, y **no lo hice a propósito**
   por el presupuesto de Actions.

**No mergeé el PR.** Queda para vos.
