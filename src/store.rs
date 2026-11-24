use crate::{
    grid::{Signal, Strategy},
    market::{Quote, local, today},
};
use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{
    Row, SqliteConnection, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

pub async fn open(path: &std::path::Path) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await?;
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS plans (id TEXT PRIMARY KEY, body TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS signals (id TEXT PRIMARY KEY, plan TEXT NOT NULL, at INTEGER NOT NULL,
        body TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'open', delivery TEXT NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0, retry_at INTEGER NOT NULL DEFAULT 0,
        error TEXT, fill INTEGER, fee INTEGER);
        CREATE INDEX IF NOT EXISTS outbox ON signals(status,delivery,retry_at);
        CREATE TABLE IF NOT EXISTS notebook_entries (
            section TEXT NOT NULL, entry_key TEXT NOT NULL, body TEXT NOT NULL,
            PRIMARY KEY(section, entry_key)
        );").execute(&pool).await?;
    Ok(pool)
}

pub async fn plans(pool: &SqlitePool) -> Result<Vec<Strategy>> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT body FROM plans ORDER BY rowid DESC")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|s| serde_json::from_str(s))
        .collect::<Result<_, _>>()?)
}
pub async fn get(db: &mut SqliteConnection, id: &str) -> Result<Strategy> {
    let body: String = sqlx::query_scalar("SELECT body FROM plans WHERE id=?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .context("策略不存在")?;
    Ok(serde_json::from_str(&body)?)
}
pub async fn save(db: &mut SqliteConnection, s: &Strategy) -> Result<()> {
    sqlx::query(
        "INSERT INTO plans(id,body) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body",
    )
    .bind(&s.id)
    .bind(serde_json::to_string(s)?)
    .execute(db)
    .await?;
    Ok(())
}
pub async fn apply_quote(pool: &SqlitePool, id: &str, q: &Quote, now: i64) -> Result<()> {
    if !q.usable(now) {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    let mut s = get(&mut tx, id).await?;
    if !s.active || s.config.symbol != q.symbol || s.last_time >= q.at {
        return Ok(());
    }
    let day = local(q.at).format("%Y-%m-%d").to_string();
    for sig in s.tick(q.price, q.at, &day) {
        sqlx::query("INSERT INTO signals(id,plan,at,body,delivery) VALUES(?,?,?,?,?)")
            .bind(&sig.id)
            .bind(id)
            .bind(sig.at)
            .bind(serde_json::to_string(&sig)?)
            .bind(if s.config.notify { "queued" } else { "off" })
            .execute(&mut *tx)
            .await?;
    }
    save(&mut tx, &s).await?;
    tx.commit().await?;
    Ok(())
}

#[derive(Serialize)]
pub struct Entry {
    pub signal: Signal,
    pub status: String,
    pub delivery: String,
    pub error: Option<String>,
    pub fill: Option<i64>,
    pub fee: Option<i64>,
}
pub async fn recent(pool: &SqlitePool) -> Result<Vec<Entry>> {
    // Open reservations always remain accessible even when there are more than 100 historic orders.
    let rows=sqlx::query("SELECT * FROM signals WHERE status='open' OR rowid IN
        (SELECT rowid FROM signals WHERE status!='open' ORDER BY rowid DESC LIMIT 100) ORDER BY rowid DESC").fetch_all(pool).await?;
    rows.into_iter()
        .map(|r| {
            Ok(Entry {
                signal: serde_json::from_str(r.try_get("body")?)?,
                status: r.try_get("status")?,
                delivery: r.try_get("delivery")?,
                error: r.try_get("error")?,
                fill: r.try_get("fill")?,
                fee: r.try_get("fee")?,
            })
        })
        .collect()
}
pub async fn resolve(pool: &SqlitePool, id: &str, fill: Option<(i64, i64)>) -> Result<()> {
    let mut tx = pool.begin().await?;
    let raw: String = sqlx::query_scalar("SELECT body FROM signals WHERE id=? AND status='open'")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .context("找不到待处理订单，可能已经登记")?;
    let sig: Signal = serde_json::from_str(&raw)?;
    let mut s = get(&mut tx, &sig.strategy).await?;
    s.resolve(&sig, fill, &today())?;
    save(&mut tx, &s).await?;
    sqlx::query("UPDATE signals SET status=?,fill=?,fee=?,delivery=CASE WHEN delivery='queued' THEN 'closed' ELSE delivery END WHERE id=?")
        .bind(if fill.is_some(){"filled"}else{"cancelled"}).bind(fill.map(|x|x.0)).bind(fill.map(|x|x.1)).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn set_active(pool: &SqlitePool, id: &str, active: bool) -> Result<()> {
    let mut tx = pool.begin().await?;
    let mut s = get(&mut tx, id).await?;
    if s.active != active {
        s.active = active;
        s.last = None;
        save(&mut tx, &s).await?;
    }
    tx.commit().await?;
    Ok(())
}
pub async fn insert(pool: &SqlitePool, s: &Strategy) -> Result<()> {
    let mut tx = pool.begin().await?;
    save(&mut tx, s).await?;
    tx.commit().await?;
    Ok(())
}
