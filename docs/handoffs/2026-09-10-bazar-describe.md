# describe.net enters the bazaar as a first-class citizen

> Created: 2026-09-10
> Scope: curation + storefront only. No terraform, no deploy, no registration.
> Companion worker: `dn-bazar-registro` in the `describe-net` repo owns the registration half.

## What the owner asked

> "me gustaria que describe-net fuera tambien un first-class citizen en el
> facilitador Ultravioleta y que se estuviera registrado en el bazar"

Two halves. This change is the first one: describe.net is now a curated
`first_party` entry in the manifest and it is named on the bazaar page next to
the other three. The second half — the resources existing in
`GET /discovery/resources` at all — is not ours and is not done; see
[Para c0der](#para-c0der).

## What changed

Three files.

**`config/bazaar_curation.json`** — a fourth `first_party` entry, in the same
shape as Execution Market and MeshRelay: `name`, `tier`, `prefixes`, `homepage`,
`expectedPayTo`, `$evidence`. Three prefixes, all on `api.describe.net`:

| Prefix | Why this one |
|---|---|
| `/reputation/` | Covers the four paid reputation routes with one path-boundary match; nothing free lives under it |
| `/leaderboard/page` | The paid deep page. Written with the `/page` segment because `/leaderboard` itself is free |
| `/mcp` | The MCP server, free at the door, paid tools relaying their own route's 402 |

No `erc8004` field. That is a measurement, not an omission — see below.

**`static/bazaar.html`** — one line in the `VIP` array, the same
`{name, tier, url, match, blurb:{en,es}}` shape as its three neighbours. No
redesign, no new i18n keys: the blurbs in that array are inline per language, so
nothing was added to either dictionary.

**`src/discovery_curation.rs`** — `load()` now goes through a private
`parse(&str)`, so a test can hand the module the file that actually ships
instead of a hand-built copy that drifts from it. Four tests, described below.

## The evidence, all of it dated 2026-09-10

Every paid route was probed with a real GET and answered 402 with the same six
`accepts` and the same single `payTo`:

| Route | HTTP | amount |
|---|---|---|
| `/reputation/wallet/{w}` | 402 | 10000 |
| `/reputation/wallet/{w}/history` | 402 | 30000 |
| `/reputation/rater/{r}` | 402 | 10000 |
| `/reputation/agent/{chain}/{id}` | 402 | 20000 |
| `/leaderboard/page` | 402 | 10000 |

Networks on every one of them: `eip155:8453`, `eip155:43114`, `eip155:42161`,
`eip155:10`, `eip155:137`, `eip155:42220`. That is six, not the three the
assignment expected — Optimism, Polygon and Celo are live on describe.net too,
and `GET /pricing` agrees (`networks: [base, avalanche, arbitrum, optimism,
polygon, celo]`).

`payTo` on all five: `0xe4dc963c56979E0260fc146b87eE24F18220e545`.

The free routes were probed too, because the prefixes are only as good as what
they exclude: `GET /leaderboard` and `GET /health` both answered 200, and
`/leaderboard/page` cannot match either.

`POST /mcp` answered 200 to an `initialize` handshake (`serverInfo` =
`describe.net` 2.0.0, protocol 2025-06-18) and `tools/list` returned 14 tools.
`mcp.describe.net` does not resolve; the MCP lives on `api.describe.net/mcp`.
`/.well-known/x402` is 404 on both `describe.net` and `api.describe.net`.

### The shared payTo is not a bug

`0xe4dc…e545` is byte-identical to MeshRelay's `expectedPayTo` in the same file,
checksum casing included. It is the owner's shared collection wallet. MeshRelay
could not be re-probed the same day to confirm it still uses it:
`GET /channels` answered `{"channels":[],"total":0}` and both
`/payments/access/` paths answered 404, so MeshRelay serves no paid route today.
The address match is therefore against the repository's own recorded value, not
against a second live 402.

`the_shared_collection_wallet_stays_a_measured_value` pins both entries to that
address. A future rotation is meant to turn it red: the file's `$comment` asks
for live-verified evidence, so changing a `payTo` should cost a re-probe.

### No ERC-8004 identity, and why

`GET /.well-known/agent-card.json` answers 200 on both hosts. The card declares
`protocolVersion` 0.3.0, `provider.organization` "Ultravioleta DAO", two skills
and a `documentationUrl` — and no registration, no `agentId`, no registry
address. The payTo wallet does hold twelve ERC-8004 identities (nine on Base,
three on Avalanche, per describe.net's own free `GET /wallets/{payTo}/chains`),
but that wallet is shared across the owner's products, so none of those ids can
be attributed to describe.net from outside. The field is omitted rather than
guessed. Nothing in `attest_targets()` changes, since it already skips entries
with no `erc8004`.

### The one claim not verified from here

That describe.net settles through *this* facilitator. The assignment cites
`describe-net/describenet/paywall.py:84` and `mcp_server.py:129` pinning
`FACILITATOR_URL=https://facilitator.ultravioletadao.xyz`; that repository is
not checked out on this machine. describe.net's public surface never names a
facilitator — not the 402 body, not `/openapi.json`, not `/skill.md` — and the
last 200 rows of our own `GET /transactions` carry no matching `payTo`, which
proves nothing either way given that index is a fire-and-forget sample rather
than a ledger. The `$evidence` string says so in those words.

## Tests

Four new ones in `src/discovery_curation.rs`, all against the shipped file via
`include_str!` so they cannot pass on a copy:

- `describe_net_is_first_party_on_its_paid_routes_only` — the six paid URLs
  resolve `first_party` with `alive=false`, so nothing is explained by the health
  fallback; the four free ones resolve to nothing.
- `nothing_that_merely_looks_like_describe_net_inherits_the_tier` — the
  `.evil.com` suffix, the `api-describe.net` lookalike, the bare apex, plain
  `http`, and a userinfo prefix all get no tier.
- `the_shared_collection_wallet_stays_a_measured_value` — pins the address on
  both entries and requires every `first_party` entry to carry `$evidence`.
- `the_bazaar_page_names_every_first_party_entry` — every `first_party` name and
  homepage appears in `static/bazaar.html`. That page hardcodes its own
  showcase, so without this a product can be curated by the API and invisible on
  the page, or listed on the page with no tier behind it.

Verified by mutation, not by colour. Loosening `/leaderboard/page` to
`/leaderboard` turns the first red with `left: Some(FirstParty)`; deleting the
page line turns the fourth red; flipping one hex digit of the payTo turns the
third red. Worth recording that the near-miss `/leaderboard/` does *not* go red,
and correctly so: `match_manifest_prefix` only prefix-matches past a trailing
slash, so `/leaderboard/` still excludes `/leaderboard`. The pinned path is the
narrower of the two anyway.

Full CI gate run locally, green: `cargo fmt --all -- --check`,
`cargo clippy` (0 errors, warning count unchanged),
`python3 scripts/verify_landing_canonical.py --offline`,
`cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1`
(1746 passed, 0 failed) and the axum/reqwest/compliance suite (71 passed).

## Para c0der

**Lo que se agrego.** `config/bazaar_curation.json` tiene a describe.net como
cuarta entrada `first_party`, con tres prefijos sobre `api.describe.net`
(`/reputation/`, `/leaderboard/page`, `/mcp`), homepage `https://describe.net`,
`expectedPayTo` medido y un `$evidence` con las cinco sondas 402, las seis redes,
las rutas gratis que quedan afuera, el handshake del MCP y las dos cosas que NO
se pudieron verificar. `static/bazaar.html` lo nombra en el mismo formato que a
los otros tres. Cuatro tests nuevos, verificados por mutacion.

**Lo que NO se hizo, a proposito.** Nada se registro en
`POST /discovery/register`. Ese es el trabajo del worker `dn-bazar-registro` en
el repo `describe-net`. Tampoco se toco `terraform/`, ni el deploy, ni
`src/erc8004/*`, ni `handlers.rs` (worker `x4-mint-atomico` vivo en esos
archivos), ni `README.md` — la seccion del bazar ahi describe el mecanismo y no
nombra a ningun consumidor, asi que agregar nombres seria una lista nueva, no el
mismo formato.

**Lo que le falta al lado de describe-net para que aparezca en
/discovery/resources.** Esto es la mitad que decide si la curacion se ve o no.
`CurationManifest::resolve()` corre **por recurso ya listado**: el tier ordena y
etiqueta el listado, no lo crea. Al 2026-09-10
`GET /discovery/resources?limit=100` no devuelve ninguna URL de describe.net, asi
que hoy la entrada esta correcta y latente. Para que se encienda, describe-net
tiene que registrar sus recursos, y las URLs registradas tienen que caer dentro
de los tres prefijos de arriba — si registra `https://describe.net/...` (apex) en
vez de `https://api.describe.net/...`, el match falla por host exacto y el
recurso se lista como `verified`/`listed` sin la etiqueta. Publicar
`/.well-known/x402` (hoy 404 en los dos hosts) ayuda al crawler pero no es lo
que otorga el tier; el tier lo otorga este archivo.

Una vez registrado, la comprobacion es:

```bash
curl -s "https://facilitator.ultravioletadao.xyz/discovery/resources?limit=200" \
  | jq '[.items[] | select(.url|test("describe\\.net"))
        | {url, tier: .curation.tier, label: .curation.label, health: .health.status}]'
```

`tier: "first_party"` y `label: "describe.net"` es la señal de que las dos
mitades se encontraron.

**Un detalle operativo que conviene saber.** El sondeo de salud cuarentena un
recurso al instante si un 402 vivo anuncia un `payTo` que el listado nunca
declaro (`paytoswap`). describe.net anuncia `0xe4dc…e545` en las seis redes; si
el registro declara otra cosa, o si describe.net rota la wallet sin que se
actualice este archivo, el recurso se cuarentena y desaparece del listado por
defecto. Rotar esa wallet es un cambio de dos repos.
