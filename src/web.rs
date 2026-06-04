use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::analyze::{self, validate_export_bundle};
use crate::collector::CollectorKind;
use crate::error::{Error, Result};
use crate::hardware::capabilities::CapabilitiesReport;
use crate::model::{
    AnalysisOptions, AnalysisReport, ContextSeries, ContextSourceStatus, ContextTransition,
    ExportBundle, FlagCatalogEntry, FlagSeries, FlagTransition, JournalEvent, MetricSeries,
    SystemInfo, TimeBounds, WindowSummary,
};
use crate::store::Store;
use crate::system_info;
use crate::throttle;
use crate::version;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub capabilities: CapabilitiesReport,
}

#[derive(Debug, Deserialize)]
pub struct RangeQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub device_pci: Option<String>,
    pub limit: Option<i64>,
    pub flag_name: Option<String>,
    pub direction: Option<String>,
    pub source_id: Option<String>,
    pub key: Option<String>,
    pub max_points: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub device_pci: Option<String>,
    #[allow(dead_code)]
    pub format: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct JournalBatchQuery {
    pub ids: String,
}

#[derive(Debug, Deserialize)]
pub struct AnalyzeExportBody {
    pub bundle: ExportBundle,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    pub device_pci: Option<String>,
    pub issue_tag: Option<String>,
}

#[derive(Debug, Serialize)]
struct AnalyzeExportResponse {
    report: AnalysisReport,
    ai_prompt: String,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    git_hash: &'static str,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/system", get(system_page))
        .route("/help", get(help_page))
        .route("/analyze", get(analyze_page))
        .route("/app.css", get(app_css))
        .route("/datasource.js", get(datasource_js))
        .route("/api/findings", get(findings))
        .route("/api/analyze/export", post(analyze_export))
        .route("/api/health", get(health))
        .route("/api/capabilities", get(capabilities))
        .route("/api/time-bounds", get(time_bounds))
        .route("/api/system", get(system_info))
        .route("/api/system/refresh", post(refresh_system_info))
        .route("/api/summary", get(window_summary))
        .route("/api/export", get(export_data))
        .route("/api/devices", get(devices))
        .route("/api/flags", get(flags))
        .route("/api/transitions", get(transitions))
        .route("/api/transitions/journal", get(transitions_journal_batch))
        .route("/api/transitions/{id}/journal", get(transition_journal))
        .route("/api/series/{flag}", get(flag_series))
        .route("/api/metrics/{metric}", get(metric_series))
        .route("/api/context/keys", get(context_keys))
        .route("/api/context/health", get(context_health))
        .route("/api/context/transitions", get(context_transitions))
        .route(
            "/api/context/transitions/journal",
            get(context_transitions_journal_batch),
        )
        .route(
            "/api/context/transitions/{id}/journal",
            get(context_transition_journal),
        )
        .route("/api/context/series/{key}", get(context_series))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn system_page() -> Html<&'static str> {
    Html(include_str!("../static/system.html"))
}

async fn help_page() -> Html<&'static str> {
    Html(include_str!("../static/help.html"))
}

async fn analyze_page() -> Html<&'static str> {
    Html(include_str!("../static/analyze.html"))
}

async fn app_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/app.css"),
    )
}

async fn datasource_js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../static/datasource.js"),
    )
}

async fn health() -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok",
        version: version::VERSION,
        git_hash: version::GIT_HASH,
    })
}

async fn capabilities(State(state): State<AppState>) -> Json<CapabilitiesReport> {
    Json(state.capabilities.clone())
}

async fn time_bounds(State(state): State<AppState>) -> Result<Json<TimeBounds>> {
    Ok(Json(state.store.time_bounds().await?))
}

async fn system_info(State(state): State<AppState>) -> Result<Json<Option<SystemInfo>>> {
    Ok(Json(state.store.latest_system_info().await?))
}

async fn refresh_system_info(State(state): State<AppState>) -> Result<Json<SystemInfo>> {
    let info = tokio::task::spawn_blocking(system_info::collect_system_info)
        .await
        .map_err(|e| Error::Other(format!("system inventory task failed: {e}")))?;
    state.store.insert_system_info_snapshot(&info).await?;
    Ok(Json(info))
}

async fn findings(
    State(state): State<AppState>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<AnalyzeExportResponse>> {
    let (from, to) = parse_range(q.from, q.to)?;
    let opts = AnalysisOptions {
        from_ms: Some(from),
        to_ms: Some(to),
        device_pci: q.device_pci.clone(),
        issue_tag: None,
    };
    let bundle = state
        .store
        .build_analysis_bundle(from, to, q.device_pci.as_deref())
        .await?;
    let report = analyze::analyze_bundle(&bundle, &opts);
    Ok(Json(AnalyzeExportResponse {
        ai_prompt: analyze::render_ai_prompt(&report),
        report,
    }))
}

async fn analyze_export(
    Json(body): Json<AnalyzeExportBody>,
) -> Result<Json<AnalyzeExportResponse>> {
    validate_export_bundle(&body.bundle).map_err(Error::BadRequest)?;
    let from = body.from_ms.unwrap_or(body.bundle.from_ms);
    let to = body.to_ms.unwrap_or(body.bundle.to_ms);
    if from > to {
        return Err(Error::BadRequest("from must be <= to".into()));
    }
    let report = analyze::analyze_bundle(
        &body.bundle,
        &AnalysisOptions {
            from_ms: Some(from),
            to_ms: Some(to),
            device_pci: body.device_pci,
            issue_tag: body.issue_tag,
        },
    );
    Ok(Json(AnalyzeExportResponse {
        ai_prompt: analyze::render_ai_prompt(&report),
        report,
    }))
}

async fn window_summary(
    State(state): State<AppState>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<WindowSummary>> {
    let (from, to) = parse_range(q.from, q.to)?;
    Ok(Json(
        state
            .store
            .window_summary(from, to, q.device_pci.as_deref())
            .await?,
    ))
}

async fn export_data(
    State(state): State<AppState>,
    Query(q): Query<ExportQuery>,
) -> Result<Response> {
    let (from, to) = parse_range(q.from, q.to)?;
    let bundle = state
        .store
        .export_bundle(from, to, q.device_pci.as_deref())
        .await?;
    Ok(export_json_response(&bundle))
}

fn export_json_response(bundle: &ExportBundle) -> Response {
    let body = serde_json::to_vec_pretty(bundle).unwrap_or_default();
    let filename = format!("framelog-export-{}-{}.json", bundle.from_ms, bundle.to_ms);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response()
}

async fn devices(State(state): State<AppState>) -> Result<Json<serde_json::Value>> {
    let devices = state.store.devices().await?;
    Ok(Json(serde_json::json!({ "devices": devices })))
}

async fn flags(State(state): State<AppState>) -> Result<Json<serde_json::Value>> {
    let observed = state.store.distinct_flags().await?;
    let catalog: Vec<FlagCatalogEntry> = throttle::catalog_flags()
        .into_iter()
        .map(|name| FlagCatalogEntry {
            name: name.to_string(),
            category: throttle::flag_category(name).to_string(),
            description: throttle::flag_description(name).to_string(),
        })
        .collect();
    Ok(Json(
        serde_json::json!({ "flags": observed, "catalog": catalog }),
    ))
}

async fn transitions(
    State(state): State<AppState>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<Vec<FlagTransition>>> {
    let (from, to) = parse_range(q.from, q.to)?;
    let limit = query_limit(q.limit);
    let items = state
        .store
        .list_flag_transitions(crate::store::FlagTransitionQuery {
            from_ms: from,
            to_ms: to,
            limit,
            device_pci: q.device_pci.as_deref(),
            flag_name: q.flag_name.as_deref(),
            direction: q.direction.as_deref(),
            newest_first: true,
        })
        .await?;
    Ok(Json(items))
}

async fn transition_journal(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<JournalEvent>>> {
    let items = state.store.journal_for_transition(id).await?;
    Ok(Json(items))
}

async fn transitions_journal_batch(
    State(state): State<AppState>,
    Query(q): Query<JournalBatchQuery>,
) -> Result<Json<Vec<JournalEvent>>> {
    let ids = parse_ids(&q.ids)?;
    Ok(Json(state.store.journal_for_transitions(&ids).await?))
}

async fn context_transitions_journal_batch(
    State(state): State<AppState>,
    Query(q): Query<JournalBatchQuery>,
) -> Result<Json<Vec<JournalEvent>>> {
    let ids = parse_ids(&q.ids)?;
    Ok(Json(
        state.store.journal_for_context_transitions(&ids).await?,
    ))
}

fn parse_ids(raw: &str) -> Result<Vec<i64>> {
    let mut out = Vec::new();
    for part in raw.split(',').filter(|s| !s.is_empty()) {
        let id: i64 = part
            .trim()
            .parse()
            .map_err(|e| Error::BadRequest(format!("invalid id '{part}': {e}")))?;
        out.push(id);
    }
    Ok(out)
}

async fn flag_series(
    State(state): State<AppState>,
    Path(flag): Path<String>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<FlagSeries>> {
    let (from, to) = parse_range(q.from, q.to)?;
    let series = state
        .store
        .flag_series(q.device_pci.as_deref(), &flag, from, to, q.max_points)
        .await?;
    Ok(Json(series))
}

async fn metric_series(
    State(state): State<AppState>,
    Path(metric): Path<String>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<MetricSeries>> {
    let (from, to) = parse_range(q.from, q.to)?;
    Ok(Json(
        state
            .store
            .metric_series(&metric, q.device_pci.as_deref(), from, to, q.max_points)
            .await?,
    ))
}

async fn context_keys(State(state): State<AppState>) -> Result<Json<serde_json::Value>> {
    let keys = state.store.context_keys().await?;
    Ok(Json(serde_json::json!({ "keys": keys })))
}

async fn context_health(State(state): State<AppState>) -> Result<Json<Vec<ContextSourceStatus>>> {
    Ok(Json(state.store.latest_context_health().await?))
}

async fn context_transitions(
    State(state): State<AppState>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<Vec<ContextTransition>>> {
    let (from, to) = parse_range(q.from, q.to)?;
    let limit = query_limit(q.limit);
    Ok(Json(
        state
            .store
            .list_context_transitions_query(crate::store::ContextTransitionQuery {
                from_ms: from,
                to_ms: to,
                limit,
                source_id: q.source_id.as_deref(),
                key: q.key.as_deref(),
                newest_first: true,
            })
            .await?,
    ))
}

async fn context_transition_journal(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<JournalEvent>>> {
    Ok(Json(state.store.journal_for_context_transition(id).await?))
}

async fn context_series(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<ContextSeries>> {
    let (from, to) = parse_range(q.from, q.to)?;
    Ok(Json(
        state
            .store
            .context_series(&key, from, to, q.max_points)
            .await?,
    ))
}

fn parse_range(from: Option<i64>, to: Option<i64>) -> Result<(i64, i64)> {
    let now = crate::util::current_ts_ms();
    let to = to.unwrap_or(now);
    let from = from.unwrap_or(to - 86_400_000);
    if from > to {
        return Err(Error::BadRequest("from must be <= to".into()));
    }
    Ok((from, to))
}

fn query_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(500).clamp(1, 5_000)
}

pub async fn serve(
    bind: std::net::SocketAddr,
    store: Arc<Store>,
    collector: CollectorKind,
    apu_only: bool,
) -> Result<()> {
    let capabilities = crate::hardware::capabilities::probe(collector, apu_only);
    let state = AppState {
        store,
        capabilities,
    };
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| Error::Web(format!("bind {bind}: {e}")))?;

    tracing::info!(%bind, "web server listening");
    axum::serve(listener, app)
        .await
        .map_err(|e| Error::Web(format!("serve: {e}")))?;
    Ok(())
}
