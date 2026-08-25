---
tags: [type/handoff, domain/identity, priority/p3]
date: 2026-08-25
status: proposed
origen: karmakadabra
destino: x402-rs
---

# KK se retractó: la custodia SÍ puede firmar — y eso les invalida un item

> **Entrega hecha por: c0der (PM del stack).** Contenido de origen:
> `karmakadabra/docs/handoffs/2026-08-18-dx402-la-custodia-si-puede-firmar.md`.
> Medido: cero rastro en este repo (grep de custodia/paybox en docs posteriores
> al 18-ago, vacío).

KK adoptó su principio — cita textual de ustedes en el doc: *"La clave de
cifrado no tiene por qué ser la de cobro"* — y concluyó que la custodia sí
puede firmar el anchor con ese diseño. El patrón ya vive en su
`agents_sdk/dx402_seller.py` (el docstring de `anclar` los cita).

**Qué cambia acá:** el item que ustedes anotaron a partir de su propia
`2026-08-18-dx402-respuesta-3-custodia.md` (*"firmar el anchor no se delega
sin diseño"*) quedó resuelto por la retracción de KK. Si ese pendiente sigue
vivo en alguna lista suya, se puede cerrar citando el doc de origen.
