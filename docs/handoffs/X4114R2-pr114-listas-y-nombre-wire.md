# X4114R2 - PR #114, segunda ronda del refutador

## Estado

- Hecho: P1-A, `read_challenge` arma un `TransportRecipients` por clave
  (`accepts`, `paymentRequirements`) y `pay_to_from_402` los juzga por separado
  (el `payTo` v1 de nivel superior se nombra en cada lista). P1-B,
  `spelled_for_clients` acepta un nombre v1 solo si el serde derivado de
  `Network` lo lee y resuelve al mismo CAIP-2. P3: la guarda `0x` de
  `client_address` ya la mata `an_evm_address_a_client_cannot_read_is_not_the_declared_offer`
  (mutacion M5 en el cuerpo del PR); no se toca.
- Tests nuevos en `discovery_health::strict_offer_identity_tests`:
  `payment_requirements_decoy_does_not_vouch_for_accepts`,
  `each_list_key_still_vouches_for_its_own_extra_option`,
  `a_from_str_alias_is_not_a_v1_wire_name`,
  `the_v1_wire_name_still_vouches_for_the_extra_option`.
- Falta: nada del codigo; suite, mutaciones y clippy en el comentario de la
  ronda del PR #114.
- Proximo paso: refutacion de c0der sobre el nuevo head.
