"use strict";
const $ = (id) => document.getElementById(id);
const money = (cents) =>
  (cents / 100).toLocaleString("zh-CN", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
const cents = (id) => Math.round(Number($(id).value) * 100);
const esc = (s) =>
  String(s ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const stamp = (at) =>
  new Intl.DateTimeFormat("zh-CN", {
    timeZone: "Asia/Shanghai",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(at * 1000);
let snapshot = null,
  currentQuote = null,
  draft = null,
  previewId = 0,
  quoteId = 0,
  quoteTimer,
  previewTimer,
  toastTimer,
  editing;
async function api(path, body) {
  const options =
    body === undefined
      ? {}
      : {
          method: "POST",
          headers: { "Content-Type": "application/json", "X-Grid-Action": "1" },
          body: JSON.stringify(body),
        };
  const r = await fetch("/api/" + path, options);
  const value = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(value.error || "请求失败（" + r.status + "）");
  return value;
}
function toast(msg) {
  $("toast").textContent = msg;
  $("toast").hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => ($("toast").hidden = true), 4500);
}
function config() {
  return {
    symbol: $("symbol").value.trim(),
    lower: cents("lower"),
    upper: cents("upper"),
    grids: Number($("grids").value),
    mode: document.querySelector("[name=mode]:checked").value,
    quantity: Number($("quantity").value),
    cash: cents("cash"),
    shares: Number($("shares").value),
    notify: $("notify").checked,
  };
}
async function lookup() {
  const seq = ++quoteId,
    symbol = $("symbol").value.trim();
  const star = symbol.startsWith("688"),
    beijing = symbol.startsWith("920");
  $("quantity").min = star ? 200 : 100;
  $("quantity").step = star || beijing ? 1 : 100;
  $("quantity").max = star
    ? 100000
    : /^(300|301)/.test(symbol)
      ? 300000
      : 1000000;
  if (star && Number($("quantity").value) === 100) $("quantity").value = 200;
  currentQuote = null;
  $("quote").textContent = "请输入六位 A 股代码";
  planPreview();
  if (!/^\d{6}$/.test(symbol)) return;
  $("quote").textContent = "查询最近行情…";
  try {
    const q = await api("quote/" + symbol);
    if (seq !== quoteId || symbol !== $("symbol").value.trim()) return;
    currentQuote = q;
    $("quote").textContent =
      q.name + "　¥" + money(q.price) + "　" + stamp(q.at);
    if (!$("lower").value)
      $("lower").value = (
        Math.max(1, Math.floor(q.price * 0.95)) / 100
      ).toFixed(2);
    if (!$("upper").value)
      $("upper").value = (Math.ceil(q.price * 1.05) / 100).toFixed(2);
    planPreview();
  } catch (e) {
    if (seq === quoteId) $("quote").textContent = e.message;
  }
}
function planPreview() {
  const seq = ++previewId;
  clearTimeout(previewTimer);
  previewTimer = setTimeout(async () => {
    const cfg = config();
    const quote =
      currentQuote?.symbol === cfg.symbol ? currentQuote.price : null;
    try {
      const result = await api("preview", { ...cfg, reference: quote });
      if (seq !== previewId) return;
      draft = { cfg, quote, levels: result.levels, cells: result.cells };
    } catch (e) {
      if (seq !== previewId) return;
      draft = { cfg, quote, levels: [], cells: [], error: e.message };
    }
    draw();
  }, 120);
}

function updateChartSources() {
  const select = $("chart-source");
  const value = select.value;
  const options = ['<option value="">参数预览</option>'];
  for (const { strategy: p } of snapshot.plans) {
    const label = p.name + " " + p.config.symbol + " · " + p.id.slice(0, 6);
    options.push(
      '<option value="' + esc(p.id) + '">' + esc(label) + "</option>",
    );
  }
  const html = options.join("");
  if (select.innerHTML !== html) {
    select.innerHTML = html;
    select.value = value;
  }
}

// A price may carry opposite sides from adjacent cells. Keep each signal on its own row.
function ladderRows(view, openOrders) {
  const rows = [],
    occupied = new Set();
  function add(price, kind, label, side = "", qty = null) {
    rows.push({ price, kind, label, side, qty });
    occupied.add(price);
  }
  for (const { signal } of openOrders) {
    add(signal.price, "pending", "待处理", signal.side, signal.quantity);
  }
  view.cells.forEach((cell, i) => {
    if (!cell.pending) {
      const level = view.levels[i + (cell.side === "sell" ? 1 : 0)];
      add(level, "planned", "计划", cell.side, view.cfg.quantity);
    }
  });
  if (view.quote != null) add(view.quote, "quote", "最近报价");
  for (const price of view.levels) {
    if (!occupied.has(price)) add(price, "reference", "网格价位");
  }
  return rows.sort((a, b) => b.price - a.price);
}
function draw() {
  const id = $("chart-source").value;
  const p = snapshot?.plans.find(
    ({ strategy }) => strategy.id === id,
  )?.strategy;
  const view = p
    ? {
        cfg: p.config,
        levels: p.levels,
        cells: p.cells,
        quote:
          p.last ??
          (currentQuote?.symbol === p.config.symbol
            ? currentQuote.price
            : null),
      }
    : draft;
  if (!view) return;
  const { cfg } = view;
  const openOrders = p
    ? snapshot.orders.filter(
        (o) => o.status === "open" && o.signal.strategy === p.id,
      )
    : [];
  $("chart-symbol").textContent = cfg.symbol || "—";
  $("preview-mode").textContent =
    cfg.mode === "arithmetic" ? "等差网格" : "等比网格";
  $("chart-range").textContent =
    view.error || "¥" + money(cfg.lower) + " – ¥" + money(cfg.upper);
  $("spread").textContent = view.error
    ? ""
    : cfg.grids +
      " 格 · " +
      (p
        ? openOrders.length + " 笔待处理"
        : cfg.mode === "arithmetic"
          ? "平均价差 ¥" +
            ((cfg.upper - cfg.lower) / 100 / cfg.grids).toFixed(2)
          : "每格约 " +
            (
              (Math.pow(cfg.upper / cfg.lower, 1 / cfg.grids) - 1) *
              100
            ).toFixed(2) +
            "%");
  const chart = $("chart");
  const rows = view.error ? [] : ladderRows(view, openOrders);
  const html = rows
    .map((r) => {
      const direction =
        r.side === "buy" ? "买入" : r.side === "sell" ? "卖出" : "";
      const text =
        r.label +
        direction +
        (r.qty != null ? " · " + r.qty.toLocaleString("zh-CN") + " 股" : "");
      return (
        '<div role="listitem" class="price-row ' +
        r.kind +
        " " +
        r.side +
        '"><i aria-hidden="true"></i><span class="order-label">' +
        text +
        '</span><b class="limit-price">¥' +
        money(r.price) +
        "</b></div>"
      );
    })
    .join("");
  const source = id || "draft-" + cfg.symbol;
  const changed = chart.dataset.source !== source;
  if (html !== chart.innerHTML) chart.innerHTML = html;
  if (changed && rows.length) {
    chart.dataset.source = source;
    const focus =
      chart.querySelector(".pending") || chart.querySelector(".quote");
    chart.scrollTop = focus
      ? focus.offsetTop - (chart.clientHeight - focus.offsetHeight) / 2
      : 0;
  }
}
const empty = (a, b) =>
  '<div class="empty"><strong>' + a + "</strong><p>" + b + "</p></div>";
const statusText = { open: "待处理", filled: "已成交", cancelled: "已取消" };
const deliveryText = {
  queued: "待推送",
  sent: "已推送",
  off: "仅本地",
  failed: "推送失败",
  expired: "推送已过期",
  closed: "已停止推送",
};
function paint() {
  const s = snapshot,
    open = s.orders.filter((o) => o.status === "open").length;
  $("num-active").textContent = s.plans.filter((p) => p.strategy.active).length;
  $("num-open").textContent = open;
  $("num-filled").textContent = s.filled_count;
  $("market").textContent = s.market_window ? "连续竞价时段" : "非连续竞价时段";
  $("channel").textContent = s.feishu ? "已配置" : "未配置";
  $("test").disabled = !s.feishu;
  $("notify").disabled = !s.feishu;
  $("plan-count").textContent = s.plans.length;
  $("plans").innerHTML =
    s.plans
      .map(
        ({ strategy: p, free_cash, free_shares }) =>
          '<div class="plan"><div><strong>' +
          esc(p.name) +
          "　" +
          esc(p.config.symbol) +
          '</strong>　<span class="pill">' +
          (p.active ? "运行中" : "已暂停") +
          "</span><small>" +
          p.config.grids +
          " 格 · ¥" +
          money(p.config.lower) +
          " – " +
          money(p.config.upper) +
          " · " +
          (p.config.notify ? "飞书推送" : "仅本地") +
          "</small><small>可用 ¥" +
          money(free_cash) +
          "　可卖 " +
          free_shares +
          " 股 · 余额不足跳过 " +
          p.skipped +
          " 格次</small><small>" +
          (p.last
            ? "上次报价 ¥" + money(p.last) + " · " + stamp(p.last_time)
            : "等待首个有效盘中报价") +
          "　" +
          esc(s.market_window ? s.health[p.config.symbol] : "") +
          '</small></div><button class="light" data-plan="' +
          esc(p.id) +
          '" data-active="' +
          !p.active +
          '">' +
          (p.active ? "暂停" : "恢复") +
          "</button></div>",
      )
      .join("") || empty("还没有策略", "设置左侧参数，启动第一组网格。");
  orders();
  updateChartSources();
  draw();
}
function orders() {
  const filter = $("filter").value;
  const entries = (snapshot?.orders || []).filter(
    (o) => filter === "all" || o.status === filter,
  );
  if (!entries.length) {
    $("orders").innerHTML = empty(
      "暂无" + (filter === "all" ? "" : statusText[filter]) + "订单",
      "行情触达网格后，订单将显示在这里。",
    );
    return;
  }
  const rows = entries.map(
    ({ signal: s, status, delivery, error, fill, fee }) =>
      "<tr><td>" +
      stamp(s.at) +
      "</td><td>" +
      esc(s.name) +
      '<span class="sub">' +
      esc(s.symbol) +
      '</span></td><td class="' +
      s.side +
      '">' +
      (s.side === "buy" ? "买入" : "卖出") +
      "</td><td>¥" +
      money(s.price) +
      '<span class="sub">第 ' +
      (s.cell + 1) +
      " 格</span></td><td>" +
      s.quantity +
      " 股</td><td>" +
      statusText[status] +
      (fill != null
        ? '<span class="sub">¥' +
          money(fill) +
          " · 费用 ¥" +
          money(fee) +
          "</span>"
        : "") +
      '</td><td title="' +
      esc(error) +
      '">' +
      deliveryText[delivery] +
      "</td><td>" +
      (status === "open"
        ? '<button class="order-btn" data-fill="' +
          s.id +
          '">登记成交</button><button class="order-btn" data-cancel="' +
          s.id +
          '">取消</button>'
        : "—") +
      "</td></tr>",
  );
  $("orders").innerHTML =
    '<div class="scroll"><table><thead><tr>' +
    ["行情时间", "股票", "方向", "信号限价", "数量", "状态", "飞书", "操作"]
      .map((x) => "<th>" + x + "</th>")
      .join("") +
    "</tr></thead><tbody>" +
    rows.join("") +
    "</tbody></table></div>";
}
let refreshSeq = 0;
const gridFields = [
  "symbol",
  "lower",
  "upper",
  "grids",
  "quantity",
  "cash",
  "shares",
  "filter",
];
let gridDraft = null;
try {
  gridDraft = JSON.parse(sessionStorage.getItem("gejian.draft.grid") || "null");
  if (gridDraft) {
    for (const id of gridFields)
      if (typeof gridDraft[id] === "string") $(id).value = gridDraft[id];
    for (const radio of document.querySelectorAll("[name=mode]"))
      radio.checked = radio.value === gridDraft.mode;
  }
} catch {
  gridDraft = null;
}
window.addEventListener("pagehide", () => {
  const data = Object.fromEntries(gridFields.map((id) => [id, $(id).value]));
  data.mode =
    document.querySelector("[name=mode]:checked")?.value || "arithmetic";
  data.notify = $("notify").checked;
  data.chart = $("chart-source").value;
  try {
    sessionStorage.setItem("gejian.draft.grid", JSON.stringify(data));
  } catch {}
});
async function refresh() {
  const seq = ++refreshSeq;
  try {
    const value = await api("state");
    if (seq !== refreshSeq) return;
    const first = !snapshot;
    snapshot = value;
    if (first) {
      $("notify").checked = value.feishu && (gridDraft?.notify ?? true);
      updateChartSources();
      const open = value.orders.find((o) => o.status === "open");
      $("chart-source").value = gridDraft?.chart ?? open?.signal.strategy ?? "";
    }
    paint();
  } catch (e) {
    if (seq === refreshSeq) $("market").textContent = "服务连接异常";
  }
}
$("symbol").addEventListener("input", () => {
  clearTimeout(quoteTimer);
  quoteId++;
  currentQuote = null;
  $("lower").value = "";
  $("upper").value = "";
  quoteTimer = setTimeout(lookup, 300);
});
$("chart-source").addEventListener("change", draw);
$("form").addEventListener("input", () => {
  $("chart-source").value = "";
  planPreview();
});
$("form").addEventListener("submit", async (e) => {
  e.preventDefault();
  $("create").disabled = true;
  $("form-note").textContent = "";
  try {
    const result = await api("plans", config());
    toast("策略已启动");
    await refresh();
    $("chart-source").value = result.id;
    draw();
  } catch (err) {
    $("form-note").textContent = err.message;
  } finally {
    $("create").disabled = false;
  }
});
$("test").addEventListener("click", async () => {
  const b = $("test");
  b.disabled = true;
  try {
    await api("feishu/test", {});
    toast("飞书已接收测试消息");
  } catch (e) {
    toast(e.message);
  } finally {
    b.disabled = !snapshot?.feishu;
  }
});
$("plans").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-plan]");
  if (!b) return;
  b.disabled = true;
  try {
    await api("plans/" + b.dataset.plan + "/active", {
      active: b.dataset.active === "true",
    });
    await refresh();
  } catch (err) {
    toast(err.message);
    b.disabled = false;
  }
});
$("filter").addEventListener("change", orders);
$("orders").addEventListener("click", (e) => {
  const b = e.target.closest("[data-fill],[data-cancel]");
  if (!b) return;
  const id = b.dataset.fill || b.dataset.cancel,
    s = snapshot.orders.find((o) => o.signal.id === id).signal;
  editing = { id, action: b.dataset.fill ? "fill" : "cancel" };
  $("dialog-title").textContent = b.dataset.fill
    ? "登记当日整笔成交"
    : "取消订单信号";
  $("dialog-desc").textContent =
    s.day +
    "　" +
    s.name +
    " " +
    s.symbol +
    "　" +
    (s.side === "buy" ? "买入" : "卖出") +
    " " +
    s.quantity +
    " 股，限价 ¥" +
    money(s.price) +
    "。" +
    (b.dataset.fill
      ? "请按信号当日的实际成交回报登记。"
      : "请确认这笔订单尚未成交。");
  $("fill-fields").hidden = !b.dataset.fill;
  $("fill-fields").style.display = b.dataset.fill ? "grid" : "none";
  $("fill-price").required = !!b.dataset.fill;
  $("fill-price").value = (s.price / 100).toFixed(2);
  $("fee").value = "0";
  $("dialog-note").textContent = "";
  $("dialog").showModal();
});
$("dismiss").onclick = () => $("dialog").close();
$("resolve-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  $("commit").disabled = true;
  try {
    await api("orders/" + editing.id, {
      action: editing.action,
      price: cents("fill-price"),
      fee: cents("fee"),
    });
    $("dialog").close();
    await refresh();
  } catch (err) {
    $("dialog-note").textContent = err.message;
  } finally {
    $("commit").disabled = false;
  }
});
function clock() {
  $("clock").textContent = new Date().toLocaleTimeString("zh-CN", {
    timeZone: "Asia/Shanghai",
    hour12: false,
  });
}
clock();
setInterval(clock, 1000);
refresh();
setInterval(refresh, 5000);
lookup();
