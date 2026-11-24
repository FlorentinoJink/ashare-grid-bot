use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Arithmetic,
    Geometric,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub symbol: String,
    pub lower: i64,
    pub upper: i64,
    pub grids: usize,
    pub mode: Mode,
    pub quantity: i64,
    pub cash: i64,
    pub shares: i64,
    pub notify: bool,
}

pub fn market(code: &str) -> Result<&'static str> {
    ensure!(
        code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()),
        "请输入六位股票代码"
    );
    match &code[..3] {
        "600" | "601" | "603" | "605" | "688" => Ok("sh"),
        "000" | "001" | "002" | "003" | "300" | "301" => Ok("sz"),
        "920" => Ok("bj"),
        _ => bail!("请输入沪深主板、创业板、科创板或北交所普通股票代码"),
    }
}

impl Config {
    pub fn initial_cells(&self, levels: &[i64], reference: i64, mut shares: i64) -> Vec<Cell> {
        levels
            .windows(2)
            .map(|pair| {
                let side = if pair[0] >= reference && shares >= self.quantity {
                    shares -= self.quantity;
                    Side::Sell
                } else {
                    Side::Buy
                };
                Cell {
                    side,
                    pending: None,
                }
            })
            .collect()
    }

    pub fn levels(&self) -> Result<Vec<i64>> {
        market(&self.symbol)?;
        ensure!(
            self.lower > 0 && self.upper > self.lower && self.upper <= 10_000_000,
            "价格区间无效（单位为分）"
        );
        ensure!((2..=100).contains(&self.grids), "网格数应为 2 至 100");
        let (min, max, step) = match &self.symbol[..3] {
            "688" => (200, 100_000, 1),
            "300" | "301" => (100, 300_000, 100),
            "920" => (100, 1_000_000, 1),
            _ => (100, 1_000_000, 100),
        };
        ensure!(
            (min..=max).contains(&self.quantity) && self.quantity % step == 0,
            "该板块每格股数范围为 {min} 至 {max} 股，步长 {step} 股"
        );
        ensure!(
            (0..=10_000_000_000_000).contains(&self.cash)
                && (0..=100_000_000).contains(&self.shares),
            "初始资金或持仓超出范围"
        );
        ensure!(
            self.cash >= self.lower * self.quantity || self.shares >= self.quantity,
            "资金和底仓均不足以触发一格"
        );
        let mut levels = Vec::with_capacity(self.grids + 1);
        for i in 0..=self.grids {
            let n = self.grids as i64;
            let price = match self.mode {
                Mode::Arithmetic => self.lower + ((self.upper - self.lower) * i as i64 + n / 2) / n,
                Mode::Geometric => (self.lower as f64
                    * (self.upper as f64 / self.lower as f64).powf(i as f64 / n as f64))
                .round() as i64,
            };
            ensure!(
                levels.last().is_none_or(|p| *p < price),
                "网格价格按分取整后重叠，请减少格数"
            );
            levels.push(price);
        }
        Ok(levels)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn label(self) -> &'static str {
        match self {
            Self::Buy => "买入",
            Self::Sell => "卖出",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Cell {
    pub side: Side,
    pub pending: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Strategy {
    pub id: String,
    pub name: String,
    pub config: Config,
    pub levels: Vec<i64>,
    pub cells: Vec<Cell>,
    pub active: bool,
    pub cash: i64,
    pub shares: i64,
    pub locked: BTreeMap<String, i64>,
    pub last: Option<i64>,
    pub last_time: i64,
    pub skipped: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Signal {
    pub id: String,
    pub strategy: String,
    pub symbol: String,
    pub name: String,
    pub cell: usize,
    pub side: Side,
    pub price: i64,
    pub quantity: i64,
    pub quote: i64,
    pub at: i64,
    pub day: String,
}

impl Strategy {
    pub fn new(config: Config, name: String) -> Result<Self> {
        Ok(Self {
            id: Uuid::new_v4().to_string(),
            name,
            levels: config.levels()?,
            cells: vec![],
            active: true,
            cash: config.cash,
            shares: config.shares,
            locked: BTreeMap::new(),
            last: None,
            last_time: 0,
            skipped: 0,
            config,
        })
    }
    pub fn available(&self, day: &str) -> (i64, i64) {
        let mut cash = self.cash;
        let mut shares = self.shares
            - self
                .locked
                .iter()
                .filter(|(d, _)| d.as_str() >= day)
                .map(|(_, q)| q)
                .sum::<i64>();
        for (i, c) in self
            .cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.pending.is_some())
        {
            match c.side {
                Side::Buy => cash -= self.levels[i] * self.config.quantity,
                Side::Sell => shares -= self.config.quantity,
            }
        }
        (cash, shares)
    }
    pub fn tick(&mut self, price: i64, at: i64, day: &str) -> Vec<Signal> {
        if !self.active || price <= 0 || at <= self.last_time {
            return vec![];
        }
        self.last_time = at;
        self.locked.retain(|d, _| d.as_str() >= day);
        let previous = self.last.replace(price);
        if self.cells.is_empty() {
            self.cells = self
                .config
                .initial_cells(&self.levels, price, self.available(day).1);
        }
        let Some(old) = previous else {
            return vec![];
        };
        if old == price {
            return vec![];
        }
        let falling = price < old;
        let (mut money, mut available_shares) = self.available(day);
        let mut signals = vec![];
        for k in 0..self.cells.len() {
            let i = if falling { self.cells.len() - 1 - k } else { k };
            let cell = &mut self.cells[i];
            if cell.pending.is_some() {
                continue;
            }
            let limit = match cell.side {
                Side::Buy if falling && price <= self.levels[i] && self.levels[i] < old => {
                    self.levels[i]
                }
                Side::Sell
                    if !falling && old < self.levels[i + 1] && self.levels[i + 1] <= price =>
                {
                    self.levels[i + 1]
                }
                _ => continue,
            };
            let qty = self.config.quantity;
            match cell.side {
                Side::Buy if money >= limit * qty => money -= limit * qty,
                Side::Sell if available_shares >= qty => available_shares -= qty,
                _ => {
                    self.skipped += 1;
                    continue;
                }
            }
            let id = Uuid::new_v4().to_string();
            cell.pending = Some(id.clone());
            signals.push(Signal {
                id,
                strategy: self.id.clone(),
                symbol: self.config.symbol.clone(),
                name: self.name.clone(),
                cell: i,
                side: cell.side,
                price: limit,
                quantity: qty,
                quote: price,
                at,
                day: day.to_owned(),
            });
        }
        signals
    }

    // Filled orders flip this interval to its opposite boundary. Cancelling only releases the reservation.
    pub fn resolve(&mut self, s: &Signal, fill: Option<(i64, i64)>, today: &str) -> Result<()> {
        ensure!(
            self.cells.get(s.cell).and_then(|c| c.pending.as_ref()) == Some(&s.id),
            "订单已处理"
        );
        if let Some((price, fee)) = fill {
            ensure!(
                price > 0 && price <= 10_000_000 && (0..=price * s.quantity).contains(&fee),
                "成交价或总费用无效"
            );
            let gross = price * s.quantity;
            match s.side {
                Side::Buy => {
                    ensure!(price <= s.price, "买入价超出信号限价");
                    ensure!(
                        self.available(today).0 + s.price * s.quantity >= gross + fee,
                        "扣费后资金不足"
                    );
                    self.cash -= gross + fee;
                    self.shares += s.quantity;
                    if s.day.as_str() >= today {
                        *self.locked.entry(s.day.clone()).or_default() += s.quantity;
                    }
                    self.cells[s.cell].side = Side::Sell;
                }
                Side::Sell => {
                    ensure!(price >= s.price, "卖出价低于信号限价");
                    self.shares -= s.quantity;
                    self.cash += gross - fee;
                    self.cells[s.cell].side = Side::Buy;
                }
            }
        }
        self.cells[s.cell].pending = None;
        Ok(())
    }
}
