# .githooks

Hooks trackeados del repo. Git no los usa solo: hay que activarlos **una vez por clon**:

```bash
git config core.hooksPath .githooks
```

Verificar: `git config core.hooksPath` imprime `.githooks`.

## pre-commit

Bloquea el commit si alguna **línea agregada** del diff staged contiene:

| Regla | Patrón | Qué protege |
|-------|--------|-------------|
| `hex64` | `0x` + 64 hex | private keys (INC-2026-03-30). También pega en tx hashes: citar URL del explorer o un prefijo, no el hash completo |
| `logptr` | `.log:` + dígitos | punteros a líneas de logs crudos de chat |
| `query` | línea que empieza por `query:` + letra | volcados crudos de consultas |
| `local-list` | strings fijos de `.githooks/pre-commit.local` | rutas del corpus, discos, handles — **solo en el archivo local, nunca en el repo** |

El hook trackeado lleva **solo patrones genéricos**. La lista sensible vive en `.githooks/pre-commit.local` (gitignored; formato en `pre-commit.local.example`) y se carga si existe. Nada de lo que imprime el hook incluye el texto que pegó ni una entrada de la lista: solo `archivo:línea [regla]`.

Bypass: `git commit --no-verify`. No lo hagas; si el hook se equivoca (falso positivo en un tx hash), reescribe la línea.

Requiere `awk` con intervalos `{n}` (gawk de Git for Windows lo trae; el hook falla cerrado si no).

### Conteos de redes publicados

Después del escaneo, si el commit toca un archivo que `scripts/network_copy.py` cubre (`COVERED`: README, `CLAUDE.md`,
la guía de redes, los docs vivos, la landing, `/docs`...) o una de sus entradas (`INPUTS`: el manifiesto, el registro de
tokens, la Terraform de producción...), el hook corre `scripts/network_copy.py --staged`: el check entero sobre **lo que
se commitea** (el índice, exportado a un directorio temporal), no sobre el árbol de trabajo. Bloquea si un conteo o una
lista de redes no sale de `config/network_capabilities.json`; `python3 scripts/network_copy.py --write` arregla casi todo.
Corre también en un commit sin líneas agregadas (uno que solo borra).

Existe porque un cambio de solo docs no dispara ningún CI (regla del dueño, 2026-09-11): sin el hook, un número tipeado
en el README o en `CLAUDE.md` lo atrapa recién la siguiente corrida del CI que compila. Sin un Python 3 en el `PATH`
(`python3` o `python`) el paso se salta con un aviso.
