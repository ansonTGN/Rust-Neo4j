Pruebas para aprendizaje con interaccion Rust y Neo4j con Axum y D3

# Movies • Neo4j Demo (Axum + Neo4j + D3)

Aplicación full-stack en **Rust** que expone una API HTTP con **Axum** y visualiza un grafo de películas y personas desde **Neo4j**.
Incluye **métricas Prometheus**, **healthcheck**, UI estática con **Tailwind + Alpine**, y grafo interactivo con **D3** (con propiedades de cada nodo).

---

## 🚀 Stack

* **Backend**: \[Axum 0.7], \[tokio], \[tower-http], \[tracing], \[color-eyre]
* **DB**: \[neo4rs] (Bolt/TLS)
* **Métricas**: \[metrics-exporter-prometheus], \[axum-prometheus]
* **Frontend**: HTML estático (CDN) con **TailwindCSS**, **Alpine.js**, **D3.js**
* **TLS**: rustls 0.23 con **ring** (provider instalado en `main.rs`)

> Por defecto se conecta a `neo4j+s://demo.neo4jlabs.com` (DB pública de ejemplo “movies”).

---

## 📦 Requisitos

* **Rust** estable (Edition 2021).
  Instala con `rustup` si no lo tienes.
* **No necesitas Node** ni toolchain frontend: el HTML usa CDN.
* (Opcional) **Neo4j** propio. Si no configuras nada, la app usa el demo público.

---

## ⚙️ Configuración

Variables de entorno (todas tienen default razonable):

| Variable               | Default                        | Descripción                        |
| ---------------------- | ------------------------------ | ---------------------------------- |
| `NEO4J_URI`            | `neo4j+s://demo.neo4jlabs.com` | URI Bolt (con o sin TLS)           |
| `NEO4J_USER`           | `movies`                       | Usuario Neo4j                      |
| `NEO4J_PASSWORD`       | `movies`                       | Password Neo4j                     |
| `NEO4J_DATABASE`       | `movies`                       | Base de datos                      |
| `PORT`                 | `8080`                         | Puerto de escucha                  |
| `REQUEST_TIMEOUT_SECS` | `20`                           | Timeout de solicitud               |
| `MAX_CONCURRENCY`      | `512`                          | Límite server-side de concurrencia |
| `MAX_BODY_BYTES`       | `1048576`                      | Límite de tamaño de body           |

Ejemplo `.env`:

```bash
NEO4J_URI=neo4j+s://demo.neo4jlabs.com
NEO4J_USER=movies
NEO4J_PASSWORD=movies
NEO4J_DATABASE=movies
PORT=8080
```

> Si usas Neo4j local sin TLS, puedes usar `NEO4J_URI=bolt://localhost:7687`.

---

## ▶️ Ejecutar

```bash
# Compilar en release y ejecutar
cargo run --release
```

Salida esperada:

```
listening on 0.0.0.0:8080
```

Abre el navegador en: **[http://localhost:8080/](http://localhost:8080/)**

> El provider de crypto **ring** para `rustls` se **instala al inicio** del `main()` (Opción A). Esto evita el pánico de “Could not automatically determine the process-level CryptoProvider”.

---

## 🧭 Endpoints

* `GET /` → redirige a `/index.html` (UI)
* `GET /health` → Healthcheck (`ok` cuando todo va bien)
* `GET /metrics` → Métricas Prometheus (scrapeable)
* `GET /search?q=&offset=&limit=` → Búsqueda por título (paginada)
* `GET /movie/:title` → Detalle de película (título exacto)
* `POST /movie/vote/:title` → Incrementa votos de la película
* `GET /graph?…` → Subgrafo con filtros (ver parámetros)

### Parámetros de `/graph`

| Parámetro      | Tipo       | Ejemplo                    | Descripción                                    |
| -------------- | ---------- | -------------------------- | ---------------------------------------------- |
| `limit`        | number     | `200`                      | Máx. relaciones devueltas (1..1000)            |
| `rel`          | CSV string | `ACTED_IN,DIRECTED`        | Filtro por tipos de relación (si vacío, todas) |
| `root`         | string     | `Tom Hanks` \| `Apollo 13` | Nodo raíz (Person.name o Movie.title)          |
| `depth`        | number     | `2`                        | Profundidad desde `root` (1..6)                |
| `node_incl`    | CSV string | `Movie,Person`             | Solo etiquetas incluidas                       |
| `node_excl`    | CSV string | `User,Company`             | Excluir etiquetas                              |
| `released_gte` | number     | `1990`                     | Año de película (mínimo, inclusive)            |
| `released_lte` | number     | `2005`                     | Año de película (máximo, inclusive)            |

**Respuesta**:

```json
{
  "nodes": [
    { "title": "Apollo 13", "label": "movie", "props": { "title": "Apollo 13", "released": 1995, "tagline": "...", "votes": 123 } },
    { "title": "Tom Hanks", "label": "person", "props": { "name": "Tom Hanks" } }
  ],
  "links": [
    { "source": 0, "target": 1, "rel": "ACTED_IN" }
  ]
}
```

> Cada **nodo** incluye `props` con **todas las propiedades** devueltas por Neo4j.

### Ejemplos `curl`

```bash
# Health
curl -s http://localhost:8080/health

# Búsqueda
curl -s "http://localhost:8080/search?q=matrix&offset=0&limit=10" | jq .

# Detalle
curl -s "http://localhost:8080/movie/The%20Matrix" | jq .

# Voto
curl -X POST -s "http://localhost:8080/movie/The%20Matrix/vote" | jq .

# Grafo (subgrafo desde persona, 2 saltos)
curl -s "http://localhost:8080/graph?root=Tom%20Hanks&depth=2&rel=ACTED_IN,DIRECTED&node_incl=Movie,Person&released_gte=1990" | jq .
```

---

## 🖥️ UI (assets/index.html)

* **Búsqueda** de películas (lista lateral).
* **Detalle** de película (título, año, tagline, reparto, botón de votos).
* **Grafo** interactivo:

  * Colores por tipo de nodo y relación, flechas y etiquetas.
  * Filtros por relación, **profundidad desde selección**, contador de nodos/enlaces.
  * **Panel de selección con propiedades completas del nodo (`props`)**.
  * Atajos:

    * `G` refrescar grafo
    * `/` enfocar búsqueda
    * `Enter` ejecutar búsqueda
    * *Doble clic* en película abre el detalle
* **Métricas** en `/metrics` y **estado** del servidor en el header.

---

## 🧱 Middlewares & Hardening

* **CORS** (GET/POST), **Compression**, **Timeout**, **Request Body Limit**
* **Concurrency limit** global (Tower)
* **Trace** de requests con `tracing`
* **Request ID** y **Sensitive Headers**
* **Headers** de seguridad básicos:

  * `x-content-type-options: nosniff`
  * `x-frame-options: DENY`
  * `referrer-policy: no-referrer`

---

## 🧩 Estructura

```
.
├─ assets/
│  └─ index.html           # UI (Tailwind + Alpine + D3)
├─ src/
│  └─ main.rs              # Axum + Neo4j + métricas
├─ Cargo.toml
└─ README.md
```

---

## 🐛 Troubleshooting

* **Pánico de Rustls “CryptoProvider”**
  El código ya instala el provider `ring` al inicio del `main()`. Si cambias a `aws-lc-rs`, recuerda actualizar:

  ```rust
  rustls::crypto::aws_lc_rs::default_provider().install_default()?;
  ```

* **Conexión Neo4j falla**
  Verifica `NEO4J_URI/USER/PASSWORD/DATABASE`. Si es local sin TLS: `bolt://localhost:7687`.

* **No se ve el grafo**
  Mira `/health`. Si está ok, abre DevTools → Network para comprobar `/graph`.
  Verifica que hay datos en la DB o ajusta filtros (rel, node\_incl, depth…).

---

## 🤝 Contribuir

Issues y PRs son bienvenidos. Estilo Rust estándar (`rustfmt`).
Intenta mantener el frontend sin build steps (CDN) para facilitar la ejecución.

---

## 📜 Licencia

MIT © 2025 — Tú decides. Puedes cambiarla si prefieres otra.

---

## ✨ Créditos

* Dataset de ejemplo **movies** por Neo4j Labs.
* Este proyecto es educativo: muestra Axum + Neo4j + D3 con métricas y un frontend mínimo.
