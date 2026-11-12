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
- **S11 (M3 CDP).** "Conectar a Chrome/Chromium. Procesar eventos Network. Correlacionar requests y responses.
  Gestionar redirecciones y fallos. Obtener response bodies cuando sea posible. Persistir eventos.
  Resultado: captura real de Chrome." — "Conexión WebSocket a Chrome/Chromium." — "Habilitar el dominio
  `Network`." — "Procesar, al menos, `Network.requestWillBeSent`, `Network.requestWillBeSentExtraInfo`,
  `Network.responseReceived`, `Network.responseReceivedExtraInfo`, `Network.loadingFinished` y
  `Network.loadingFailed`." — "Tratar correctamente cuerpos base64 y respuestas no recuperables." —
  "Manejar desconexión y reconexión sin corromper sesiones." — "No asumir que CDP proporciona todos los timings
  o cuerpos en todos los casos."
- **S12 (dependencias verificadas).** "Utilizar versiones estables y compatibles verificadas al iniciar el
  proyecto." — "Si una librería propuesta no está mantenida, no es compatible o no ofrece la funcionalidad
  necesaria, investiga una alternativa y documenta la decisión antes de introducirla."
- **S13 (M4 proxy).** "Implementar proxy HTTP. Añadir CONNECT y TLS interception. Implementar gestión segura de
  CA. Normalizar eventos al modelo común. Añadir límites y pruebas de seguridad. Resultado: captura de
  aplicaciones compatibles con proxy." — "Escuchar en localhost por defecto." — "Permitir modo de túnel sin
  descifrado cuando corresponda." — "No instalar certificados en el sistema automáticamente sin consentimiento."
  — "Proteger la clave privada con permisos restrictivos." — "No desactivar la validación TLS del cliente de
  forma global."
- **S14 (limitaciones documentadas).** "Documentar limitaciones como certificate pinning, QUIC/HTTP/3, protocolos
  no HTTP y aplicaciones que ignoran el proxy."
- **S10 (no fingir).** "No afirmes que una función está implementada si solo existe una maqueta o stub." — "Nunca
  simules una captura real con datos mock en una función que se presente como terminada."

## Tasks

| ID | Specs | Route | Outcome | Commit |
| --- | --- | --- | --- | --- |
| T1 | S1, S5 | inline | Workspace `netra` con 7 crates, CI, fmt/clippy, ODD document. | `chore: bootstrap netra workspace` |
| T2 | S5, S6, S8 | inline | Dominio: ids, value objects, headers ordenados, bodies con estado, redacción, ports, config. | `feat(core): domain model, ports and redaction policy` |
| T3 | S5, S8 | inline | Delta de actualizaciones incrementales, kinds estables, totales de timing. | `feat(core): exchange updates, media kinds and timing totals` |
| T4 | S4, S6, S8 | inline | Procesador de correlación con límites, redacción, huérfanos y contadores de descarte. | `feat(capture): correlate provider events into incremental updates` |
| T5 | S2, S6 | inline | Motor de análisis: body view, JSON, timing, headers, errores, estadísticas. | `feat(analysis): body, timing, header and error inspection` |
| T6 | S3, S4, S6, S8 | inline | Storage SQLite: migraciones, WAL, permisos privados, apply incremental, retención, recuperación. | `feat(storage): sqlite persistence with crash recovery` |
| T7 | S3, S4, S5, S8 | inline | Casos de uso, query service paginado, runtime de captura con canales acotados, export HAR. | `feat(application): use cases and capture runtime` |
| T8 | S2, S4, S6 | worker (`gentle-ai-worker`) | TUI Ratatui: layout adaptable, lista, inspector de 7 tabs, filtros, ayuda, confirmación. | `feat(tui): navigable inspector with adaptive layout` |
| T9 | S7, S8, S10 | inline | CLI real: capture/sessions/config, códigos de salida, TUI host, demo etiquetada. | `feat(cli): capture, sessions, export and the TUI host` |
| T10 | S9 | inline | README, architecture, security, roadmap y 5 ADRs. | `docs: architecture, security model, roadmap and ADRs` |
| T11 | S11, S12 | inline | ADR 0006: cliente CDP propio sobre tokio-tungstenite, alternativas evaluadas con datos de crates.io. | `docs(adr): choose an in-tree CDP client` |
| T12 | S11, S12, S8 | inline | `netra-capture/cdp`: protocolo, discovery, mapeo puro de eventos Network y provider con sesiones flat, auto-attach y bodies. | `feat(capture): CDP provider for Chrome/Chromium` |
| T13 | S11, S12 | inline | CLI `--cdp-endpoint`, `--source cdp` real; el proxy sigue nombrando M4. Fallo rápido si el proveedor no arranca. | `feat(cli): live CDP capture wiring` |
| T14 | S11, S8 | inline | Test de integración contra Chromium real con fixture local; regresiones del traductor y de bodies. | incluido en T12 |
| T15 | S13, S14 | inline | ADR 0007: proxy HTTP/1.1 propio, autoridad local explícita y límites compartidos. | `docs(adr): choose a hand-rolled proxy` |
| T16 | S13, S6, S8 | inline | `netra-capture/proxy`: parser HTTP/1.1, CA con hojas por host, CONNECT en túnel e interceptación TLS, límites y timeouts. | `feat(capture): HTTP/HTTPS proxy with local authority` |
| T17 | S13, S6, S8 | inline | `netra ca init|export|show` y `--source proxy`; captura de ambos orígenes a la vez. | `feat(cli): proxy capture and CA commands` |
| T18 | S6, S8 | inline | Fuga encontrada en smoke real: `Referer` y URLs dentro de JSON conservaban `?token=`. Máscara de query en cualquier URL almacenada. | `fix(security): mask query values in referer headers and JSON urls` |

Naming note: the original plan had 7 tasks; the delta/broadcast split made T3–T7 explicit so every commit is a
single reviewable work unit.

## Log

- **L1 (user, verbatim):** "@netra_network_inspector_project_prompt.md quiero que crees este proyecto. Si hay
  alguna duda puedes preguntar" — the attached `netra_network_inspector_project_prompt.md` is the specification.
- **L2 (brief, verbatim):** "Implementa primero **M0: Foundation**. No intentes construir toda la aplicación en una
  sola operación ni generes grandes cantidades de código sin compilar y probar." — "Al completar M0, deja el
  workspace compilando, con contratos iniciales documentados, CLI mínima y tests básicos. Luego continúa con M1 y
  M2, salvo que el usuario solicite otro orden."
- **L3 (evidence).** Environment: `rustc 1.99.1` (stable), `cargo 1.99.1`, `chromium`/`brave` in `/usr/bin`,
  `sqlite3` present, Linux x86_64. No prior `netra` checkout: greenfield.
- **L4 (decision).** Crates are added only when consumed. CDP, proxy and TLS dependencies (chromiumoxide or
  alternative, hudsucker or alternative, rustls, rcgen) are deferred to M3/M4 and get their own ADR with a
  maintenance check at that point.
- **L5 (decision).** Project root is `/home/alex_buelvas/Projects/Github/netra`, branch `feat/netra-foundation`.
- **L6 (decision).** `cargo capture --source cdp|proxy` without `--demo` exits with code 3 and names the missing
  milestone. M3/M4 are not implemented, and S10 forbids pretending otherwise; the demo provider is the only source
  that exists and it is labelled synthetic in code, docs and CLI help.
- **L7 (decision).** Structured payloads (JSON/XML) above `limits.max_body_bytes` are persisted as *omitted*
  instead of truncated: a truncated JSON document cannot be redacted by the JSON field rules, so truncating it
  would be a credential-leak path. Text and binary payloads are still truncated and flagged. Recorded in ADR 0004
  and `docs/security.md`.
- **L8 (evidence, green).** `cargo test --workspace`: 210 tests pass. `cargo clippy --workspace --all-targets`:
  no warnings. `cargo fmt --all --check`: clean. Release binary smoke-tested with `script -qec`: `capture --demo`
  persists 5 exchanges (1 failed) and the TUI opens and closes on `q`.
- **L9 (decision).** The TUI does not start capture: `netra capture` records, `netra` reads. This keeps a single
  writer per session, lets WAL readers refresh freely, and satisfies S4 without inventing a second source of truth.
- **L10 (independent verification, findings).** `gentle-ai-verify` confirmed both high-risk claims for the
  demo path (no credential reaches the database, the WAL or the HAR; incremental writes survive `kill -9` with
  `integrity_check = ok` and no orphan rows) and refuted five defects. All five were fixed and pinned with tests:
  `D1` unparsable-URL redaction bypass (`redact_url` now masks query values textually), `D2` configured redaction
  rules replaced the built-in carriers (`merged_with_defaults`), `D3` startup recovery stopped a *live* session
  (ownership by pid: migration `M2_OWNER_PID`, `SessionRepository::set_owner`, probe-aware recovery),
  `D4` permissions applied after creation (`0600`/`0700` applied at creation, sidecars tightened),
  `D5` configured `data_dir` was silently ignored (honoured, relative paths resolved against the config file,
  never written back), `D6` HAR exports were world-readable (`0600` via `OpenOptions::mode`).
  Advisory limits kept and documented: non-JSON bodies, custom header names, malformed JSON, and truncation of
  non-structured payloads.
- **L11 (evidence, green after corrections).** `cargo test --workspace`: **228 tests pass** (was 210; the
  verifier's probes became regression tests). `cargo clippy --workspace --all-targets`: no warnings.
  `cargo fmt --all --check`: clean.
- **L12 (provider stop).** The native RDD review of the storage work unit was refused with
  `lens_context_budget_exceeded` (~1900 authored lines with tests): no review authority was created, and the
  provider requires splitting the candidate into chained smaller commits before reviewing.
- **L14 (user, verbatim).** "Si no esta en el documento, usa un CDP adecuado para el proyecto y un ADR"
- **L15 (research, crates.io 2026-11-12).** `chromiumoxide` 0.9.1 (actualizado 2026-02-25, 5.7M descargas,
  embebe el protocolo generado completo), `headless_chrome` 1.0.22 (2026-06-11, API síncrona), `browser-protocol`
  0.1.6 (2026-10-03, 583 descargas), `tokio-tungstenite` 0.30.0 y `serde_json` como base. Decisión: cliente CDP
  propio en `netra-capture/cdp` (ADR 0006), no por moda sino porque el adaptador solo necesita cuatro métodos y
  la traducción a `CaptureEvent` ya existe.
- **L16 (evidence, CDP end-to-end).** Chromium 155 headless + fixture local: `netra capture --source cdp
  --cdp-endpoint 127.0.0.1:9499` persistió 18 exchanges con cuerpos y timings; el query `?token=leaks` quedó como
  `token=%5BREDACTED%5D` en la base. Test de integración `cdp_integration` (se saltea si no hay binario y usa
  `NETRA_CHROMIUM` para fijarlo). Total del workspace: 258 tests.
- **L17 (defect found during M3).** Una captura cuyo proveedor no arranca salía con código 0 y una sesión vacía.
  Ahora el runtime llama `provider.start()` antes de lanzar tareas y falla cerrado (código 1), borrando la sesión
  creada.
- **L18 (user, verbatim).** "Comienza con el M4, realiza la verficación y de la publicación me encargo yo"
- **L19 (decision).** M4 implementado con cliente/proxy propio (ADR 0007) en lugar de `hudsucker`: el modelo de
  eventos, los límites y la redacción ya viven en el pipeline, y el proxy solo necesita línea de request, headers,
  `Content-Length` y chunked. Un exchange por conexión (`Connection: close`) para que el framing no sea ambiguo.
- **L20 (security finding, fixed).** El smoke test con Chromium real a través del proxy mostró el token en dos
  lugares después de la redacción: el header `Referer` y una URL dentro del cuerpo JSON de la respuesta. Se extendió
  la política: toda URL almacenada (request line, headers con URL, strings JSON con `http(s)://` o `/ruta?`) pierde
  sus query params sensibles. Un test del pipeline cubre el caso; el smoke volvió a 0 coincidencias del token.
- **L21 (evidence, M4).** Chromium headless con `--proxy-server` y `--proxy-bypass-list='<-loopback>'`: 15–17
  exchanges persistidos por el proxy, incluida la petición al fixture; tests `proxy_integration` cubren HTTP,
  CONNECT sin descifrado, interceptación TLS contra un origen TLS local, límites y el opt-in de loopback.
  Workspace: 288 tests.
- **L13 (user decision, verbatim).** "No lo haga manda el commit con las 1900 lineas." — the storage work unit
  stays as one commit; it is not split. Consequence, recorded so it is not mistaken for an oversight: that
  candidate cannot pass the native reviewer budget, so it carries no native review; its risk is covered by the
  independent verification run (L10/L11) plus 228 tests. Any future native review of storage must start from a
  new, smaller candidate.
