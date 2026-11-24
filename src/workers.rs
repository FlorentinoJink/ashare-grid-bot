use crate::{App, feishu, grid::Signal, market, store};
use anyhow::Result;
use chrono::Utc;
use sqlx::Row;
use std::{collections::BTreeMap, time::Duration};

async fn poll(app: &App) -> Result<()> {
    if !market::session(market::local(Utc::now().timestamp())) {
        return Ok(());
    }
    let mut symbols = BTreeMap::new();
    for s in store::plans(&app.db)
        .await?
        .into_iter()
        .filter(|s| s.active)
    {
        symbols
            .entry(s.config.symbol)
            .or_insert_with(Vec::new)
            .push(s.id);
    }
    let mut tasks = tokio::task::JoinSet::new();
    for (symbol, ids) in symbols {
        let app = app.clone();
        tasks.spawn(async move {
            let _permit = app.quote_limit.acquire().await?;
            let note = match market::fetch(&app.client, &symbol).await {
                Ok(q) if q.usable(Utc::now().timestamp()) => {
                    for id in ids {
                        store::apply_quote(&app.db, &id, &q, Utc::now().timestamp()).await?;
                    }
                    "行情正常".to_string()
                }
                Ok(_) => "等待新鲜盘中行情".into(),
                Err(e) => e.to_string(),
            };
            app.health.write().await.insert(symbol, note);
            Ok::<_, anyhow::Error>(())
        });
    }
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}
pub async fn quotes(app: App) {
    loop {
        if let Err(e) = poll(&app).await {
            eprintln!("行情轮询失败 {e}");
        }
        tokio::time::sleep(Duration::from_secs(8)).await;
    }
}
pub(crate) async fn deliver(app: &App, now: i64) -> Result<bool> {
    sqlx::query("UPDATE signals SET delivery='expired' WHERE delivery='queued' AND (at<? OR ?=0)")
        .bind(now - 180)
        .bind(market::session(market::local(now)))
        .execute(&app.db)
        .await?;
    let Some(sender) = &app.feishu else {
        return Ok(false);
    };
    let row=sqlx::query("SELECT id,body,attempts FROM signals WHERE delivery='queued' AND status='open' AND retry_at<=? ORDER BY rowid LIMIT 1")
        .bind(now).fetch_optional(&app.db).await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let sig: Signal = serde_json::from_str(row.try_get("body")?)?;
    let attempts: i64 = row.try_get("attempts")?;
    let (status, error) = match sender
        .send(&app.client, &feishu::message(&sig), &sig.id)
        .await
    {
        Ok(()) => ("sent", None),
        Err(e) => (
            if attempts >= 4 { "failed" } else { "queued" },
            Some(e.to_string()),
        ),
    };
    sqlx::query("UPDATE signals SET attempts=attempts+1,delivery=?,retry_at=?,error=? WHERE id=? AND (delivery='queued' OR (delivery='closed' AND ?='sent'))")
        .bind(status).bind(now+(4<<attempts)).bind(error).bind(&sig.id).bind(status).execute(&app.db).await?;
    Ok(true)
}
pub async fn outbox(app: App) {
    loop {
        let delay = match deliver(&app, Utc::now().timestamp()).await {
            Ok(true) => 500,
            Ok(false) => 2500,
            Err(e) => {
                eprintln!("飞书队列异常 {e}");
                5000
            }
        };
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
}
