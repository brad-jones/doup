#!/usr/bin/env bun
// Renders each `*.bench.json` (see bench-to-json.ts) into a self-contained
// `*.bench.html` next to it. The JSON is inlined (minified); Chart.js is loaded from a CDN.
//
// Usage: bun scripts/bench-to-html.ts [dir]   (default: projects/galvanize-rs/benches)
import { Glob } from "bun";
import path from "node:path";

const dir = process.argv[2] ?? "projects/galvanize-rs/benches";

const TEMPLATE = /* html */ `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Benchmark: __NAME__</title>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.7/dist/chart.umd.min.js"></script>
<style>
  :root { color-scheme: light dark; --bg:#fff; --fg:#1b1f24; --card:#f6f8fa; --bd:#d0d7de; --mut:#656d76; }
  @media (prefers-color-scheme: dark) { :root { --bg:#0d1117; --fg:#e6edf3; --card:#161b22; --bd:#30363d; --mut:#8b949e; } }
  body { font: 14px/1.5 system-ui, sans-serif; background: var(--bg); color: var(--fg); margin: 0; padding: 24px; max-width: 1200px; margin-inline: auto; }
  h1 { margin: 0 0 4px; } h2 { margin: 32px 0 8px; }
  .card { background: var(--card); border: 1px solid var(--bd); border-radius: 8px; padding: 16px; margin: 12px 0; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 8px 24px; }
  .grid div span { display: block; color: var(--mut); font-size: 12px; }
  .controls { display: flex; gap: 16px; align-items: center; flex-wrap: wrap; }
  .chart { position: relative; height: 420px; }
  table { border-collapse: collapse; width: 100%; font-variant-numeric: tabular-nums; }
  th, td { padding: 4px 8px; border-bottom: 1px solid var(--bd); text-align: right; }
  th:first-child, td:first-child { text-align: left; }
  details summary { cursor: pointer; font-weight: 600; }
  .mut { color: var(--mut); }
</style>
</head>
<body>
<h1>Benchmark: <span id="title"></span></h1>
<div class="mut" id="subtitle"></div>
<div class="card"><div class="grid" id="machine"></div></div>
<div class="card controls">
  <label>Metric
    <select id="metric">
      <option value="median">median</option><option value="mean">mean</option>
      <option value="fastest">fastest</option><option value="slowest">slowest</option>
    </select>
  </label>
  <label><input type="checkbox" id="log" checked> log scale</label>
  <span class="mut">Lower is better. Hover bars for fastest / median / mean / slowest.</span>
</div>
<div id="groups"></div>
<script id="data" type="application/json">__DATA__</script>
<script>
const data = JSON.parse(document.getElementById("data").textContent);
const PALETTE = ["#4e79a7","#f28e2b","#e15759","#76b7b2","#59a14f","#edc948","#b07aa1","#ff9da7","#9c755f","#bab0ac"];

function fmt(ns) {
  if (ns == null) return "–";
  const units = [["s",1e9],["ms",1e6],["µs",1e3],["ns",1],["ps",1e-3]];
  for (const [u, f] of units) if (ns >= f) return +(ns / f).toPrecision(4) + " " + u;
  return +(ns * 1000).toPrecision(4) + " ps";
}

document.getElementById("title").textContent = data.benchmark;
document.getElementById("subtitle").textContent =
  "Generated " + data.generatedAt + (data.gitCommit ? " @ " + data.gitCommit.slice(0, 10) : "") +
  (data.filter ? " (filter: " + data.filter + ")" : "") +
  (data.timerPrecision ? " · timer precision " + data.timerPrecision : "");

const m = data.machine;
const facts = [
  ["CPU", m.cpu.model], ["Logical cores", m.cpu.logicalCores],
  ["Memory", m.memory.totalGiB + " GiB"], ["OS", m.os], ["Arch", m.arch],
  ["Rust", (m.rustc || "").split("\\n")[0]], ["Divan", m.divan],
];
document.getElementById("machine").innerHTML = facts
  .map(([k, v]) => "<div><span></span></div>").join("");
[...document.querySelectorAll("#machine div")].forEach((d, i) => {
  d.firstChild.textContent = facts[i][0];
  d.append(String(facts[i][1] ?? "–"));
});

// Group by top-level node. Within a group, series = 2nd path segment (eg. the design / pointer
// type) and x categories = the remaining segments joined (eg. "Beagle / t=1").
const groups = new Map();
for (const r of data.results) {
  const g = r.path[0];
  if (!groups.has(g)) groups.set(g, []);
  groups.get(g).push(r);
}

const charts = [];
const root = document.getElementById("groups");
for (const [name, rows] of groups) {
  const series = [...new Set(rows.map(r => r.path[1] ?? name))];
  const labels = [...new Set(rows.map(r => r.path.slice(2).join(" / ") || "—"))];
  const lookup = new Map(rows.map(r => [(r.path[1] ?? name) + "\\0" + (r.path.slice(2).join(" / ") || "—"), r]));

  const sec = document.createElement("section");
  sec.innerHTML = "<h2></h2><div class='card'><div class='chart'><canvas></canvas></div></div>" +
    "<details><summary>Raw numbers</summary><div class='card'><table></table></div></details>";
  sec.querySelector("h2").textContent = name;
  root.append(sec);

  const table = sec.querySelector("table");
  table.innerHTML = "<tr><th>Benchmark</th><th>fastest</th><th>median</th><th>mean</th><th>slowest</th><th>samples</th><th>iters</th></tr>";
  for (const r of rows) {
    const tr = table.insertRow();
    tr.insertCell().textContent = r.path.slice(1).join(" / ") || name;
    for (const k of ["fastest","median","mean","slowest"]) tr.insertCell().textContent = fmt(r[k]);
    tr.insertCell().textContent = r.samples; tr.insertCell().textContent = r.iters;
  }

  const chart = new Chart(sec.querySelector("canvas"), {
    type: "bar",
    data: {
      labels,
      datasets: series.map((s, i) => ({
        label: s,
        backgroundColor: PALETTE[i % PALETTE.length],
        _rows: labels.map(l => lookup.get(s + "\\0" + l)),
        data: [],
      })),
    },
    options: {
      responsive: true, maintainAspectRatio: false,
      scales: { y: { type: "logarithmic", title: { display: true, text: "time per iteration (ns)" } } },
      plugins: {
        legend: { display: series.length > 1 || series[0] !== name },
        tooltip: { callbacks: { label: ctx => {
          const r = ctx.dataset._rows[ctx.dataIndex];
          return r ? ctx.dataset.label + ": fastest " + fmt(r.fastest) + " · median " + fmt(r.median) +
            " · mean " + fmt(r.mean) + " · slowest " + fmt(r.slowest) : ctx.dataset.label;
        } } },
      },
    },
  });
  charts.push(chart);
}

function update() {
  const metric = document.getElementById("metric").value;
  const log = document.getElementById("log").checked;
  for (const c of charts) {
    c.options.scales.y.type = log ? "logarithmic" : "linear";
    for (const ds of c.data.datasets) ds.data = ds._rows.map(r => r ? r[metric] : null);
    c.update();
  }
}
document.getElementById("metric").onchange = update;
document.getElementById("log").onchange = update;
update();
</script>
</body>
</html>
`;

for await (const file of new Glob("*.bench.json").scan(dir)) {
  const jsonPath = path.join(dir, file);
  const data = JSON.parse(await Bun.file(jsonPath).text());
  // Escape `<` so inlined JSON can never terminate the script tag.
  const inline = JSON.stringify(data).replaceAll("<", "\\u003c");
  const html = TEMPLATE
    .replace("__NAME__", () => data.benchmark)
    .replace("__DATA__", () => inline);
  const out = jsonPath.replace(/\.json$/, ".html");
  await Bun.write(out, html);
  console.log(`bench-to-html: wrote ${out}`);
}
