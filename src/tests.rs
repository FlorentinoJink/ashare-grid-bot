use super::*;
use chrono::TimeZone;
use grid::{Mode, Side};
use market::{Quote, china};
use std::sync::atomic::{AtomicUsize, Ordering};

fn config() -> Config {
    Config {
        symbol: "600036".into(),
        lower: 1000,
        upper: 1400,
        grids: 4,
        mode: Mode::Arithmetic,
        quantity: 100,
        cash: 1_000_000,
        shares: 0,
        notify: true,
    }
}
fn strategy() -> Strategy {
    Strategy::new(config(), "样例股".into()).unwrap()
}
fn instant(day: u32, h: u32, m: u32, s: u32) -> i64 {
    china()
        .with_ymd_and_hms(2026, 11, day, h, m, s)
        .unwrap()
        .timestamp()
}
fn q(price: i64, at: i64) -> Quote {
    Quote {
        price,
        at,
        symbol: "600036".into(),
        name: "样例股".into(),
    }
}

#[test]
fn spacing_and_rounding() {
    let mut c = config();
    assert_eq!(c.levels().unwrap(), [1000, 1100, 1200, 1300, 1400]);
    c.upper = 4000;
    c.grids = 2;
    c.mode = Mode::Geometric;
    assert_eq!(c.levels().unwrap(), [1000, 2000, 4000]);
    c.upper = 1001;
    assert!(c.levels().is_err());
}
#[test]
fn board_quantities() {
    let mut c = config();
    for (symbol, qty, ok) in [
        ("688981", 100, false),
        ("688981", 201, true),
        ("300750", 300100, false),
        ("920002", 101, true),
        ("600036", 101, false),
        ("000001", 100, true),
        ("510300", 100, false),
        ("☆123", 100, false),
    ] {
        c.symbol = symbol.into();
        c.quantity = qty;
        assert_eq!(c.levels().is_ok(), ok);
    }
}
#[test]
fn gap_uses_cash_in_crossing_order() {
    let mut c = config();
    c.cash = 250_000;
    let mut s = Strategy::new(c, "".into()).unwrap();
    let day = "2026-11-24";
    assert!(s.tick(1450, 1, day).is_empty());
    let signals = s.tick(990, 2, day);
    assert_eq!(
        signals.iter().map(|s| s.price).collect::<Vec<_>>(),
        [1300, 1200]
    );
    assert_eq!(s.skipped, 2);
    assert_eq!(s.available(day), (0, 0));
    assert!(s.tick(990, 3, day).is_empty());
    assert!(s.tick(1400, 2, day).is_empty());
    s.resolve(&signals[0], None, day).unwrap();
    assert_eq!(s.available(day), (130_000, 0));
    assert!(s.resolve(&signals[0], None, day).is_err());
}
#[test]
fn confirmed_buy_then_t_plus_one() {
    let mut s = strategy();
    let day = "2026-11-24";
    let next = "2026-11-25";
    s.tick(1100, 1, day);
    let orders = s.tick(1000, 2, day);
    assert_eq!(orders.len(), 1);
    assert!(s.resolve(&orders[0], Some((1001, 0)), day).is_err());
    s.resolve(&orders[0], Some((999, 500)), day).unwrap();
    assert_eq!((s.cash, s.shares), (899600, 100));
    assert_eq!(s.available(day).1, 0);
    assert!(s.tick(1100, 3, day).is_empty());
    s.tick(1090, 4, next);
    let sells = s.tick(1100, 5, next);
    assert_eq!(sells.len(), 1);
    assert_eq!(sells[0].side, Side::Sell);
    s.resolve(&sells[0], Some((1105, 300)), next).unwrap();
    assert_eq!((s.cash, s.shares), (1009800, 0));
    assert_eq!(s.cells[0].side, Side::Buy);
}
#[test]
fn initial_holdings_limit_pending_sells() {
    let mut c = config();
    c.shares = 100;
    let mut s = Strategy::new(c, "".into()).unwrap();
    s.tick(990, 1, "d");
    let signals = s.tick(1410, 2, "d");
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].price, 1100);
    assert_eq!(s.available("d").1, 0);
}
#[test]
fn timestamp_and_market_windows() {
    let at = instant(24, 9, 30, 0);
    assert!(q(1000, at).usable(at + 90));
    assert!(!q(1000, at).usable(at + 91));
    assert!(!q(1000, at + 1).usable(at));
    for t in [
        instant(24, 9, 29, 59),
        instant(24, 11, 30, 0),
        instant(24, 14, 57, 0),
        instant(22, 10, 0, 0),
    ] {
        assert!(!q(1000, t).usable(t));
    }
}
#[tokio::test]
async fn restart_retry_delivery_and_resolve() -> Result<()> {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(tokio::sync::Mutex::new(vec![]));
    let mock = Router::new()
        .route(
            "/open-apis/auth/v3/tenant_access_token/internal",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(body["app_id"], "test-app");
                Json(json!({"code":0,"tenant_access_token":"fixture","expire":7200}))
            }),
        )
        .route(
            "/open-apis/im/v1/messages",
            post({
                let calls = calls.clone();
                let observed = observed.clone();
                move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
                    let calls = calls.clone();
                    let observed = observed.clone();
                    async move {
                        assert_eq!(headers["authorization"], "Bearer fixture");
                        assert_eq!(body["receive_id"], "test-recipient");
                        assert!(body["content"].as_str().unwrap().contains("待人工执行"));
                        observed.lock().await.push(body["uuid"].clone());
                        Json(if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                            json!({"code":99991400})
                        } else {
                            json!({"code":0,"data":{"message_id":"receipt"}})
                        })
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let path = std::env::temp_dir().join(format!("grid-test-{}.sqlite", uuid::Uuid::new_v4()));
    let s = strategy();
    let db = store::open(&path).await?;
    store::insert(&db, &s).await?;
    let at = instant(24, 10, 0, 0);
    store::apply_quote(&db, &s.id, &q(1100, at), at).await?;
    store::apply_quote(&db, &s.id, &q(1000, at + 3), at + 3).await?;
    db.close().await;
    let app = App {
        db: store::open(&path).await?,
        client: Client::new(),
        feishu: Some(Arc::new(feishu::Feishu::fixture(base))),
        health: Default::default(),
        quote_limit: Arc::new(Semaphore::new(1)),
    };
    store::apply_quote(&app.db, &s.id, &q(1000, at + 3), at + 3).await?;
    assert!(workers::deliver(&app, at + 4).await?);
    assert!(!workers::deliver(&app, at + 5).await?);
    assert!(workers::deliver(&app, at + 10).await?);
    let orders = store::recent(&app.db).await?;
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].delivery, "sent");
    let ids = observed.lock().await;
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1]);
    let id = &orders[0].signal.id;
    let (a, b) = tokio::join!(
        store::resolve(&app.db, id, Some((995, 500))),
        store::resolve(&app.db, id, Some((995, 500)))
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(store::plans(&app.db).await?[0].shares, 100);
    app.db.close().await;
    std::fs::remove_file(path)?;
    task.abort();
    Ok(())
}
