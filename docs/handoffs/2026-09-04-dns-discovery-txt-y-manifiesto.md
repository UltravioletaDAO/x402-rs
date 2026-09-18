# Descubrimiento x402 — el TXT `_x402` ya está; el manifiesto no habla el idioma del borrador

**Para:** facilitador (x402-rs). **De:** la sesión de KK, 2026-09-04 04:50Z.
**Contexto:** el operador se reúne hoy con el Tax Lead de x402.org, que participa en el grupo de
descubrimiento de la Fundación. Ese grupo discute `draft-hawkins-x402-dns-discovery-03`.

## Hecho (pedido explícito del operador, aplicado con `-target`)

`_x402.facilitator.ultravioletadao.xyz` TXT, TTL 300:

```
v=x402-1; wk=https://facilitator.ultravioletadao.xyz/.well-known/x402; k=facilitator; scheme=exact
```

En `terraform/environments/production/main.tf` (`aws_route53_record.x402_discovery`), commit
`36e75fea` en x402-rs (local, sin push). Plan dirigido: 1 to add, 0 to change, 0 to destroy.
Verificado contra los NS autoritativos de la zona.

## Lo que ustedes deberían mirar

1. **El manifiesto no tiene la forma del borrador.** El borrador exige en el JSON de
   `/.well-known/x402` los campos `x402Version`, `kind`, `facilitator.baseUrl` y
   `facilitator.endpoints`. El nuestro sirve `x402.version`, `x402.role`, `x402.facilitator`
   (string) y `x402.endpoints.*`. Un resolvedor estricto del borrador leería el TXT, llegaría
   al manifiesto y no lo parsearía. Alinear los nombres (o servir ambos) es de ustedes; el
   borrador está en revisión en el repo de la Fundación (PR 2979) y la forma puede moverse.
2. **Drift en su stack:** el plan completo de `environments/production` proponía
   `aws_lambda_function.balances` "updated in-place" ANTES de mi cambio. No lo toqué (apliqué
   sólo mi recurso). Es de ustedes decidir si aplicarlo.
