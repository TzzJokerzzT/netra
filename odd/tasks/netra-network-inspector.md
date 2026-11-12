# netra — Network Inspector TUI

Feature branch: `feat/netra-foundation`. Specification source: `../netra_network_inspector_project_prompt.md`
(referenced by the user's message, kept outside the repository as the authoritative brief).

## Specs

- **S1 (M0 Foundation).** "M0 — Foundation: Crear workspace. Configurar crates. Configurar Rust stable, Tokio,
  logging y errores. Definir entidades, enums y ports. Crear CLI inicial. Configurar CI básica. Resultado:
  proyecto compila y tiene estructura modular."
- **S2 (M1 TUI Foundation).** "M1 — TUI Foundation: Configurar Ratatui/Crossterm. Crear layout principal.
  Implementar navegación por teclado. Mostrar requests mock. Crear inspector y tabs. Resultado: TUI navegable
  con datos simulados."
- **S3 (M2 Storage Engine).** "M2 — Storage Engine: Integrar SQLite. Crear migraciones y repositorios. Persistir
  sesiones y exchanges. Persistir headers y bodies con límites. Implementar guardado y restauración. Recuperar
  sesiones interrumpidas. Resultado: historial persistente, independiente de la TUI."
- **S4 (persistencia primero).** "La persistencia será **por sesión**, con almacenamiento automático durante la
  captura y guardado explícito de la sesión como acción del usuario." — "Refrescar la pantalla, cambiar de panel,
  filtrar o limpiar la vista nunca debe borrar los registros." — "La acción de limpiar la vista solo debe limpiar
  filtros/selección/representación; nunca borrar datos persistidos." — "La captura y el almacenamiento deben
  continuar siendo correctos aunque la TUI se refresque o cambie de pantalla."
- **S5 (arquitectura).** "Adoptar **Clean Architecture + Hexagonal Architecture**, con separación estricta entre
  dominio, aplicación, adaptadores de infraestructura y presentación." — "Debe ser independiente de Ratatui,
  SQLite, Tokio, CDP, proxy y frameworks externos [el Domain Layer]." — "La TUI debe invocar estos casos de uso y
  no consultar SQLite ni comunicarse directamente con CDP/proxy."
- **S6 (seguridad por defecto).** "Redactar por defecto headers como `Authorization`, `Proxy-Authorization`,
  `Cookie` y `Set-Cookie`." — "Diferenciar entre body ausente, omitido, redactado y truncado." — "Guardar archivos
  con permisos privados." — "Escuchar en localhost por defecto."
- **S7 (CLI).** "Diseñar la CLI con Clap. Validar argumentos, presentar errores útiles y devolver códigos de salida
  apropiados. No inventar comandos que no estén implementados."
- **S8 (tests obligatorios de M0–M2).** "Implementar tests desde el comienzo." Unit tests: "Entidades y estados.",
  "Filtros.", "Normalización.", "Redacción de datos sensibles.", Repository tests: "CRUD.", "Migraciones.",
  "Transacciones.", "Integridad referencial.", "Persistencia de bodies.", "Restauración de sesiones."
- **S9 (documentación).** "`README.md`: descripción, requisitos, instalación, uso y ejemplos." — "`docs/architecture.md`:
  capas, crates, flujos y contratos." — "`docs/security.md`: modelo de amenazas, CA, TLS, redacción, proxy y Replay."
  — "`docs/roadmap.md`: milestones y estado." — "`docs/adr/`: Architecture Decision Records."
- **S10 (no fingir).** "No afirmes que una función está implementada si solo existe una maqueta o stub." — "Nunca
  simules una captura real con datos mock en una función que se presente como terminada."

## Tasks

| ID | Specs | Route | Outcome | Commit |
| --- | --- | --- | --- | --- |
| T1 | S1, S5 | inline | Workspace `netra` con 7 crates, dominio independiente de infraestructura, ports, CLI Clap, tracing, CI. | `chore: bootstrap netra workspace` |
| T2 | S5, S6, S8 | inline | Dominio: ids, value objects, entidades, headers ordenados, bodies con estado, redacción de secretos, tests. | `feat(core): domain model and redaction policy` |
| T3 | S1, S5, S8 | inline | Aplicación: casos de uso sobre ports, filtros, estado de captura, análisis básico. | `feat(application): use cases over ports` |
| T4 | S3, S4, S6, S8 | inline | Storage SQLite: migraciones versionadas, repositorios, WAL, límites, recuperación. | `feat(storage): sqlite persistence with recovery` |
| T5 | S2, S4, S6 | worker | TUI Ratatui: layout adaptable, lista, inspector con tabs, filtros, ayuda, confirmación. | `feat(tui): navigable inspector layout` |
| T6 | S7, S8 | inline | CLI real: `capture`, `sessions list/open/export/delete`, `config`; códigos de salida. | `feat(cli): sessions and capture commands` |
| T7 | S9 | inline | README, docs/architecture.md, docs/security.md, docs/roadmap.md, ADRs. | `docs: architecture, security and roadmap` |

## Log

- **L1 (user, verbatim):** "@netra_network_inspector_project_prompt.md quiero que crees este proyecto. Si hay
  alguna duda puedes preguntar" — the attached `netra_network_inspector_project_prompt.md` is the specification.
- **L2 (brief, verbatim):** "Implementa primero **M0: Foundation**. No intentes construir toda la aplicación en una
  sola operación ni generes grandes cantidades de código sin compilar y probar." — "Al completar M0, deja el
  workspace compilando, con contratos iniciales documentados, CLI mínima y tests básicos. Luego continúa con M1 y
  M2, salvo que el usuario solicite otro orden."
- **L3 (evidence).** Environment: `rustc 1.99.1` (stable), `cargo 1.99.1`, `chromium`/`brave` in `/usr/bin`,
  `sqlite3` present, Linux x86_64. No prior `netra` checkout: greenfield.
- **L4 (decision).** Crates are added only when consumed (no CDP/proxy/TLS dependencies in M0–M2); M3/M4 dependency
  choices (chromiumoxide/hudsucker/rustls/rcgen) are researched and recorded as ADRs when those milestones start.
- **L5 (decision).** Project root is `/home/alex_buelvas/Projects/Github/netra`, branch `feat/netra-foundation`.
