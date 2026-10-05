# X4114R1 - PR #114, ronda del refutador (REF-X4114)

## Estado

- Hecho: identidad exacta de la oferta separada de la clave de drift
  (`DeclaredOffer.chain` CAIP-2 completo; `network` sigue siendo la clave
  conservadora por familia); asset/payTo sin case-folding fuera de EVM
  (`canonical_address`, `client_address`); la opcion viva solo cuenta como
  oferta si su `scheme` deserializa literal como `types::Scheme` y sus
  direcciones EVM son `0x` + 40 hex; WARN de la opcion extra extraido a
  `log_extra_networks` con test de captura; preview UTF-8 del agregador corta
  en frontera de caracter y la fixture de pagina invalida es multibyte.
- Tests nuevos: `discovery_health::strict_offer_identity_tests` (P1-1, P1-2,
  P1-3, EVM legitimo, control positivo, WARN) y
  `discovery_aggregator::tests::a_page_past_the_cap_that_does_not_parse_is_stepped_over`
  con HTML multibyte.
- Falta: nada del codigo; el resultado de suite, mutaciones y clippy esta en
  el cuerpo del PR #114 y en el comentario de la ronda.
- Proximo paso: refutacion de c0der sobre el nuevo head.

## Pregunta abierta (no decidida aqui)

Un destinatario declarado en CUALQUIER red sigue sin ser drift en otra red
declarada (regla previa a esta PR, rojo de base del refutador
`recipient_declared_elsewhere_is_not_declared_on_base`). Cambiarlo es politica,
no parte de esta ronda.
