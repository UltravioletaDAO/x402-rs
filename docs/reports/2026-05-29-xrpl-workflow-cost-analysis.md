# Análisis de Costos: Workflow `xrpl-native-integration`

> **Fecha:** 29 de mayo de 2026
> **Tipo:** Análisis ejecutivo / simulación de costos (Project Management)
> **Contexto:** Estimación de "¿qué hubiera costado este workflow con un equipo humano tradicional?"
> **Disclaimer:** Cifras de salarios y costos son estimaciones de mercado 2026 con fines ilustrativos. El costo de cómputo de IA es aproximado; el foco está en el **orden de magnitud** del contraste.

---

## 1. Resumen ejecutivo

El workflow `xrpl-native-integration` ejecutó **12 agentes de IA (Opus 4.8, 1M de contexto)** que eliminaron el stub roto de XRPL-EVM y construyeron la integración **nativa de XRPL (`xrpl:0`)** del facilitador x402 para **XRP, RLUSD y USDC**, modelada sobre el patrón de Stellar + el esquema de pago presignado **t54**.

| Métrica | Valor |
|---|---:|
| Tokens procesados | **~1.074 millones** |
| Acciones ejecutadas (tool calls) | **365** |
| Agentes | **12 / 12 completados** |
| Tiempo de reloj | **33 min 41 s** |
| Tiempo de cómputo sumado | **~44 min 47 s** |

**Veredicto del análisis:** un equipo humano equivalente habría costado **~$50,000 USD (~$205M COP)** y tomado **~2 semanas calendario**. El workflow lo entregó en **34 minutos** por el costo de cómputo de unos pocos dólares.

---

## 2. Los 12 agentes (datos crudos del workflow)

| # | Fase | Agente | Tokens | Acciones | Tiempo |
|---|------|--------|-------:|------:|-------:|
| 1 | Research | live-code-map | 130.4k | 13 | 2m31s |
| 2 | Research | xrpl-rust-api | 71.8k | 23 | 3m15s |
| 3 | Research | t54-wire-format | 41.5k | 13 | 2m08s |
| 4 | Research | asset-facts | 46.7k | 22 | 2m23s |
| 5 | Synthesize | implementation-brief | 65.0k | 5 | 3m13s |
| 6 | Implement | remove-xrpl-evm-stub | 59.1k | 36 | 1m56s |
| 7 | Implement | **core-scaffold** | **188.3k** | **91** | 9m19s |
| 8 | Implement | chain-xrpl-rs | 169.5k | 53 | 7m49s |
| 9 | Peripheral | config+lambda | 70.6k | 12 | 1m56s |
| 10 | Peripheral | frontend | 77.4k | 37 | 3m29s |
| 11 | Peripheral | readme+plan | 54.3k | 19 | 2m07s |
| 12 | Review | diff-review | 99.7k | 41 | 4m41s |
| | | **TOTAL** | **~1,074.3k** | **365** | reloj: 33m41s |

---

## 3. Organigrama: los cargos equivalentes

| # | Agente | Cargo equivalente | Seniority |
|---|--------|-------------------|-----------|
| 1 | live-code-map | Staff Engineer / Tech Lead de Arquitectura | Staff |
| 2 | xrpl-rust-api | Senior Blockchain Engineer (especialista XRPL) | Senior |
| 3 | t54-wire-format | Protocol & Cryptography Engineer | Senior |
| 4 | asset-facts | Blockchain Research Analyst | Mid |
| 5 | implementation-brief | Solutions Architect / Technical PM | Senior |
| 6 | remove-xrpl-evm-stub | Software Engineer (Refactoring) | Mid |
| 7 | core-scaffold | Staff Backend Engineer (Rust) | Staff |
| 8 | chain-xrpl-rs | Senior Blockchain Protocol Engineer (Rust) | Senior |
| 9 | config+lambda | DevOps / Cloud Engineer (AWS) | Senior |
| 10 | frontend | Frontend Engineer | Mid |
| 11 | readme+plan | Technical Writer / DevRel | Mid |
| 12 | diff-review | Principal Engineer / QA Lead | Principal |
| — | (orquestador) | Engineering Manager / PM | Manager |

> Armar este equipo (3 Staff/Principal, 5 Senior, 4 Mid) toma **3–6 meses de reclutamiento** en la vida real.

---

## 4. Nómina y costeo (mercado 2026)

| Agente | Salario anual | Tarifa/h | Horas humanas | Costo |
|--------|--------------:|---------:|--------------:|------:|
| live-code-map | $280k | $135 | 12 h | $1,620 |
| xrpl-rust-api | $240k | $115 | 16 h | $1,840 |
| t54-wire-format | $260k | $125 | 12 h | $1,500 |
| asset-facts | $160k | $77 | 8 h | $616 |
| implementation-brief | $230k | $111 | 8 h | $888 |
| remove-xrpl-evm-stub | $150k | $72 | 4 h | $288 |
| core-scaffold | $270k | $130 | 36 h | $4,680 |
| chain-xrpl-rs | $255k | $123 | 32 h | $3,936 |
| config+lambda | $200k | $96 | 8 h | $768 |
| frontend | $170k | $82 | 12 h | $984 |
| readme+plan | $130k | $63 | 8 h | $504 |
| diff-review | $300k | $144 | 12 h | $1,728 |
| **SUBTOTAL** | | | **168 h-persona** | **$19,352** |

**168 horas-persona = ~21 días laborales = ~1 mes** de trabajo de un ingeniero (solo código productivo).

---

## 5. Cronograma humano (camino crítico)

Las fases tienen dependencias (no se implementa sin plan):

```
SEMANA 1
  Lun-Mar  FASE 1 Research (4 paralelo) ......... 2 días   (cuello: xrpl-rust-api 16h)
  Mié      FASE 2 Synthesize (1) ................ 1 día
  Jue-Vie  FASE 3 Implement arranca...
SEMANA 2
  Lun-Mié  FASE 3 Implement (3 paralelo) ........ 4.5 días (cuello: core-scaffold 36h)
  Jue      FASE 4 Peripheral (3 paralelo) ....... 1.5 días
  Vie      FASE 5 Review (1) .................... 1.5 días

CAMINO CRÍTICO: ~10.5 días hábiles = ~2 SEMANAS CALENDARIO
```

**La IA lo hizo en 33m41s → ~142x más rápido** en tiempo de oficina.

---

## 6. El overhead invisible (coordinación)

Con 12 personas, el costo de coordinación explota (**66 canales de comunicación** posibles — Ley de Brooks):

| Ceremonia | Cálculo | Horas-persona |
|---|---|---:|
| Sprint planning | 2h × 13 | 26 h |
| Daily standups | 15min × 13 × 10 días | 32.5 h |
| Architecture review | 2h × 8 | 16 h |
| Code reviews síncronos | varias sesiones | 15 h |
| Retrospectiva | 1.5h × 13 | 19.5 h |
| Comunicación asíncrona | ~8% | 13.4 h |
| **TOTAL** | | **~122 h-persona** |

El overhead (122h) es casi tan grande como el trabajo productivo (168h). **Los agentes tuvieron 0 reuniones.**

---

## 7. Presupuesto total consolidado

| Concepto | Costo |
|---|---:|
| Trabajo productivo (168h) | $19,352 |
| Overhead de coordinación (122h) | $12,974 |
| Gestión del PM (40h) | $4,800 |
| **Subtotal directo** | **$37,126** |
| + Cargas sociales y beneficios (×1.3) | +$11,138 |
| **COSTO FULLY-LOADED** | **~$48,000 – $55,000 USD** |

### Aterrizado en pesos colombianos (USD/COP ~$4,100)

- **~$205 millones COP**
- ≈ **128 meses** de salario mínimo colombiano 2026 (~$1.6M/mes) = **~11 años**
- ≈ el sueldo anual completo de **~10 colombianos**

---

## 8. Contraste IA vs humano

| | Equipo humano | 12 agentes IA |
|---|---|---|
| Costo | ~$50,000 USD | ~$30–60 cómputo (o incluido en suscripción) |
| Tiempo calendario | ~2 semanas | 34 minutos |
| Reclutamiento | 3–6 meses | 0 segundos |
| Disponibilidad | 8am–5pm, L–V | 24/7/365 |
| Bajas / vacaciones | Sí | Nunca |
| Reuniones | 122 horas | 0 |

> **Ahorro en costo: ~1,000x. Ahorro en tiempo: ~142x.** Cambio de categoría, no mejora incremental.

---

## 9. Métricas extra (perspectiva PM)

1. **Throughput de talento:** 168 h-persona entregadas en 0.56h reales → **~300 horas de trabajo humano por cada hora de reloj.**
2. **Densidad de procesamiento:** ~531 tokens/seg agregados → **~76x** la velocidad de lectura humana, escribiendo código en simultáneo.
3. **Decisiones por minuto:** 365 acciones / 34 min = **10.8 decisiones técnicas/min**, sin fatiga.
4. **Cero bus factor:** sin punto único de falla.
5. **Rework incluido gratis:** auditoría completa (agente #12). En humanos el rework es 20–40% del esfuerzo.
6. **Time-to-market:** llegar 2 semanas antes en cripto puede valer más que el ahorro mismo.
7. **Costo por hora-persona entregada:** ~$298/h de valor humano, a un costo de centavos.

---

## 10. Analogías

- **Como construir una casa:** cimientos (core-scaffold), estructura (chain-xrpl-rs), instalaciones (config+lambda), acabados (frontend), planos (docs), inspección (diff-review) — completa en lo que dura un partido de fútbol.
- **Un episodio y medio de serie** = un mes de trabajo de ingeniería de 12 personas.
- **El ahorro (~$50k)** = un carro 0km, u 11 años de salario mínimo colombiano.
- **Leyeron una Biblia entera** (~800,000 palabras) de código y la entendieron antes de que se enfríe el café.

---

## 11. Veredicto del PM

Como Project Manager, habría presentado un presupuesto de **~$50,000 USD (~$205M COP)** y un cronograma de **2 semanas** con 12 especialistas difíciles de conseguir. Se entregó en **34 minutos** por el costo de un almuerzo, con auditoría incluida.

**PERO:** esto es un **borrador de primera calidad, no producción aprobada.** Falta lo que ningún agente reemplaza por sí solo: **revisar el diff, compilar, probar y decidir el deploy.** La IA hizo el trabajo de 12 personas; la firma del release sigue siendo del dueño.

---

*Generado por Claude Code (Opus 4.8, 1M context) como análisis ejecutivo. Cifras estimadas con fines ilustrativos.*
