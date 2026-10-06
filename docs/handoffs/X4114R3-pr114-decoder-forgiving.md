# X4114R3 - PR #114, tercera ronda del refutador (VER2-X4114)

## Estado

- Hecho: P2-1. `decode_payment_required` (`src/discovery_health.rs`) lee el
  header `PAYMENT-REQUIRED` con un decoder forgiving que es superconjunto de lo
  que leen los clientes que pagan (`Buffer.from(v, "base64")` en Node, `atob`
  en el navegador): los dos alfabetos (tambien mezclados), padding opcional,
  bits sobrantes ignorados, espacios y cualquier caracter fuera del alfabeto
  saltados, corte en el primer `=`, un simbolo final suelto descartado, y el
  JSON leido con UTF-8 invalido reemplazado (`from_utf8_lossy`). Antes solo
  `STANDARD` con padding o `URL_SAFE_NO_PAD`: un header en otra grafia contaba
  como ausente y el body, con la oferta declarada, tapaba un header que pagaba
  a otro.
- Medido con Node v22.23.3: `Buffer` lee el `payTo` en las 11 grafias de
  `client_readable_headers`; `atob` (forgiving-base64 del WHATWG) es mas
  estricto (rechaza `-`/`_` y caracteres basura), asi que todo lo que lee
  `atob` lo lee tambien `Buffer` y el decoder nuevo.
- Tests nuevos en `discovery_health::strict_offer_identity_tests`:
  `a_header_only_attacker_in_any_client_readable_base64_is_a_drift` (body
  declarado, header solo-atacante en 11 grafias -> `Drifted`) y
  `the_same_spellings_carrying_the_declared_offer_are_not_a_drift` (control:
  el mismo header con la oferta declarada se lee y da `Declared`).
- Verificado en Linux sobre `origin/main` `f544fa0c` (ya contenido, sin merge
  nuevo): workspace con red cerrada 3665 passed / 0 failed; clippy
  `--workspace --all-targets` sale 0 (sin warnings nuevos en lo tocado);
  mutaciones M1..M8 en rojo. Detalle en el comentario de la ronda del PR #114.
- Fuera de esta ronda, por decision de c0der: el punto 2 de la ronda
  (preexistente), P3-1 y P3-2.
- Falta: nada del codigo.
- Proximo paso: refutacion de c0der sobre el nuevo head.
