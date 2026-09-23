# El logo de cada red, al menos tan grande como los de sus stablecoins — handoff 2026-09-23

- **Rama**: `c0der/iconos-red`, apilada sobre `c0der/hedera-reprobe` (#101, `4e1a1bb0`). Sin PR
  todavía: cuando #101 se mergee, `git rebase --onto origin/main c0der/hedera-reprobe c0der/iconos-red`,
  CHANGELOG/VERSION 2.39.5 y recién ahí el PR.
- **Pedido del dueño**: *"Los de los blockchains tienen que ser igual de grandes que los de los
  stablecoins, como mínimo"*.
- **Alcance**: solo `static/index.html` (el logo junto al nombre, en las 43 tarjetas de
  Mainnets y Testnets que tienen stablecoins) y su test. Stablecoins, tipografía, colores,
  bordes, orden de la grilla y la barra de filtros, sin tocar.

## La causa, medida

Los PNG de red de 96 px traen 12 px de margen transparente por lado (75 % opaco, medido con
PIL sobre el canal alfa), igual que los de stablecoin. Las stablecoins ya recortan ese margen:
una imagen de 42,67 px dentro de una caja de 32 px deja un glifo visible de 32 px, y la píldora
con su aro mide 38,78 px. Los logos de red no lo recortaban: caja de 32 px, **glifo visible de
24 px**. Excepciones: `arc.png` es 100 % opaco, `hedera.png` 90,27 %, `stellar.png` 81,25 % y
`polygon.png` 70,83 %.

## El cambio

Los 43 `<img>` pasan de `style="width: 32px; height: 32px; object-fit: contain;"` a
`class="network-logo"`. Arc y Hedera conservan su `border-radius: 50%` inline. La regla CSS:

- `--glyph: 40px`, el glifo visible: 40 px ≥ 38,78 px de la píldora.
- La imagen mide `--glyph / --visible`, donde `--visible` es la parte opaca del asset; las
  cuatro excepciones van por selector de `src`, como ya se hace con `rlusd` en las stablecoins.
- Márgenes negativos dejan la huella en el ancho del glifo y en los 32 px de alto que tenía
  la fila. El nombre conserva su separación y queda centrado sobre el logo, y **ninguna
  tarjeta crece**: 220,91 px antes y 220,92 px después, y las capturas miden lo mismo antes y
  después (2672×3056 la grilla, 620×444 la tarjeta).

## Medición (Chrome headless, DPR 2, portada servida en local)

`logo caja` = rectángulo del `<img>`; `logo glifo` = caja × parte opaca del PNG; `stablecoin` =
`.token-logo`; `píldora` = `.token-pill`; `centro nombre − centro logo` = desalineación vertical.

| Ancho | Tarjeta | Antes: logo caja / glifo | Después: logo caja / glifo | Stablecoin / píldora (igual antes y después) | Centro nombre − centro logo | Alto tarjeta antes → después |
|---|---|---|---|---|---|---|
| 1440 | Base (EVM) | 32 / 24 | 53,33 / **40** | 32 / 38,78 | 0 | 220,91 → 220,92 |
| 1440 | Solana | 32 / 24 | 53,33 / **40** | 32 / 38,78 | 0 | 220,91 → 220,92 |
| 1440 | Hedera | 32 / 28,8 | 44,30 / **40** | 32 / 38,78 | 0 | 220,91 → 220,92 |
| 1440 | Ethereum | 32 / 24 | 53,33 / **40** | 32 / 38,78 | 0 | 220,91 → 220,92 |
| 390 | Base (EVM) | 32 / 24 | 53,33 / **40** | 32 / 38,78 | 0 | 220,91 → 220,92 |
| 390 | Solana | 32 / 24 | 53,33 / **40** | 32 / 38,78 | 0 | 220,91 → 220,92 |
| 390 | Hedera | 32 / 28,8 | 44,30 / **40** | 32 / 38,78 | 0 | 220,91 → 220,92 |
| 390 | Ethereum | 32 / 24 | 53,33 / **40** | 32 / 38,78 | 0 | 220,91 → 220,92 |

## Capturas (1440 px, oscuro)

Antes:

![Mainnets antes](assets/iconos-red-antes-mainnets-1440-oscuro.png)
![Ethereum antes](assets/iconos-red-antes-tarjeta-ethereum.png)

Después:

![Mainnets después](assets/iconos-red-despues-mainnets-1440-oscuro.png)
![Ethereum después](assets/iconos-red-despues-tarjeta-ethereum.png)

Servidas en local con `/health/ready` todo en `ok` (sin puntos, para que no distraigan) y un
`/supported` de producción leído a las 13:48Z. Los saldos se ven como `—` a propósito.

## Test

jsdom no hace layout, así que no puede medir tamaños renderizados. El test de
`tests/frontend-capabilities.test.cjs` («every network logo shows a glyph at least as large as
a stablecoin icon») calcula lo que pinta la página:

- decodifica cada PNG de red con `zlib` (paleta con `tRNS` y RGBA) y mide su parte opaca. Da
  los mismos valores que PIL: 0,7500 / 0,7083 / 0,8125 / 0,9027 / 1,0000;
- lee `--glyph`, `--visible` y las excepciones del CSS, y el tamaño de la píldora de
  `.token-logo` + `.token-pill`;
- exige que cada tarjeta con stablecoins tenga su logo con la clase, que ninguna vuelva a la
  caja fija de 32 px, que el CSS no se aparte del PNG en más de 0,01 y que el glifo pintado sea
  ≥ la píldora.

Mutaciones, todas en rojo: `--glyph` a 32 px, quitar la excepción de Polygon, devolver una
tarjeta a la caja fija de 32 px y quitarle la clase a una tarjeta.

Suite local: x402-rs 2706 tests pasan y 0 fallan (los tests de Rust también leen
`index.html`); frontend 14/14; `verify_landing_canonical --offline` OK.
