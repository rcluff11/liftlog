//! liftlog web server: JSON API + the mobile page, sharing ~/.liftlog/liftlog.db with the CLI.
//!
//! Run with `cargo run --bin server`, then open the printed URL on your phone.

use axum::{
    Json, Router,
    extract::{Path, rejection::JsonRejection},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use chrono::{Local, NaiveDate};
use liftlog::*;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::net::UdpSocket;

const INDEX_HTML: &str = include_str!("../../static/index.html");
const ICON_180: &[u8] = include_bytes!("../../static/icon-180.png");
const ICON_512: &[u8] = include_bytes!("../../static/icon-512.png");
const MANIFEST: &str = r##"{
  "name": "LiftLog",
  "short_name": "LiftLog",
  "start_url": "/",
  "display": "standalone",
  "background_color": "#0b0d10",
  "theme_color": "#0b0d10",
  "icons": [
    { "src": "/icon-180.png", "sizes": "180x180", "type": "image/png" },
    { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png" }
  ]
}"##;

/// How many sessions /api/history returns.
const HISTORY_DAYS: usize = 20;

// ---------- errors ----------

enum ApiError {
    BadRequest(String),
    NotFound(String),
    Internal(String),
    /// Malformed JSON body; keeps axum's status (400, 415 or 422).
    BadJson(JsonRejection),
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::BadJson(rejection) => (rejection.status(), rejection.body_text()),
            ApiError::Internal(msg) => {
                eprintln!("server error: {msg}");
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            }
        };
        (status, Json(ErrorBody { error })).into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        ApiError::Internal(format!("database error: {e}"))
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

/// One connection per request: simple, and fine for a single user.
fn db() -> Result<Connection, ApiError> {
    open_db().map_err(|e| ApiError::Internal(format!("couldn't open database: {e}")))
}

/// Exercise names are stored lowercased and trimmed.
fn clean_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim().to_lowercase();
    if name.is_empty() {
        return Err(ApiError::BadRequest("exercise name is required".into()));
    }
    Ok(name)
}

// ---------- handlers ----------

async fn get_exercises() -> ApiResult<Vec<String>> {
    Ok(Json(exercise_names(&db()?)?))
}

async fn get_next(Path(exercise): Path<String>) -> ApiResult<Option<Suggestion>> {
    let name = clean_name(&exercise)?;
    Ok(Json(suggest_next(&db()?, &name)?))
}

async fn get_history(Path(exercise): Path<String>) -> ApiResult<Vec<HistoryDay>> {
    let name = clean_name(&exercise)?;
    let mut days = history(&db()?, &name)?;
    days.truncate(HISTORY_DAYS);
    Ok(Json(days))
}

async fn get_prs() -> ApiResult<Vec<Pr>> {
    Ok(Json(prs(&db()?)?))
}

async fn get_trend(Path(exercise): Path<String>) -> ApiResult<Trend> {
    let name = clean_name(&exercise)?;
    Ok(Json(trend(&db()?, &name)?))
}

#[derive(Deserialize)]
struct LogRequest {
    exercise: String,
    sets: Vec<Set>,
    date: Option<String>,
}

#[derive(Serialize)]
struct Saved {
    saved: usize,
}

async fn post_log(body: Result<Json<LogRequest>, JsonRejection>) -> ApiResult<Saved> {
    let Json(req) = body.map_err(ApiError::BadJson)?;

    let name = clean_name(&req.exercise)?;
    if req.sets.is_empty() {
        return Err(ApiError::BadRequest("add at least one set".into()));
    }
    for (i, set) in req.sets.iter().enumerate() {
        if !set.weight.is_finite() || set.weight < 0.0 || set.weight > 2000.0 {
            return Err(ApiError::BadRequest(format!(
                "set {}: weight must be 0-2000 lbs",
                i + 1
            )));
        }
        if set.reps == 0 || set.reps > 100 {
            return Err(ApiError::BadRequest(format!(
                "set {}: reps must be 1-100",
                i + 1
            )));
        }
    }

    let date = match req.date.as_deref().map(str::trim) {
        None | Some("") => Local::now().date_naive(),
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .map_err(|_| ApiError::BadRequest(format!("date '{d}' should look like YYYY-MM-DD")))?,
    };

    let mut conn = db()?;
    log_sets(&mut conn, &name, &req.sets, date)?;
    Ok(Json(Saved {
        saved: req.sets.len(),
    }))
}

async fn api_not_found() -> ApiError {
    ApiError::NotFound("unknown API endpoint".into())
}

// ---------- static files ----------

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        MANIFEST,
    )
}

async fn icon_180() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png")], ICON_180)
}

async fn icon_512() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png")], ICON_512)
}

/// The machine's LAN address, found by asking the OS which interface would
/// route to the internet. Nothing is actually sent.
fn lan_ip() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Fail fast if the database can't be opened.
    open_db()?;

    let api = Router::new()
        .route("/exercises", get(get_exercises))
        .route("/next/{exercise}", get(get_next))
        .route("/history/{exercise}", get(get_history))
        .route("/prs", get(get_prs))
        .route("/trend/{exercise}", get(get_trend))
        .route("/log", post(post_log))
        .fallback(api_not_found);

    let app = Router::new()
        .route("/", get(index))
        .route("/manifest.webmanifest", get(manifest))
        .route("/icon-180.png", get(icon_180))
        .route("/apple-touch-icon.png", get(icon_180))
        .route("/icon-512.png", get(icon_512))
        .nest("/api", api);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    println!("liftlog server running.");
    // 127.0.0.1, not localhost: on Windows "localhost" tries IPv6 first and adds ~200ms per request.
    println!("  On this computer: http://127.0.0.1:3000");
    match lan_ip() {
        Some(ip) => println!("  On your iPhone (same Wi-Fi): http://{ip}:3000"),
        None => println!("  On your iPhone: http://<this computer's IP>:3000"),
    }
    println!("Press Ctrl+C to stop.");
    axum::serve(listener, app).await?;
    Ok(())
}
