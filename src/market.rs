use crate::grid::market;
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDateTime, TimeZone, Timelike, Utc, Weekday};
use reqwest::Client;
use serde::Serialize;

pub fn china() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).unwrap()
}
pub fn local(ts: i64) -> DateTime<FixedOffset> {
    DateTime::from_timestamp(ts, 0)
        .unwrap()
        .with_timezone(&china())
}
pub fn today() -> String {
    Utc::now()
        .with_timezone(&china())
        .format("%Y-%m-%d")
        .to_string()
}
pub fn session(t: DateTime<FixedOffset>) -> bool {
    let sec = t.num_seconds_from_midnight();
    !matches!(t.weekday(), Weekday::Sat | Weekday::Sun)
        && ((34200..41400).contains(&sec) || (46800..53820).contains(&sec))
}
#[derive(Clone, Serialize)]
pub struct Quote {
    pub symbol: String,
    pub name: String,
    pub price: i64,
    pub at: i64,
}
impl Quote {
    pub fn usable(&self, now: i64) -> bool {
        (0..=90).contains(&(now - self.at)) && session(local(now)) && session(local(self.at))
    }
}
fn parse(bytes: &[u8], symbol: &str) -> Result<Quote> {
    let (raw, _, _) = encoding_rs::GBK.decode(bytes);
    let prefix = format!("v_{}{}=\"", market(symbol)?, symbol);
    let fields: Vec<_> = raw
        .trim()
        .strip_prefix(&prefix)
        .context("未找到该股票")?
        .split('~')
        .collect();
    ensure!(fields.len() > 30 && fields[2] == symbol, "行情代码不匹配");
    let price = (fields[3].parse::<f64>().context("行情价格无效")? * 100.0).round() as i64;
    ensure!((1..=10_000_000).contains(&price), "行情无有效成交价");
    let time = NaiveDateTime::parse_from_str(fields[30], "%Y%m%d%H%M%S").context("行情时间无效")?;
    Ok(Quote {
        symbol: symbol.into(),
        name: fields[1].into(),
        price,
        at: china()
            .from_local_datetime(&time)
            .single()
            .unwrap()
            .timestamp(),
    })
}
pub async fn fetch(client: &Client, symbol: &str) -> Result<Quote> {
    let url = format!("https://qt.gtimg.cn/q={}{}", market(symbol)?, symbol);
    let r = client
        .get(url)
        .send()
        .await
        .context("行情服务连接失败")?
        .error_for_status()
        .context("行情服务暂不可用")?;
    parse(&r.bytes().await.context("行情读取失败")?, symbol)
}
