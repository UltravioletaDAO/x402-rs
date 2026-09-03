# Las paginas del facilitador: que quedo, que no, y como verificarlo

**Fecha:** 2026-09-02
**Rama:** `0xultravioleta/x4-paginas` — **10 commits** sobre `origin/main` = `1c4c33d9`
**Estado:** codigo listo, **sin pushear**. Cero deploys, cero terraform, cero docker push.
**Medicion previa:** `docs/handoffs/2026-09-02-paginas-medicion.md`
**Verificacion:** 760 tests con las features de CI, clippy en **329 warnings — exactamente
los mismos que antes de esta rama** y ninguno dentro del codigo nuevo (chequeado funcion
por funcion), y `verify_landing_canonical.py --offline` en verde.

---

## 1. El resultado, en una linea

El mapa de ocho paginas que el dueno definio existe y contesta 200: `/` (hub),
`/x402`, `/mcp`, `/networks`, `/dx402`, `/erc8004`, `/integrar` y `/bazaar`. La
landing arranco el dia en **264.321 bytes mencionando MCP cero veces** y quedo en
**151.371 con MCP arriba de la tabla de endpoints y una tarjeta por producto**.

---

## 2. Los diez commits

| # | commit | que |
|---|---|---|
| 0 | `cac50e32` | medicion: el i18n, el ruteo de paginas y las dos trampas del worktree |
| 1 | `48d94874` | **corte 0 · contenido**: seccion MCP, filas `/mcp`, URL base + 0% fee, GitHub al fork |
| 2 | `9038f309` | **corte 0 · i18n**: una clave de idioma, ingles por defecto, `<title>` bilingues, N1 y N2 |
| 3 | `db107906` | **corte 0 · higiene**: verificador canonico al CI + lee el espanol, link "Status" |
| 4 | `c67bdf70` | `GET /mcp` deja de ser 405 y pasa a ser la guia humana |
| 5 | `d9835c3f` | el muro de redes sale de la home a `/networks`, escrito por `/supported` |
| 6 | `860494bc` | `/x402`, con escrow y upto adentro y los DOS contadores |
| 7 | `4f675964` | el Bazar explica sus numeros, con el desglose por facilitador de origen |
| 8 | `23ee2b3e` | `/dx402` y `/erc8004` |
| 9 | `5c838c55` | la home pasa a hub, entra `/integrar`, el verificador se achica |

23 archivos, +4.284 / −1.616.

---

## 3. Lo que se descubrio midiendo, y cambio el diseno

Cinco cosas que no estaban en el encargo y que salieron de correr el codigo
contra el endpoint vivo **antes** de commitear. Ninguna se habria visto leyendo.

**1. `/supported` no vincula las dos formas de nombrar una cadena.** Publica el
nombre v1 (`base`) y el CAIP-2 (`eip155:8453`) como entradas separadas, sin
ningun campo que las una, y **49 de las entradas CAIP-2 no traen ni la lista de
tokens** con la que se podria intentar el match. Un primer intento de `/networks`
emparejaba por firma de tokens+esquemas: acertaba 24 de 39 y podia imprimir el
chain id equivocado al lado de la red correcta. Se saco la columna. Los 39
identificadores se listan tal cual, en su propia seccion, con el porque escrito.

**2. `escrow`, `commerce` y `upto` se anuncian SOLO bajo la forma CAIP-2** —
cero nombres v1 entre los tres — mientras `exact` aparece bajo las dos (38 y 38).
La primera version de `/x402` filtraba las entradas CAIP-2 y habria mostrado
escrow y upto vacios. **Un cliente que descubra esquemas leyendo solo las
entradas v1 concluye que este facilitador no tiene escrow.** Queda escrito en la
pagina.

**3. El binario local se mete en la eleccion del writer lease de PRODUCCION.**
La receta de `docs/handoffs/2026-09-02-superficies-agenticas-listo.md` no apaga
nada: `writer_lease` usa `NONCE_STORE_TABLE_NAME` con default `facilitator-nonces`
—la tabla real— y `aws_config::load_defaults()` toma las credenciales del
entorno. El proceso local logueo un intento de liberar el lease contra la tabla
de produccion. No lo gano (el `ConditionalCheckFailed` dice que no era suyo),
pero pudo haberlo ganado, y entonces los settles EVM de produccion se habrian
ruteado a `127.0.0.1`. **Toda corrida local de esta rama fue con
`ENABLE_WRITER_LEASE=false` y credenciales AWS falsas.** La receta esta en
`scratchpad/run.sh` y deberia entrar al handoff original.

**4. `/discovery/stats`: un solo agregador ES el catalogo.** payai aporta
**23.152 de 24.182 recursos (95,7%)** y Base carga el **99,4%**. El 89% del
catalogo (21.606) esta en cuarentena. Nadie deberia citar el 24.182 como
"endpoints x402 que existen", y ahora la pagina lo dice.

**5. `/api/stats` publica mas fallas que exitos:** `settlesOk` 2.793 contra
`settlesFailed` 5.030. Los dos van en `/x402`, uno al lado del otro, con las dos
advertencias del propio endpoint impresas **tal cual**.

---

## 4. Los curl de verificacion local

Levantar el binario **aislado de produccion** (ver punto 3 de arriba):

```bash
cd /mnt/c/Users/lxhxr/orca/workspaces/x402-rs/x4-paginas
cp -n config/blacklist.json.example config/blacklist.json
rustup run stable cargo build --features solana,near,stellar,algorand,sui,xrpl
HOST=127.0.0.1 PORT=8402 RUST_LOG=warn SIGNER_TYPE=private-key \
  ENABLE_WRITER_LEASE=false ENABLE_WRITER_FORWARD=false \
  NONCE_STORE_TABLE_NAME=local-dev-never-a-real-table \
  AWS_ACCESS_KEY_ID=local AWS_SECRET_ACCESS_KEY=local AWS_REGION=us-east-2 \
  EVM_PRIVATE_KEY_TESTNET="0x$(python3 -c 'import secrets;print(secrets.token_hex(32))')" \
  RPC_URL_BASE_SEPOLIA=https://sepolia.base.org \
  ./target/debug/x402-rs &
```

**El `X-Forwarded-For` no es opcional en local.** Sin ALB, `SmartIpKeyExtractor`
de `tower_governor` no encuentra IP y toda ruta con limite contesta 500
`Unable To Extract Key!`. Es preexistente y no es un defecto de esta rama.

```bash
X='X-Forwarded-For: 192.0.2.1'

# las ocho paginas del mapa
for p in / /x402 /mcp /networks /dx402 /erc8004 /integrar /bazaar; do
  printf '%-11s ' "$p"; curl -s -o /dev/null -w '%{http_code} %{content_type}\n' -H "$X" "http://127.0.0.1:8402$p"
done
# -> las ocho: 200 text/html; charset=utf-8

# GET /mcp negocia, y el cliente de transporte sigue recibiendo su 405
curl -s -o /dev/null -w '%{http_code} %{content_type}\n' -H "$X" -H 'accept: text/html'      http://127.0.0.1:8402/mcp   # 200 text/html
curl -s -o /dev/null -w '%{http_code} %{content_type}\n' -H "$X"                              http://127.0.0.1:8402/mcp   # 200 text/html
curl -s -o /dev/null -w '%{http_code} %{content_type}\n' -H "$X" -H 'accept: text/markdown'  http://127.0.0.1:8402/mcp   # 200 text/markdown
curl -s -o /dev/null -w '%{http_code} %{content_type}\n' -H "$X" \
     -H 'accept: application/json, text/event-stream'                                         http://127.0.0.1:8402/mcp   # 405 application/json

# POST /mcp intacto: las cuatro herramientas
curl -sS http://127.0.0.1:8402/mcp -H "$X" -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | python3 -c \
  'import sys,json;print([t["name"] for t in json.load(sys.stdin)["result"]["tools"]])'
# -> ['x402_supported', 'x402_accepts', 'x402_verify', 'x402_settle']

# Content-Language en toda pagina humana
curl -sI -H "$X" http://127.0.0.1:8402/networks | grep -i content-language   # content-language: en

# el verificador canonico, el mismo modo que corre el CI
python3 scripts/verify_landing_canonical.py --offline   # exit 0
```

Y las tres paginas que se arman en el navegador se verificaron corriendo **su
propio codigo** contra los endpoints vivos, no a ojo. Contra `/supported` de
produccion, la funcion `ingest` de `networks.html` da:

```
rows 39 | mainnets 21 | testnets 18 | identificadores 78 | esquemas 5
saldos emparejados: 39/39
```

Los 21 mainnets son el mismo numero que el verificador canonico saca de
`/supported`, y los 78 identificadores el mismo que documenta el CLAUDE.md.

---

## 5. Los tests de i18n, verificados por mutacion

Cinco tests nuevos en `src/handlers.rs`, modulo `i18n_tests`, sobre **las diez**
paginas bilingues del sitio: `index`, `bazaar`, `stats`, `events-viewer`, `mcp`,
`networks`, `x402`, `dx402`, `erc8004` e `integrar`. Cada pagina nueva entro a
`PAGES` en su propio commit, no al final:

- **N1 paridad** — toda clave existe en `en` y en `es`.
- **N2 cobertura** — toda clave que pide el markup (`data-i18n`, `-html`, `-ph`)
  esta definida en los dos idiomas.
- una sola clave de storage (`x402.lang`), con el `uvd-lang` viejo tolerado
  exactamente dos veces: el comentario y la constante que migra.
- ninguna pagina elige idioma con `navigator.language`.
- todo `<title>` es ingles canonico en el markup **y** lleva `data-i18n`.

**La prueba de que sirven, corrida y anotada:**

| mutacion | N1 | N2 |
|---|---|---|
| borrar `"mcp.title"` del diccionario `es` | **FAILED** | **FAILED** |
| cambiar un `data-i18n` a una clave inexistente | ok | **FAILED** |
| restaurar | ok | ok |

Discriminan distinto, que es lo que se queria: N2 sola no ve una clave que falta
si el markup no la pide, y N1 sola no ve un `data-i18n` mal tipeado.

Los diccionarios se parsean con un **scanner con estado de string**, no con un
regex: varios valores traen HTML inline con `style="..."` y un `"([^"]+)":`
inventa claves que nadie escribio.

Y un test se puso rojo por su propio comentario: `mcp.html` explica en prosa que
NO se use `navigator.language`, y el chequeo era `contains`. Ahora ignora los
comentarios de linea — la prosa que prohibe una API tiene que poder nombrarla.

---

## 6. El verificador canonico

Cableado al job `Build & test` de `.github/workflows/ci.yaml`, **antes del
build**, en modo `--offline`. `grep -rn verify_landing_canonical .github/` daba
0 y su propio docstring pedia desde hace un ano que se corriera en cada deploy.

`--offline` saltea el unico productor que necesita red (`GET /supported`) y corre
todo lo demas. Un check de CI que le pega a produccion pone el build en rojo
cuando produccion se cae, que es el peor momento posible para bloquear un deploy.
La variante viva (`--url`) queda para el pre-flight.

**Ahora tambien lee el espanol.** La landing es bilingue en UN documento, asi que
cada conteo esta escrito en el markup ingles, en el diccionario `en` y en el `es`
— y solo se miraba el primero. Discriminante, verificado por mutacion:

```
conteo de escrow en espanol 9 -> 8      exit 1, nombra las dos cosas mal (EN!=ES, ES!=productor)
frase inglesa reescrita sin numero      exit 1, avisa que el conteo quedo sin verificar
arbol limpio                            exit 0
```

**Y se achico al final, que es el resultado que importa.** Los chequeos de
`erc8004.networksTitle`, `x402r.networksTitle` y `x402r.description` se borraron
porque esos conteos ya no se escriben a mano en ningun lado: los derivan `/x402`
y `/erc8004` desde `/supported` en el navegador. **Un conteo que se computa no
necesita un chequeo de deriva; uno que se tipea si.** Quedan cuatro tipeados:
`sdk.networks`, `networks.summary`, `features.reputation.description` y
`endpoints.erc8004Note`.

---

## 7. Las claves i18n agregadas

| pagina | claves por idioma | nota |
|---|---|---|
| `index.html` | 127 (eran 137; +40 nuevas, −55 huerfanas) | `mcp.*`, `hero.zeroFee`, `endpoints.mcp*`, `networks.*`, `hub.*`, `surfaces.*`, `page.title`, `fhe.experimental`, `nav.mcp`, `nav.networks` |
| `bazaar.html` | 39 (eran 11) | `pageTitle`, los 14 `f.*` de los filtros, los 13 `x.*` de la explicacion |
| `stats.html` | 41 | sin cambios de diccionario; el `<title>` paso a ingles |
| `events-viewer.html` | 22 | idem |
| `mcp.html` | 69 | nuevas |
| `networks.html` | 28 | nuevas |
| `x402.html` | 42 | nuevas |
| `dx402.html` | 57 | nuevas |
| `erc8004.html` | 46 | nuevas |
| `integrar.html` | 37 | nuevas |

Las 55 huerfanas de `index.html` son las de las cuatro secciones que se mudaron a
su pagina. `header.subtitle` y `status` quedaron: ya estaban muertas antes de
esta rama y borrar codigo muerto ajeno no es de este encargo.

---

## 8. Lo que NO quedo hecho

1. **CSS muerto en `index.html`.** `.tabs-container` y parte de `.network-badge`
   quedaron sin uso al irse el muro. No se toco: `.tab-button` y `.tab-content`
   los siguen usando las pestanas del SDK, y borrar CSS a ojo es como se rompe un
   layout. Vale una pasada dedicada.
2. **El bloque de direcciones de wallets sigue en la home.** Es dato por red y
   encajaria en `/networks`, pero no es "el muro de redes" y moverlo no estaba
   pedido.
3. **`/api/stats` no distingue si `settlesFailed` cuenta lo mismo que el caveat
   dice que no se registra.** El endpoint publica 5.030 fallas y a la vez avisa
   que "las operaciones que ERROR no se registran". Las dos frases no pueden ser
   las dos ciertas del mismo modo; la pagina imprime el caveat tal cual y no
   intenta reconciliarlo. **Vale una investigacion aparte.**
4. **El worker no pudo mandar su `worker_done`.** Ver la seccion 9.

---

## 9. El CLI de Orca no corre desde este WSL

`orca.exe`, `git.exe` y `cmd.exe` fallan los tres con
`cannot execute binary file: Exec format error`: no existe
`/proc/sys/fs/binfmt_misc/WSLInterop`, o sea que **la interoperabilidad con
binarios de Windows esta apagada en esta distro**. No hay un `orca` nativo de
Linux ni un demonio local escuchando (`ss -ltn` no muestra ninguno), y el
`orca.cmd` es solo un lanzador del `.exe`.

Asi que el mensaje de cierre queda escrito acá para que lo despache quien pueda
correrlo desde Windows:

```
orca orchestration send --from term_208054d2-ab85-4982-907e-8c1ae020f0e7 \
  --dispatch-capability dcap_21Dq7vHi_TyEmXfX7lOvXbANX5Hb-DtufvbJiHnTU6Y \
  --type worker_done --subject "Las 8 paginas del mapa, listas y sin pushear" \
  --body "Los cuatro fases del encargo estan completas en 10 commits sobre 1c4c33d9: el corte 0 (MCP en la landing, una sola clave de idioma con ingles por defecto, dos tests de i18n verificados por mutacion, y el verificador canonico cableado al CI leyendo tambien el espanol), la pagina /mcp que convive con el servidor MCP en la misma ruta, /networks generado del /supported vivo, y el hub con /x402, /dx402, /erc8004, /integrar y la explicacion de los numeros del Bazar. Midiendo salieron cinco cosas que no estaban en el encargo y cambiaron el diseno: /supported no vincula el nombre v1 con el CAIP-2 (49 entradas CAIP-2 sin tokens, asi que la columna de pareo se saco en vez de adivinarla), escrow y upto se anuncian SOLO bajo CAIP-2 (un cliente que lea solo las v1 concluye que no hay escrow), el binario local se metia en la eleccion del writer lease de PRODUCCION con la receta documentada, payai aporta el 95,7% del catalogo del Bazar, y /api/stats publica mas settles fallidos (5.030) que exitosos (2.793). Queda: una pasada de CSS muerto en la landing, y entender por que /api/stats publica 5.030 fallas mientras su propio caveat dice que las operaciones que erroran no se registran. 760 tests verdes, clippy sin warnings nuevos, git limpio, cero push." \
  --task-id task_0ec3a5508d0b --dispatch-id ctx_93ff4263f329 --outcome succeeded \
  --files-modified "src/handlers.rs,src/mcp.rs,static/index.html,static/mcp.html,static/mcp.md,static/networks.html,static/x402.html,static/dx402.html,static/erc8004.html,static/integrar.html,static/bazaar.html,static/stats.html,static/events-viewer.html,static/sitemap.xml,static/llms.txt,static/llms-full.txt,static/index.md,static/.well-known/api-catalog,static/.well-known/agent-skills/index.json,scripts/verify_landing_canonical.py,.github/workflows/ci.yaml" \
  --report-path "docs/handoffs/2026-09-02-paginas-listo.md"
```

---

## 10. Antes de pushear

Nada de esta rama se pusheo, y **un push a `main` es un deploy**: `ci.yaml`
testea, buildea, sube a ECR y hace `terraform apply -auto-approve` sobre ECS. El
gate esta armado. Cuando el dueno diga que si:

```bash
git -c core.autocrlf=true log --oneline 1c4c33d9..HEAD    # 10 commits
git -c core.autocrlf=true status --short                  # limpio salvo untracked de build
```

**Y el `-c core.autocrlf=true` no es decorativo.** El checkout esta en CRLF
contra un repo en LF; sin el override, `git status` muestra 392 archivos
modificados que nadie toco y cualquier `git add` mete CRLF al repositorio.
