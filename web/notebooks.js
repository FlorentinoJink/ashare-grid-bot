"use strict";
const section = document.body.dataset.section;
const dividendPage = section === "dividends";
const form = document.getElementById("entry");
const endpoint = "/api/notebooks/" + section;
const $ = (id) => document.getElementById(id);
const decimal = new Intl.NumberFormat("zh-CN", {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
const money = (n) => "¥" + decimal.format(n);
const percent = (n) => decimal.format(n) + "%";
const draftKey = "gejian.draft." + section;
let entries = [];

function values() {
  const data = {};
  for (const f of form.elements) {
    if (f.name)
      data[f.name] =
        f.type === "number"
          ? Number.isFinite(f.valueAsNumber)
            ? f.valueAsNumber
            : null
          : f.value.trim();
  }
  return data;
}
function restore(data) {
  for (const f of form.elements)
    if (f.name && Object.hasOwn(data, f.name)) f.value = data[f.name] ?? "";
}
function remember() {
  try {
    sessionStorage.setItem(draftKey, JSON.stringify(values()));
  } catch {}
}
function dividendMetrics(d) {
  return {
    yield: (d.dividend / d.price) * 100,
    watch: (d.dividend * 100) / d.target,
    income: d.dividend * d.shares,
  };
}
function allocationMetrics(d) {
  const total = d.equity + d.bonds;
  const weight = (100 * d.equity) / total;
  return {
    total,
    weight,
    gap: weight - d.target,
    change: Math.round(((total * d.target) / 100 - d.equity) * 100) / 100,
  };
}
function update() {
  const d = values();
  const ready = [...form.querySelectorAll('input[type="number"]')]
    .filter((f) => f.name !== "year")
    .every((f) => f.validity.valid);
  let note = "";
  let result = {};
  if (ready && dividendPage) {
    const m = dividendMetrics(d);
    result = {
      yield: percent(m.yield),
      watch: money(m.watch),
      income: money(m.income),
    };
    note =
      d.basis === "预案"
        ? "采用预案分红，金额以最终实施公告为准。"
        : "收息按输入持仓估算，实际金额以账户到账为准。";
  } else if (ready && !dividendPage && d.equity + d.bonds > 0) {
    const m = allocationMetrics(d);
    result = {
      total: money(m.total),
      weight: percent(m.weight),
      gap: (m.gap > 0 ? "+" : "") + decimal.format(m.gap) + " 个百分点",
      change:
        (m.change > 0 ? "+" : m.change < 0 ? "−" : "") +
        money(Math.abs(m.change)),
    };
    note =
      m.change === 0
        ? "按当前市值，权益占比已达到目标。"
        : "按组合总市值不变测算，债券市值对应" +
          (m.change > 0 ? "减少 " : "增加 ") +
          money(Math.abs(m.change)) +
          "。";
  } else if (ready && !dividendPage) note = "两类资产至少有一项市值须大于零。";
  for (const el of document.querySelectorAll(".result dd"))
    el.textContent = result[el.id] ?? "—";
  $("result-note").textContent = note;
}
function draw() {
  const body = $("records").tBodies[0];
  body.replaceChildren();
  for (const entry of entries) {
    const d = entry.data;
    let fields;
    if (dividendPage) {
      const m = dividendMetrics(d);
      fields = [d.symbol, d.year, d.basis, percent(m.yield), money(m.watch)];
    } else {
      const m = allocationMetrics(d);
      fields = [d.name, money(m.total), percent(m.weight), percent(d.target)];
    }
    const row = body.insertRow();
    for (const value of fields) row.insertCell().textContent = value;
    const button = document.createElement("button");
    button.className = "order-btn";
    button.type = "button";
    button.textContent = "编辑";
    button.addEventListener("click", () => {
      restore(d);
      remember();
      update();
      $("save-status").textContent = "";
      form.elements[0].focus();
    });
    row.insertCell().append(button);
  }
  $("count").textContent = entries.length;
  $("records").hidden = !entries.length;
  $("empty").hidden = !!entries.length;
}
async function load() {
  const r = await fetch(endpoint);
  if (!r.ok) throw new Error("记录读取失败，请刷新重试。");
  entries = await r.json();
  draw();
}
form.addEventListener("input", () => {
  remember();
  update();
  $("save-status").textContent = "";
});
form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const body = values();
  if (!dividendPage && body.equity + body.bonds <= 0) {
    $("save-status").textContent = "组合总市值须大于零。";
    return;
  }
  $("save").disabled = true;
  try {
    const r = await fetch(endpoint, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Grid-Action": "1" },
      body: JSON.stringify(body),
    });
    const record = await r.json();
    if (!r.ok) throw new Error(record.error || "保存失败");
    entries = entries.filter((e) => e.key !== record.key).concat(record);
    entries.sort((a, b) => a.key.localeCompare(b.key, "zh-CN"));
    draw();
    remember();
    $("save-status").textContent = "已保存";
  } catch (e) {
    $("save-status").textContent = e.message;
  } finally {
    $("save").disabled = false;
  }
});
try {
  restore(JSON.parse(sessionStorage.getItem(draftKey) || "{}"));
} catch {}
update();
load().catch((err) => {
  $("empty").textContent = err.message;
});
