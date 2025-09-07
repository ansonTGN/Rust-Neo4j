use std::{collections::HashMap, net::SocketAddr, time::Duration};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    serve, Json, Router,
};
use axum::http::Method;
use axum_prometheus::PrometheusMetricLayer;
use color_eyre::eyre::{eyre, Report, Result};
use futures::TryStreamExt as _;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use neo4rs::{query, ConfigBuilder, Graph, Node as NeoNode};
use serde::{Deserialize, Serialize};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::{
    cors::{Any, CorsLayer},
    compression::CompressionLayer,
    limit::RequestBodyLimitLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::ServeDir,
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    trace::{DefaultMakeSpan, DefaultOnFailure, DefaultOnRequest, DefaultOnResponse, TraceLayer},
    sensitive_headers::SetSensitiveHeadersLayer,
};
use tracing::{debug, error, info, instrument, Level};
use tracing_error::ErrorLayer;
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _, EnvFilter};
use uuid::Uuid;

// --- OpenAPI / Swagger ---
use utoipa::{OpenApi, ToSchema, IntoParams};
use utoipa_swagger_ui::SwaggerUi;

// ============================
// Config
// ============================

#[derive(Debug, Clone)]
struct AppConfig {
    bind_host: [u8; 4],
    port: u16,
    neo4j_uri: String,
    neo4j_user: String,
    neo4j_password: String,
    neo4j_database: String,
    request_timeout_secs: u64,
    max_concurrency: usize,
    max_body_bytes: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            bind_host: [0, 0, 0, 0],
            neo4j_uri: std::env::var("NEO4J_URI")
                .unwrap_or_else(|_| "neo4j+s://demo.neo4jlabs.com".to_string()),
            neo4j_user: std::env::var("NEO4J_USER").unwrap_or_else(|_| "movies".to_string()),
            neo4j_password: std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "movies".to_string()),
            neo4j_database: std::env::var("NEO4J_DATABASE").unwrap_or_else(|_| "movies".to_string()),
            port: std::env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(8080),
            request_timeout_secs: std::env::var("REQUEST_TIMEOUT_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(20),
            max_concurrency: std::env::var("MAX_CONCURRENCY").ok().and_then(|s| s.parse().ok()).unwrap_or(512),
            max_body_bytes: std::env::var("MAX_BODY_BYTES").ok().and_then(|s| s.parse().ok()).unwrap_or(1_048_576),
        }
    }
}

// ============================
// Main
// ============================

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    // Provider de crypto (ring) para Rustls 0.23
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls ring provider");

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info,axum::rejection=trace".into()))
        .with(tracing_subscriber::fmt::layer())
        .with(ErrorLayer::default())
        .init();

    let cfg = AppConfig::default();

    // Prometheus (exponemos /metrics)
    let prom_handle: PrometheusHandle = PrometheusBuilder::new()
        .install_recorder()
        .expect("failed to install prometheus recorder");
    let prom_layer = PrometheusMetricLayer::new();

    let db = db(&cfg)?;
    if let Err(e) = warmup(&db).await {
        error!(error=?e, "warmup query failed");
    }

    let service = Service { db };

    let assets_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");

    // CORS
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any);

    use axum::http::header::{AUTHORIZATION, COOKIE, SET_COOKIE};

    // --- Router + Swagger UI ---
    let app = Router::new()
        .route("/", get(|| async { Redirect::temporary("/index.html") }))
        .route("/health", get(health))
        .route("/metrics", get({
            let h = prom_handle.clone();
            move || async move { h.render() }
        }))
        .route("/movie/:title", get(movie))
        .route("/movie/vote/:title", post(vote))
        .route("/search", get(search))
        .route("/graph", get(graph))
        // Swagger UI en /docs y JSON en /api-docs/openapi.json
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .fallback_service(ServeDir::new(assets_dir))
        .with_state(service)
        // middlewares
        .layer(prom_layer)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(
                    DefaultMakeSpan::new()
                        .level(Level::INFO)
                        .include_headers(false),
                )
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO))
                .on_failure(DefaultOnFailure::new().level(Level::ERROR)),
        )
        .layer(SetSensitiveHeadersLayer::new([AUTHORIZATION, COOKIE, SET_COOKIE]))
        .layer(SetRequestIdLayer::new(
            HeaderName::from_static("x-request-id"),
            MakeRequestUuid,
        ))
        .layer(PropagateRequestIdLayer::new(HeaderName::from_static("x-request-id")))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(cors)
        .layer(CompressionLayer::new())
        .layer(ConcurrencyLimitLayer::new(cfg.max_concurrency))
        .layer(RequestBodyLimitLayer::new(cfg.max_body_bytes))
        .layer(TimeoutLayer::new(Duration::from_secs(cfg.request_timeout_secs)));

    let addr = SocketAddr::from((cfg.bind_host, cfg.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("listening on {}", listener.local_addr().unwrap());

    serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

// ============================
// OpenAPI Doc
// ============================

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Movies API",
        version = "1.0.0",
        description = "Demo Axum + Neo4j con grafo y métricas"
    ),
    paths(
        health,
        movie,
        vote,
        search,
        graph
    ),
    components(
        schemas(
            Movie, MovieResult, Person, VoteResult, BrowseResponse, Node, Link, Search, Browse
        )
    ),
    tags(
        (name = "movies", description = "Operaciones sobre películas")
    )
)]
struct ApiDoc;

// ============================
// Infra
// ============================

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => {},
            _ = sigint.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    info!("shutdown signal received, stopping server...");
}

fn db(cfg: &AppConfig) -> Result<Graph> {
    let config = ConfigBuilder::new()
        .uri(&cfg.neo4j_uri)
        .user(&cfg.neo4j_user)
        .password(&cfg.neo4j_password)
        .db(cfg.neo4j_database.as_str())
        .build()?;

    Ok(Graph::connect(config)?)
}

async fn warmup(db: &Graph) -> Result<()> {
    const PING: &str = "RETURN 1 AS ok";
    let mut rows = db.execute(neo4rs::query(PING)).await?;
    let _ok: i64 = rows.single().await?.get("ok")?;
    Ok(())
}

// ============================
// Handlers
// ============================

#[utoipa::path(
    get,
    path = "/health",
    tag = "movies",
    responses(
        (status = 200, description = "Service healthy", body = String)
    )
)]
async fn health(State(service): State<Service>) -> Result<impl IntoResponse, AppError> {
    const PING: &str = "RETURN 1 AS ok";
    let mut rows = service.db.execute(neo4rs::query(PING)).await?;
    let ok: i64 = rows.single().await?.get("ok")?;
    if ok == 1 {
        Ok((StatusCode::OK, "ok"))
    } else {
        Err(AppError::new(eyre!("healthcheck failed"), StatusCode::SERVICE_UNAVAILABLE))
    }
}

#[utoipa::path(
    get,
    path = "/movie/{title}",
    tag = "movies",
    params(
        ("title" = String, Path, description = "Movie title (exact match)")
    ),
    responses(
        (status = 200, description = "Movie detail", body = Movie),
        (status = 404, description = "Movie not found")
    )
)]
async fn movie(
    Path(title): Path<String>,
    State(service): State<Service>,
) -> Result<Json<Movie>, AppError> {
    let title = sanitize_title(title)?;
    match service.movie(title).await {
        Ok(Some(movie)) => Ok(Json(movie)),
        Ok(None) => Err(AppError::new(eyre!("not found"), StatusCode::NOT_FOUND)),
        Err(e) => Err(AppError::from(e)),
    }
}

#[utoipa::path(
    post,
    path = "/movie/vote/{title}",
    tag = "movies",
    params(
        ("title" = String, Path, description = "Movie title (exact match)")
    ),
    responses(
        (status = 200, description = "Vote counter increased", body = VoteResult),
        (status = 404, description = "Movie not found")
    )
)]
async fn vote(
    Path(title): Path<String>,
    State(service): State<Service>,
) -> Result<Json<VoteResult>, AppError> {
    let title = sanitize_title(title)?;
    Ok(Json(service.vote(title).await?))
}

#[utoipa::path(
    get,
    path = "/search",
    tag = "movies",
    params(Search),
    responses(
        (status = 200, description = "Search results", body = [MovieResult])
    )
)]
async fn search(
    Query(search): Query<Search>,
    State(service): State<Service>,
) -> Result<Json<Vec<MovieResult>>, AppError> {
    Ok(Json(service.search(search).await?))
}

#[utoipa::path(
    get,
    path = "/graph",
    tag = "movies",
    params(Browse),
    responses(
        (status = 200, description = "Graph sub-sample", body = BrowseResponse)
    )
)]
async fn graph(
    Query(browse): Query<Browse>,
    State(service): State<Service>,
) -> Result<Json<BrowseResponse>, AppError> {
    Ok(Json(service.graph(browse).await?))
}

// ============================
// Service & dominio
// ============================

#[derive(Clone)]
struct Service {
    db: Graph,
}

impl Service {
    /// Devuelve Some(Movie) si existe, None si no.
    #[instrument(skip(self))]
    async fn movie(&self, title: String) -> Result<Option<Movie>> {
        const FIND_MOVIE: &str = r#"
            MATCH (movie:Movie {title:$title})
            OPTIONAL MATCH (movie)<-[r]-(person:Person)
            WITH movie.title AS title,
                 movie.tagline AS tagline,
                 movie.released AS released,
                 movie.votes AS votes,
                 collect({
                    name: person.name,
                    job: head(split(toLower(type(r)),'_')),
                    role: r.roles
                 }) AS cast
            RETURN title, tagline, released, votes, cast
            LIMIT 1
        "#;

        let mut rows = self
            .db
            .execute(neo4rs::query(FIND_MOVIE).param("title", title))
            .await?;

        if let Some(row) = rows.next().await? {
            let movie = Movie {
                released: row.get::<Option<i64>>("released")?.map(|v| v as u32),
                title: row.get::<Option<String>>("title")?,
                tagline: row.get::<Option<String>>("tagline")?,
                votes: row.get::<Option<i64>>("votes")?.map(|v| v as usize),
                cast: {
                    let cast_vals: Vec<serde_json::Value> = row.get("cast")?;
                    let mut people = Vec::with_capacity(cast_vals.len());
                    for v in cast_vals {
                        if let Some(obj) = v.as_object() {
                            let name = obj.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let job = obj.get("job").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let role = obj.get("role").and_then(|x| x.as_array()).map(|arr| {
                                arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>()
                            });
                            people.push(Person { name, job, role });
                        }
                    }
                    if people.is_empty() { None } else { Some(people) }
                },
            };
            let summary = rows.finish().await?;
            debug!(?summary, ?movie, "movie fetched");
            Ok(Some(movie))
        } else {
            Ok(None)
        }
    }

    /// Incrementa y devuelve el total de votos actual del filme.
    #[instrument(skip(self))]
    async fn vote(&self, title: String) -> Result<VoteResult> {
        const VOTE_IN_MOVIE: &str = r#"
            MATCH (movie:Movie {title:$title})
            SET movie.votes = coalesce(movie.votes, 0) + 1
            RETURN movie.votes AS votes
        "#;

        let mut rows = self
            .db
            .execute(neo4rs::query(VOTE_IN_MOVIE).param("title", title))
            .await?;

        let votes: i64 = rows.single().await?.get("votes")?;
        Ok(VoteResult { votes: votes as u64 })
    }

    /// Búsqueda con paginación básica (offset/limit)
    #[instrument(skip(self))]
    async fn search(&self, search: Search) -> Result<Vec<MovieResult>> {
        const SEARCH_MOVIES: &str = r#"
          MATCH (movie:Movie)
          WHERE toLower(movie.title) CONTAINS toLower($part)
          RETURN movie
          SKIP $offset LIMIT $limit
        "#;

        let limit = search.limit.unwrap_or(25).clamp(1, 200);
        let offset = search.offset.unwrap_or(0).max(0);

        let mut rows = self
            .db
            .execute(
                neo4rs::query(SEARCH_MOVIES)
                    .param("part", search.q)
                    .param("offset", offset as i64)
                    .param("limit", limit as i64),
            )
            .await?;

        let movies: Vec<MovieResult> = rows.into_stream_as::<MovieResult>().try_collect().await?;
        debug!(count = movies.len(), "search results");
        Ok(movies)
    }

    /// Grafo con filtros de servidor: tipos de relación, profundidad, etiquetas y año de estreno.
    #[instrument(skip(self))]
    async fn graph(&self, browse: Browse) -> Result<BrowseResponse> {
        let limit = browse.limit.unwrap_or(200).clamp(1, 1000) as i64;

        // Normaliza lista de relaciones a MAYÚSCULAS
        let rels: Vec<String> = browse
            .rel
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_uppercase())
            .collect();

        // Etiquetas de nodo
        let node_incl: Vec<String> = browse
            .node_incl
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        let node_excl: Vec<String> = browse
            .node_excl
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        // Raíz + profundidad
        let use_root = browse.root.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty());
        let depth = browse.depth.unwrap_or(0).clamp(0, 6) as i64;

        // Filtros de año
        let released_gte: Option<i64> = browse.released_gte;
        let released_lte: Option<i64> = browse.released_lte;

        // Construcción de Cypher (dos variantes) + properties()
        let cypher = if use_root.is_some() && depth >= 1 {
            r#"
                MATCH (root)
                WHERE (root:Movie AND root.title = $root)
                   OR (root:Person AND root.name  = $root)
                   OR (root:node {title:$root})
                MATCH p = (root)-[r*1..$depth]-(n)
                UNWIND relationships(p) AS relx
                WITH DISTINCT startNode(relx) AS s, endNode(relx) AS t, type(relx) AS rel
                WHERE (size($rels) = 0 OR rel IN $rels)
                  AND (size($node_incl) = 0 OR any(lbl IN labels(s) WHERE lbl IN $node_incl))
                  AND (size($node_incl) = 0 OR any(lbl IN labels(t) WHERE lbl IN $node_incl))
                  AND (size($node_excl) = 0 OR all(lbl IN labels(s) WHERE NOT lbl IN $node_excl))
                  AND (size($node_excl) = 0 OR all(lbl IN labels(t) WHERE NOT lbl IN $node_excl))
                  AND ($released_gte IS NULL OR CASE WHEN s:Movie THEN coalesce(s.released,-1) >= $released_gte ELSE true END)
                  AND ($released_gte IS NULL OR CASE WHEN t:Movie THEN coalesce(t.released,-1) >= $released_gte ELSE true END)
                  AND ($released_lte IS NULL OR CASE WHEN s:Movie THEN coalesce(s.released,999999) <= $released_lte ELSE true END)
                  AND ($released_lte IS NULL OR CASE WHEN t:Movie THEN coalesce(t.released,999999) <= $released_lte ELSE true END)
                RETURN s, t, rel, properties(s) AS sProps, properties(t) AS tProps
                LIMIT $limit
            "#
        } else {
            r#"
                MATCH (s)-[r]->(t)
                WHERE (size($rels) = 0 OR type(r) IN $rels)
                  AND (size($node_incl) = 0 OR any(lbl IN labels(s) WHERE lbl IN $node_incl))
                  AND (size($node_incl) = 0 OR any(lbl IN labels(t) WHERE lbl IN $node_incl))
                  AND (size($node_excl) = 0 OR all(lbl IN labels(s) WHERE NOT lbl IN $node_excl))
                  AND (size($node_excl) = 0 OR all(lbl IN labels(t) WHERE NOT lbl IN $node_excl))
                  AND ($released_gte IS NULL OR CASE WHEN s:Movie THEN coalesce(s.released,-1) >= $released_gte ELSE true END)
                  AND ($released_gte IS NULL OR CASE WHEN t:Movie THEN coalesce(t.released,-1) >= $released_gte ELSE true END)
                  AND ($released_lte IS NULL OR CASE WHEN s:Movie THEN coalesce(s.released,999999) <= $released_lte ELSE true END)
                  AND ($released_lte IS NULL OR CASE WHEN t:Movie THEN coalesce(t.released,999999) <= $released_lte ELSE true END)
                RETURN s, t, type(r) AS rel, properties(s) AS sProps, properties(t) AS tProps
                LIMIT $limit
            "#
        };

        let mut rows = self.db.execute(
            query(cypher)
                .param("root", use_root.unwrap_or_default())
                .param("depth", if depth >= 1 { depth } else { 1 })
                .param("rels", rels.clone())
                .param("node_incl", node_incl.clone())
                .param("node_excl", node_excl.clone())
                .param("released_gte", released_gte)
                .param("released_lte", released_lte)
                .param("limit", limit),
        ).await?;

        // Índices para arrays compactos
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut nodes: Vec<Node> = Vec::new();
        let mut links: Vec<Link> = Vec::new();

        while let Some(row) = rows.next().await? {
            let s: NeoNode = row.get("s")?;
            let t: NeoNode = row.get("t")?;
            let rel: String = row.get("rel")?;
            let s_props: serde_json::Value = row.get("sProps")?;
            let t_props: serde_json::Value = row.get("tProps")?;

            let (s_key, s_label, s_title) = extract_key_label_title(&s)?;
            let (t_key, t_label, t_title) = extract_key_label_title(&t)?;

            let s_idx = *index.entry(s_key).or_insert_with(|| {
                let idx = nodes.len();
                nodes.push(Node { title: s_title, label: s_label.to_string(), props: s_props.clone() });
                idx
            });

            let t_idx = *index.entry(t_key).or_insert_with(|| {
                let idx = nodes.len();
                nodes.push(Node { title: t_title, label: t_label.to_string(), props: t_props.clone() });
                idx
            });

            links.push(Link { source: s_idx, target: t_idx, rel });
        }

        Ok(BrowseResponse { nodes, links })
    }
}

/// Extrae clave única, etiqueta y título visible de un Neo4j Node
/// - Movie -> ( "movie::<title>", "movie", title )
/// - Person -> ( "person::<name>", "person", name )
/// - Otro   -> ( "node::<id>", "node", "#{id}" )
fn extract_key_label_title(n: &NeoNode) -> Result<(String, &'static str, String)> {
    let labels = n.labels();

    if labels.iter().any(|&l| l == "Movie") {
        let title: String = n.get("title").unwrap_or_else(|_| format!("#{}", n.id()));
        Ok((format!("movie::{}", title), "movie", title))
    } else if labels.iter().any(|&l| l == "Person") {
        let name: String = n.get("name").unwrap_or_else(|_| format!("#{}", n.id()));
        Ok((format!("person::{}", name), "person", name))
    } else {
        Ok((format!("node::{}", n.id()), "node", format!("#{}", n.id())))
    }
}

// ============================
// Tipos API (Schemas)
// ============================

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
struct Movie {
    released: Option<u32>,
    title: Option<String>,
    tagline: Option<String>,
    votes: Option<usize>,
    cast: Option<Vec<Person>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
struct MovieResult {
    movie: Movie,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
struct Person {
    job: String,
    role: Option<Vec<String>>,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
struct VoteResult {
    votes: u64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct BrowseResponse {
    nodes: Vec<Node>,
    links: Vec<Link>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct Node {
    title: String,
    label: String,
    /// Todas las propiedades del nodo (mapa JSON)
    #[schema(value_type = Object)]
    props: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct Link {
    source: usize,
    target: usize,
    rel: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, IntoParams)]
#[into_params(parameter_in = Query)]
struct Search {
    q: String,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, IntoParams)]
#[into_params(parameter_in = Query)]
struct Browse {
    limit: Option<i32>,

    /// CSV de tipos de relación (ACTED_IN,DIRECTED,...) — si vacío, todos
    #[serde(default)]
    rel: Option<String>,

    /// Nodo raíz (Movie.title o Person.name) para BFS
    #[serde(default)]
    root: Option<String>,

    /// Profundidad (saltos) cuando hay root (1..6)
    #[serde(default)]
    depth: Option<u32>,

    /// CSV de etiquetas de nodo a INCLUIR (p.ej. "Movie,Person"); si vacío, todas
    #[serde(default)]
    node_incl: Option<String>,

    /// CSV de etiquetas de nodo a EXCLUIR; si vacío, ninguna
    #[serde(default)]
    node_excl: Option<String>,

    /// Año mínimo de Movie (inclusive)
    #[serde(default)]
    released_gte: Option<i64>,

    /// Año máximo de Movie (inclusive)
    #[serde(default)]
    released_lte: Option<i64>,
}

// ============================
// Errores
// ============================

struct AppError {
    id: Uuid,
    status: StatusCode,
    inner: Report,
}

impl AppError {
    fn new(inner: Report, status: StatusCode) -> Self {
        Self { id: Uuid::new_v4(), status, inner }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let AppError { id, status, inner } = self;
        error!(error_id=%id, status=%status, error=?inner, "request failed");

        let body = serde_json::json!({
            "error": "internal_error",
            "status": status.as_u16(),
            "error_id": id.to_string(),
        });
        (status, axum::Json(body)).into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<Report>,
{
    fn from(err: E) -> Self {
        let inner = err.into();
        debug!(error=?inner, "request error");
        Self::new(inner, StatusCode::INTERNAL_SERVER_ERROR)
    }
}

// ============================
// Helpers
// ============================

fn sanitize_title(title: String) -> Result<String, AppError> {
    let t = title.trim();
    if t.is_empty() || t.len() > 200 {
        return Err(AppError::new(eyre!("invalid title"), StatusCode::BAD_REQUEST));
    }
    Ok(t.to_string())
}



