use crate::{App, Reply};
use anyhow::{Result, bail, ensure};
use axum::{
    Json,
    extract::{Path, State},
    response::Html,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub fn page(template: &str, active: &str) -> Html<String> {
    let mut nav = String::from(r#"<nav class="strategy-nav" aria-label="策略导航"><div>"#);
    for (url, label) in [
        ("/", "网格交易"),
        ("/dividends", "红利收息"),
        ("/allocation", "股债配置"),
    ] {
        let current = if url == active {
            r#" aria-current="page""#
        } else {
            ""
        };
        nav.push_str(&format!(r#"<a href="{url}"{current}>{label}</a>"#));
    }
    nav.push_str("</div></nav>");
    Html(template.replace("<!-- strategy-nav -->", &nav))
}

#[derive(Deserialize, Serialize)]
struct Dividend {
    symbol: String,
    year: u16,
    basis: String,
    dividend: f64,
    target: f64,
    price: f64,
    shares: u32,
}

#[derive(Deserialize, Serialize)]
struct Allocation {
    name: String,
    equity: f64,
    bonds: f64,
    target: f64,
}

fn validate(section: &str, input: Value) -> Result<(String, Value)> {
    match section {
        "dividends" => {
            let d: Dividend = serde_json::from_value(input)?;
            ensure!(
                d.symbol.len() == 6 && d.symbol.bytes().all(|b| b.is_ascii_digit()),
                "请输入六位证券代码"
            );
            ensure!(
                (1990..=2100).contains(&d.year) && matches!(d.basis.as_str(), "预案" | "已实施"),
                "分红年度或口径无效"
            );
            ensure!(
                (0.0..=100000.0).contains(&d.dividend) && d.price >= 0.001 && d.price <= 100000.0,
                "分红或参考价超出范围"
            );
            ensure!(
                (0.01..=100.0).contains(&d.target),
                "目标股息率应在 0.01% 至 100% 之间"
            );
            Ok((format!("{}-{}", d.symbol, d.year), json!(d)))
        }
        "allocation" => {
            let mut a: Allocation = serde_json::from_value(input)?;
            a.name = a.name.trim().to_owned();
            ensure!(
                !a.name.is_empty() && a.name.chars().count() <= 40,
                "组合名称应为 1 至 40 个字符"
            );
            ensure!(
                [a.equity, a.bonds]
                    .into_iter()
                    .all(|v| (0.0..=1e12).contains(&v))
                    && a.equity + a.bonds > 0.0,
                "请填写有效市值，组合总额须大于零"
            );
            ensure!(
                (0.0..=100.0).contains(&a.target),
                "目标权重应在 0% 至 100% 之间"
            );
            Ok((a.name.clone(), json!(a)))
        }
        _ => bail!("未知策略页面"),
    }
}

pub async fn list(State(app): State<App>, Path(section): Path<String>) -> Reply {
    if !matches!(section.as_str(), "dividends" | "allocation") {
        return Err(anyhow::anyhow!("未知策略页面").into());
    }
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT body FROM notebook_entries WHERE section=? ORDER BY entry_key")
            .bind(section)
            .fetch_all(&app.db)
            .await
            .map_err(anyhow::Error::from)?;
    let entries: Vec<Value> = rows
        .iter()
        .map(|s| serde_json::from_str(s))
        .collect::<Result<_, _>>()
        .map_err(anyhow::Error::from)?;
    Ok(Json(json!(entries)))
}

pub async fn save(
    State(app): State<App>,
    Path(section): Path<String>,
    Json(input): Json<Value>,
) -> Reply {
    let (key, data) = validate(&section, input)?;
    let record = json!({"key":key, "data":data, "updated_at":chrono::Utc::now().timestamp()});
    sqlx::query("INSERT INTO notebook_entries(section,entry_key,body) VALUES(?,?,?) ON CONFLICT(section,entry_key) DO UPDATE SET body=excluded.body")
        .bind(section).bind(&key).bind(record.to_string()).execute(&app.db).await.map_err(anyhow::Error::from)?;
    Ok(Json(record))
}
