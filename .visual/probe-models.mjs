import https from "node:https";
import fs from "node:fs";

const BASE = "https://maas.qianwenaiapi.com/compatible-mode/v1";
const KEY = process.env.QW_KEY;
const img = fs.readFileSync(".visual/_probe.png");
const b64 = img.toString("base64");

function reqPost(path, headers, bodyBuf) {
  return new Promise((resolve) => {
    const u = new URL(BASE + path);
    const req = https.request(
      { hostname: u.hostname, path: u.pathname, method: "POST", headers },
      (r) => {
        let d = "";
        r.on("data", (c) => (d += c));
        r.on("end", () => resolve({ status: r.statusCode, body: d }));
      }
    );
    req.on("error", (e) => resolve({ status: 0, body: "ERR " + e.message }));
    req.setTimeout(120000, () => { req.destroy(); resolve({ status: 0, body: "TIMEOUT" }); });
    req.write(bodyBuf);
    req.end();
  });
}

function multipart(fields, filename, fileBuf) {
  const b = "----pms" + Date.now();
  const parts = [];
  for (const [k, v] of Object.entries(fields)) {
    parts.push(Buffer.from(`--${b}\r\nContent-Disposition: form-data; name="${k}"\r\n\r\n${v}\r\n`));
  }
  parts.push(Buffer.from(`--${b}\r\nContent-Disposition: form-data; name="image"; filename="${filename}"\r\nContent-Type: image/png\r\n\r\n`));
  parts.push(fileBuf);
  parts.push(Buffer.from(`\r\n--${b}--\r\n`));
  return { contentType: `multipart/form-data; boundary=${b}`, body: Buffer.concat(parts) };
}

async function testEdit(model) {
  const mp = multipart({ model, prompt: "Turn this into a clean pencil sketch, no text.", n: "1" }, "probe.png", img);
  const r = await reqPost("/images/edits", { Authorization: "Bearer " + KEY, "Content-Type": mp.contentType }, mp.body);
  let info = "";
  try {
    const j = JSON.parse(r.body);
    if (j.data) info = "data=" + j.data.length + " keys=" + Object.keys(j.data[0] || {}).join(",");
    else info = JSON.stringify(j).slice(0, 220);
  } catch { info = r.body.slice(0, 220); }
  console.log(`EDIT ${model} -> ${r.status} | ${info}`);
}

async function testVision(model) {
  const body = JSON.stringify({
    model,
    messages: [{ role: "user", content: [
      { type: "text", text: "用一句中文描述这张图。" },
      { type: "image_url", image_url: { url: "data:image/png;base64," + b64 } },
    ] }],
    temperature: 0.3,
  });
  const r = await reqPost("/chat/completions", { Authorization: "Bearer " + KEY, "Content-Type": "application/json" }, Buffer.from(body));
  let info = "";
  try { const j = JSON.parse(r.body); info = (j.choices?.[0]?.message?.content || JSON.stringify(j)).toString().slice(0, 160); }
  catch { info = r.body.slice(0, 160); }
  console.log(`VISION ${model} -> ${r.status} | ${info}`);
}

async function testGen(model) {
  const body = JSON.stringify({ model, prompt: "a calm ink-wash landscape, minimalist", n: 1 });
  const r = await reqPost("/images/generations", { Authorization: "Bearer " + KEY, "Content-Type": "application/json" }, Buffer.from(body));
  let info = "";
  try { const j = JSON.parse(r.body); info = j.data ? ("data=" + j.data.length + " keys=" + Object.keys(j.data[0] || {}).join(",")) : JSON.stringify(j).slice(0, 220); }
  catch { info = r.body.slice(0, 220); }
  console.log(`GEN ${model} -> ${r.status} | ${info}`);
}

async function testGenJsonImg(model) {
  const body = JSON.stringify({ model, prompt: "Turn this into a clean pencil sketch, no text.", n: 1, image: "data:image/png;base64," + b64 });
  const r = await reqPost("/images/generations", { Authorization: "Bearer " + KEY, "Content-Type": "application/json" }, Buffer.from(body));
  let info = "";
  try { const j = JSON.parse(r.body); info = j.data ? ("data=" + j.data.length) : JSON.stringify(j).slice(0, 220); }
  catch { info = r.body.slice(0, 220); }
  console.log(`GENJSONIMG ${model} -> ${r.status} | ${info}`);
}

async function testChatImg(model) {
  const body = JSON.stringify({
    model,
    messages: [{ role: "user", content: [
      { type: "text", text: "把这张图改成干净的铅笔素描风格，只输出图片。" },
      { type: "image_url", image_url: { url: "data:image/png;base64," + b64 } },
    ] }],
  });
  const r = await reqPost("/chat/completions", { Authorization: "Bearer " + KEY, "Content-Type": "application/json" }, Buffer.from(body));
  let info = "";
  try { const j = JSON.parse(r.body); info = JSON.stringify(j).slice(0, 240); } catch { info = r.body.slice(0, 240); }
  console.log(`CHATIMG ${model} -> ${r.status} | ${info}`);
}

async function testGenImg(model) {
  const mp = multipart({ model, prompt: "Turn this into a clean pencil sketch, no text.", n: "1" }, "probe.png", img);
  const r = await reqPost("/images/generations", { Authorization: "Bearer " + KEY, "Content-Type": mp.contentType }, mp.body);
  let info = "";
  try { const j = JSON.parse(r.body); info = j.data ? ("data=" + j.data.length) : JSON.stringify(j).slice(0, 220); }
  catch { info = r.body.slice(0, 220); }
  console.log(`GENIMG ${model} -> ${r.status} | ${info}`);
}

const which = process.argv[2] || "all";
if (which === "vision") {
  const list = process.argv.length > 3 ? process.argv.slice(3) : ["qwen3-vl-plus", "qwen3.5-omni-plus"];
  for (const m of list) await testVision(m);
} else if (which === "edit") {
  for (const m of process.argv.slice(3)) await testEdit(m);
} else if (which === "gen") {
  for (const m of process.argv.slice(3)) await testGen(m);
} else if (which === "genjsonimg") {
  for (const m of process.argv.slice(3)) await testGenJsonImg(m);
} else if (which === "chatimg") {
  for (const m of process.argv.slice(3)) await testChatImg(m);
} else if (which === "genimg") {
  for (const m of process.argv.slice(3)) await testGenImg(m);
} else {
  await testVision("qwen3-vl-plus");
  for (const m of ["qwen-image-2.0", "qwen-image-3.0", "wan2.7-image", "z-image-turbo"]) await testEdit(m);
}
