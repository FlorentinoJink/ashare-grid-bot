use crate::{grid::Signal, market::local};
use anyhow::{Context, Result, ensure};
use reqwest::Client;
use serde_json::{Value, json};
use std::{
    env, fs,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use toml_edit::Document;

pub struct Feishu {
    app: String,
    secret: String,
    recipient: String,
    base: String,
    token: Mutex<Option<(String, Instant)>>,
}
impl Feishu {
    pub fn load() -> Result<Option<Self>> {
        let get = |k: &str| env::var(format!("TRADER_FEISHU_{k}")).unwrap_or_default();
        let (mut app, mut secret, mut recipient) =
            (get("APP_ID"), get("APP_SECRET"), get("OPEN_ID"));
        if let Some(path) = env::var_os("FEISHU_CONFIG_FILE") {
            let raw = fs::read_to_string(path).context("无法读取指定的飞书配置文件")?;
            let doc = Document::parse(raw.trim_start_matches('\u{feff}'))
                .map_err(|_| anyhow::anyhow!("飞书配置 TOML 格式错误"))?;
            let field = |name: &str| {
                doc.get("feishu")
                    .and_then(|s| s.get(name))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned()
            };
            if app.is_empty() {
                app = field("AppID");
            }
            if secret.is_empty() {
                secret = field("AppSecret");
            }
            if recipient.is_empty() {
                recipient = field("OpenID");
            }
        }
        if [app.as_str(), secret.as_str(), recipient.as_str()]
            .iter()
            .all(|s| s.is_empty())
        {
            return Ok(None);
        }
        ensure!(
            !app.is_empty() && !secret.is_empty() && !recipient.is_empty(),
            "请完整配置飞书 App ID、App Secret 和 Open ID"
        );
        Ok(Some(Self {
            app,
            secret,
            recipient,
            base: "https://open.feishu.cn".into(),
            token: Mutex::new(None),
        }))
    }
    #[cfg(test)]
    pub fn fixture(base: String) -> Self {
        Self {
            app: "test-app".into(),
            secret: "test-secret".into(),
            recipient: "test-recipient".into(),
            base,
            token: Mutex::new(None),
        }
    }
    async fn token(&self, client: &Client) -> Result<String> {
        let mut cache = self.token.lock().await;
        if let Some((token, _)) = cache.as_ref().filter(|(_, t)| *t > Instant::now()) {
            return Ok(token.clone());
        }
        let r = client
            .post(format!(
                "{}/open-apis/auth/v3/tenant_access_token/internal",
                self.base
            ))
            .json(&json!({"app_id":self.app,"app_secret":self.secret}))
            .send()
            .await?;
        let v: Value = r.error_for_status()?.json().await?;
        ensure!(
            v["code"].as_i64() == Some(0),
            "飞书授权失败（{}）",
            v["code"].as_i64().unwrap_or(-1)
        );
        let token = v["tenant_access_token"]
            .as_str()
            .context("飞书未返回凭证")?
            .to_owned();
        *cache = Some((
            token.clone(),
            Instant::now()
                + Duration::from_secs(v["expire"].as_u64().unwrap_or(120).clamp(61, 7200) - 60),
        ));
        Ok(token)
    }
    pub async fn send(&self, client: &Client, text: &str, id: &str) -> Result<()> {
        let token = self.token(client).await?;
        let body = json!({"receive_id":self.recipient,"msg_type":"text","uuid":id,"content":json!({"text":text}).to_string()});
        let r = client
            .post(format!(
                "{}/open-apis/im/v1/messages?receive_id_type=open_id",
                self.base
            ))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await?;
        let http_ok = r.status().is_success();
        let v: Value = r.json().await?;
        if matches!(v["code"].as_i64(), Some(99991663 | 99991668)) {
            *self.token.lock().await = None;
        }
        ensure!(
            http_ok && v["code"].as_i64() == Some(0) && v["data"]["message_id"].is_string(),
            "飞书发送失败（{}）",
            v["code"].as_i64().unwrap_or(-1)
        );
        Ok(())
    }
}
pub fn message(s: &Signal) -> String {
    format!(
        "【A股网格 · 待人工执行】\n{} {}　{} {} 股\n信号限价 ¥{:.2}　触发报价 ¥{:.2}\n参考金额 ¥{:.2}　网格第 {} 格\n行情时间 {}（北京时间）\n当日信号，请核对券商可用资金、持仓及委托状态。成交后在控制台登记。\n信号编号 {}",
        s.symbol,
        s.name,
        s.side.label(),
        s.quantity,
        s.price as f64 / 100.,
        s.quote as f64 / 100.,
        (s.price * s.quantity) as f64 / 100.,
        s.cell + 1,
        local(s.at).format("%Y-%m-%d %H:%M:%S"),
        s.id
    )
}
