mod feishu;
mod grid;
mod market;
mod store;
#[cfg(test)]
mod tests;
mod workers;

use crate::grid::{Config, Strategy};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::{collections::BTreeMap, env, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{RwLock, Semaphore};

#[derive(Clone)]
struct App {
    db: SqlitePool,
    client: Client,
    feishu: Option<Arc<feishu::Feishu>>,
    health: Arc<RwLock<BTreeMap<String, String>>>,
    quote_limit: Arc<Semaphore>,
}
struct ApiError(anyhow::Error);
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = if self.0.downcast_ref::<sqlx::Error>().is_some() {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::BAD_REQUEST
        };
        (status, Json(json!({"error":self.0.to_string()}))).into_response()
    }
}
type Reply = Result<Json<Value>, ApiError>;

async fn state(State(app): State<App>) -> Reply {
    let today = market::today();
    let plans = store::plans(&app.db)
        .await?
        .into_iter()
        .map(|s| {
            let (cash, shares) = s.available(&today);
            json!({"strategy":s,"free_cash":cash,"free_shares":shares})
        })
        .collect::<Vec<_>>();
    let filled_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM signals WHERE status='filled'")
            .fetch_one(&app.db)
            .await
            .map_err(anyhow::Error::from)?;
    Ok(Json(
        json!({"plans":plans,"orders":store::recent(&app.db).await?,"filled_count":filled_count,
        "market_window":market::session(market::local(Utc::now().timestamp())),
        "feishu":app.feishu.is_some(),"health":app.health.read().await.clone()}),
    ))
}
async fn quote(State(app): State<App>, Path(code): Path<String>) -> Reply {
    let q = market::fetch(&app.client, &code).await?;
    Ok(Json(json!(q)))
}
async fn preview(Json(cfg): Json<Config>) -> Reply {
    Ok(Json(json!({"levels":cfg.levels()?})))
}
async fn create(State(app): State<App>, Json(cfg): Json<Config>) -> Reply {
    cfg.levels()?;
    if cfg.notify && app.feishu.is_none() {
        return Err(anyhow::anyhow!("请先配置服务器飞书凭据").into());
    }
    let q = market::fetch(&app.client, &cfg.symbol).await?;
    let mut s = Strategy::new(cfg, q.name.clone())?;
    if q.usable(Utc::now().timestamp()) {
        s.tick(q.price, q.at, &market::today());
    }
    store::insert(&app.db, &s).await?;
    Ok(Json(json!({"id":s.id})))
}
#[derive(Deserialize)]
struct Switch {
    active: bool,
}
async fn toggle(State(app): State<App>, Path(id): Path<String>, Json(body): Json<Switch>) -> Reply {
    store::set_active(&app.db, &id, body.active).await?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
struct Resolution {
    action: String,
    price: Option<i64>,
    fee: Option<i64>,
}
async fn resolve(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<Resolution>,
) -> Reply {
    let fill = match body.action.as_str() {
        "cancel" => None,
        "fill" => Some((
            body.price.context("请填写实际成交价")?,
            body.fee.unwrap_or(0),
        )),
        _ => return Err(anyhow::anyhow!("未知的处理方式").into()),
    };
    store::resolve(&app.db, &id, fill).await?;
    Ok(Json(json!({"ok":true})))
}
async fn test_push(State(app): State<App>) -> Reply {
    let sender = app.feishu.as_ref().context("飞书尚未配置")?;
    sender
        .send(
            &app.client,
            "【A股网格 · 连接测试】飞书通知已接通。此消息仅用于验证接收渠道。",
            &uuid::Uuid::new_v4().to_string(),
        )
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn guard(req: Request<Body>, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next();
    if !matches!(host, Some("127.0.0.1" | "localhost")) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if req.method() != axum::http::Method::GET
        && req
            .headers()
            .get("x-grid-action")
            .and_then(|v| v.to_str().ok())
            != Some("1")
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    response
}

#[tokio::main]
async fn main() -> Result<()> {
    let port = env::var("GRID_PORT")
        .unwrap_or_else(|_| "8716".into())
        .parse::<u16>()?;
    ensure!(port > 0, "端口无效");
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .context("无法绑定本机端口")?;
    let path = PathBuf::from(env::var_os("GRID_DB").unwrap_or_else(|| "data/grid.sqlite".into()));
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let app = App {
        db: store::open(&path).await?,
        client: Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        feishu: feishu::Feishu::load()?.map(Arc::new),
        health: Default::default(),
        quote_limit: Arc::new(Semaphore::new(8)),
    };
    tokio::spawn(workers::quotes(app.clone()));
    tokio::spawn(workers::outbox(app.clone()));
    let router = Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../web/style.css"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route("/api/state", get(state))
        .route("/api/quote/{code}", get(quote))
        .route("/api/preview", post(preview))
        .route("/api/plans", post(create))
        .route("/api/plans/{id}/active", post(toggle))
        .route("/api/orders/{id}", post(resolve))
        .route("/api/feishu/test", post(test_push))
        .layer(middleware::from_fn(guard))
        .with_state(app);
    println!("A股网格控制台 http://127.0.0.1:{port}");
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
