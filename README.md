Pruebas de concepto usando Rust, Axum, Neo4j y D3

[![Documentation](https://img.shields.io/badge/docs-latest-blue.svg)](https://ansonTGN.github.io/Rust-Neo4j/)

# Movies • Axum + Neo4j (Bolt) + D3

Demo full-stack en Rust que expone una API HTTP (Axum) contra Neo4j (driver `neo4rs`) y un frontend estático con **D3** para explorar el grafo de películas/personas.
Incluye **Prometheus metrics**, **tracing**, **CORS**, **compresión**, **timeouts**, **request ids** y **Swagger UI** generado con **`utoipa`**.

---

## ✨ Novedades (últimas mejoras)

* **Swagger/OpenAPI** con [`utoipa`](https://docs.rs/utoipa) y UI en **`/docs`** (OpenAPI JSON en **`/api-docs/openapi.json`**).
* Frontend (`assets/index.html`) renovado:

  * Visualización de **grafo interactivo** (D3) con zoom, pan, flechas y etiquetas de relación opcionales.
  * **Panel de propiedades** por nodo: muestra **todas las propiedades** (`props`) devueltas por la API, con copia a portapapeles y JSON expandible.
  * Filtros dinámicos por **tipo de relación** (checkboxes, “Todas/Ninguna”), **profundidad** desde la selección (BFS), **distancia de enlaces**, atenuación de no-vecinos y **leyendas** de colores.
  * **Atajos**: `G` refrescar grafo, `/` enfocar búsqueda, **Enter** busca, **doble clic** en película abre detalle.
  * Tema **dark/light** (Tailwind + Alpine).
* **TLS**: Rustls 0.23 con **ring provider** inicializado en `main.rs` para evitar el pánico por proveedor no seleccionado.
* **Observabilidad**:

  * **/metrics** (Prometheus) vía `axum-prometheus` + `metrics-exporter-prometheus`.
  * **TraceLayer** con `tracing` y `tracing-error`.
* **Endpoints**: `/search`, `/movie/:title`, `/movie/vote/:title`, `/graph`, `/health`, `/metrics`, `/docs`.

---

## 📦 Requisitos

* **Rust** 1.75+ (recomendado `rustup` estable)
* **Neo4j** accesible por Bolt+TLS (por defecto usa el *demo* público)

---

## ⚙️ Configuración (variables de entorno)

| Variable               | Default                        | Descripción                    |
| ---------------------- | ------------------------------ | ------------------------------ |
| `PORT`                 | `8080`                         | Puerto del servidor HTTP       |
| `NEO4J_URI`            | `neo4j+s://demo.neo4jlabs.com` | URI Bolt+TLS                   |
| `NEO4J_USER`           | `movies`                       | Usuario de Neo4j               |
| `NEO4J_PASSWORD`       | `movies`                       | Password de Neo4j              |
| `NEO4J_DATABASE`       | `movies`                       | Base de datos                  |
| `REQUEST_TIMEOUT_SECS` | `20`                           | Timeout por petición           |
| `MAX_CONCURRENCY`      | `512`                          | Límite de concurrencia (Tower) |
| `MAX_BODY_BYTES`       | `1048576`                      | Límite de tamaño de body       |

> **Nota**: CORS está abierto (`Any`) para facilitar pruebas.

---

## 🚀 Ejecución local

```bash
# 1) Clonar e instalar dependencias
cargo build --release

# 2) (opcional) Exportar configuración
export NEO4J_URI="neo4j+s://demo.neo4jlabs.com"
export NEO4J_USER="movies"
export NEO4J_PASSWORD="movies"
export NEO4J_DATABASE="movies"
export PORT=8080

# 3) Lanzar
cargo run --release
```

Abre:

* **Frontend**: [http://localhost:8080/](http://127.0.0.1:8080/index.html)
* **Swagger UI**: [http://localhost:8080/docs](http://localhost:8080/docs)
* **OpenAPI JSON**: [http://localhost:8080/api-docs/openapi.json](http://localhost:8080/api-docs/openapi.json)
* **Métricas Prometheus**: [http://localhost:8080/metrics](http://localhost:8080/metrics)
* **Healthcheck**: [http://localhost:8080/health](http://localhost:8080/health)

---

## 🧭 Endpoints principales

| Método | Ruta                        | Descripción                                 |
| -----: | --------------------------- | ------------------------------------------- |
|    GET | `/search?q=&offset=&limit=` | Búsqueda de películas por título (contains) |
|    GET | `/movie/:title`             | Detalle de película                         |
|   POST | `/movie/vote/:title`        | Incrementa el contador `votes`              |
|    GET | `/graph?limit=`…            | Muestra subgrafo con filtros (ver abajo)    |
|    GET | `/health`                   | Ping a la DB (`RETURN 1 AS ok`)             |
|    GET | `/metrics`                  | Exporter Prometheus                         |
|    GET | `/docs`                     | Swagger UI                                  |

### Parámetros `/graph` (query)

* `limit`: límite de aristas devueltas (1..1000, default 200)
* `rel`: CSV de tipos de relación (e.g. `ACTED_IN,DIRECTED`)
* `root`: nodo raíz (`Movie.title` o `Person.name`)
* `depth`: profundidad BFS (1..6) cuando hay `root`
* `node_incl`: CSV de etiquetas de nodos a **incluir** (`Movie,Person`)
* `node_excl`: CSV de etiquetas de nodos a **excluir**
* `released_gte` / `released_lte`: filtros por año en nodos `Movie`

**Respuesta**:

```jsonc
{
  "nodes":[
    { "title":"The Matrix", "label":"movie", "props": { "title":"The Matrix", "released":1999, ... } },
    { "title":"Keanu Reeves", "label":"person", "props": { "name":"Keanu Reeves", ... } }
  ],
  "links":[
    { "source":0, "target":1, "rel":"ACTED_IN" }
  ]
}
```

---

## 🖥️ Frontend (assets/index.html)

* **Stack**: Tailwind (CDN), Alpine.js, D3 v7.
* **Búsqueda** con paginación básica; detalle de película (cast agrupado).
* **Grafo**:

  * Flechas por relación, colores por **tipo de relación** y **tipo de nodo**.
  * **Etiquetas de relación** opcionales.
  * **Leyendas** de tipos y contadores por relación.
  * **Profundidad desde selección**: BFS filtrado por relaciones activas.
  * **Panel de selección** con **propiedades del nodo** (`props`) renderizadas como lista y JSON.
  * **Ajustar vista** automático y botón de *refit*.
* **Enlaces útiles** en cabecera: **Métricas** y **Docs** (Swagger UI).

---

## 🧩 Arquitectura (alto nivel)

```
Axum Router
├─ GET  /, /index.html  (ServeDir ./assets)
├─ GET  /health
├─ GET  /metrics        (Prometheus)
├─ GET  /search
├─ GET  /movie/:title
├─ POST /movie/vote/:title
├─ GET  /graph
└─ /docs + /api-docs/openapi.json (Swagger UI + OpenAPI via utoipa)

Service
└─ Graph (neo4rs)
   ├─ Cypher búsqueda/lectura
   └─ Construcción de subgrafo + props()
```

---

## 📝 Swagger/OpenAPI

* Declarado con `#[derive(OpenApi)]` y `#[utoipa::path]` en handlers.
* **UI** montada con `utoipa-swagger-ui` para Axum:

  * UI: `/docs`
  * JSON: `/api-docs/openapi.json`

**Dependencias relevantes en `Cargo.toml`:**

```toml
utoipa = "4"
utoipa-swagger-ui = { version = "7", features = ["axum"] }
```

> Si ves un error sobre `utoipa` y `feature = "macros"`, elimínala (ya no existe en v4).

---

## 🔒 TLS (Rustls provider)

Se instala el **ring provider** al inicio de `main`:

```rust
rustls::crypto::ring::default_provider()
    .install_default()
    .expect("failed to install rustls ring provider");
```

Esto evita el pánico: *“Could not automatically determine the process-level CryptoProvider…”*
Asegúrate de tener en `Cargo.toml`:

```toml
rustls = { version = "0.23", default-features = false, features = ["ring"] }
```

---

## 🧪 Pruebas rápidas (curl)

```bash
curl 'http://localhost:8080/health'

curl 'http://localhost:8080/search?q=matrix&limit=5'

curl 'http://localhost:8080/movie/The%20Matrix'

curl -X POST 'http://localhost:8080/movie/vote/The%20Matrix'

curl 'http://localhost:8080/graph?limit=200&rel=ACTED_IN,DIRECTED'
```

---

## 🐛 Troubleshooting

* **Pánico Rustls CryptoProvider**
  Ver sección TLS; asegura `features = ["ring"]` y la llamada `install_default()`.

* **No hay datos**
  Revisa credenciales/URI de Neo4j (`NEO4J_*`) o usa los valores por defecto del *demo*.

* **CORS**
  Está en `Any` para desarrollo. Ajusta `CorsLayer` si necesitas restringir orígenes.

---

## 📂 Estructura

```
assets/
  index.html           # UI (Tailwind + Alpine + D3)
src/
  main.rs              # Axum + Neo4j + Swagger + métricas
Cargo.toml
```

---

## 📜 Licencia

MIT. Si reutilizas partes, ¡agradece con una estrella ⭐!

---

## 🤝 Contribuir

PRs y issues son bienvenidos:

* Mantén el estilo y comentarios claros.
* Añade pruebas manuales (cURL) a la descripción.
* Si tocas el frontend, prueba atajos y panel de propiedades.

---

¡Listo! Abre **[http://localhost:8080/](http://localhost:8080/)**, explora el grafo y juega con los filtros.
Docs interactivas en **[http://localhost:8080/docs](http://localhost:8080/docs)**.
