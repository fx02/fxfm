// F3 viewer window: text / image / PDF / hex.
import { formatViewStatus, hexLines, groupThousands } from "./lib.mjs";

const T = window.__TAURI__;
const invoke = T.core.invoke;
const win = T.window.getCurrentWindow();
const path = new URLSearchParams(location.search).get("path") ?? "";

const bodyEl = document.getElementById("vbody");
const statusEl = document.getElementById("vstatus");
const findEl = document.getElementById("vfind");
const findInput = document.getElementById("vfindinput");

const ENCODINGS = new Map([
  ["1", "UTF-8"],
  ["2", "windows-1250"],
  ["3", "ISO-8859-2"],
]);
let isText = false;

function setStatus(s) {
  statusEl.textContent = s;
}

function showText(c) {
  isText = true;
  const pre = document.createElement("pre");
  pre.className = "vtext";
  pre.textContent = c.text;
  bodyEl.replaceChildren(pre);
  setStatus(formatViewStatus(c.encoding, c.total, c.truncated));
}

function showHex(c) {
  isText = false;
  const pre = document.createElement("pre");
  pre.className = "vtext";
  pre.textContent = hexLines(c.bytes, c.offset).join("\n");
  bodyEl.replaceChildren(pre);
  const shown = c.bytes.length < c.total ? ` (first ${groupThousands(c.bytes.length)})` : "";
  setStatus(`Hex · ${groupThousands(c.total)} bytes${shown}`);
}

function showImage() {
  isText = false;
  const img = document.createElement("img");
  img.className = "vimage";
  img.src = T.core.convertFileSrc(path);
  img.addEventListener("error", () => setStatus("Cannot load image"));
  bodyEl.replaceChildren(img);
  setStatus(path);
}

function showPdf() {
  isText = false;
  const bar = document.createElement("div");
  bar.className = "vbar";
  const btn = document.createElement("button");
  btn.type = "button";
  btn.textContent = "Open in Quick Look";
  btn.addEventListener("click", () => invoke("quick_look", { path }).catch((err) => setStatus(String(err))));
  bar.append(btn);
  const f = document.createElement("iframe");
  f.className = "vpdf";
  f.src = T.core.convertFileSrc(path);
  bodyEl.replaceChildren(bar, f);
  setStatus(path);
}

function render(c) {
  switch (c.kind) {
    case "text": return showText(c);
    case "image": return showImage();
    case "pdf": return showPdf();
    case "hex": return showHex(c);
    default: setStatus(`Unsupported content: ${c.kind}`);
  }
}

let statusBeforeFind = "";

function openFind() {
  statusBeforeFind = statusEl.textContent === "Not found" ? statusBeforeFind : statusEl.textContent;
  findEl.hidden = false;
  findInput.focus();
  findInput.select();
}

function closeFind() {
  findEl.hidden = true;
  findInput.blur();
}

document.addEventListener("keydown", (e) => {
  if (e.target === findInput) {
    if (e.key === "Escape") {
      e.preventDefault();
      closeFind();
    } else if (e.key === "Enter") {
      e.preventDefault();
      setStatus(window.find(findInput.value, false, e.shiftKey, true) ? statusBeforeFind : "Not found");
    }
    return;
  }
  const close = e.key === "Escape" || e.code === "F3" || (e.ctrlKey && e.code === "KeyW");
  if (close) {
    e.preventDefault();
    win.close();
  } else if (e.ctrlKey && e.code === "KeyF") {
    e.preventDefault();
    if (isText) openFind();
  } else if (isText && !e.ctrlKey && !e.metaKey && !e.altKey && ENCODINGS.has(e.key)) {
    e.preventDefault();
    load(invoke("view_text_as", { path, encoding: ENCODINGS.get(e.key) }));
  }
});

/** Shows "Loading…", then renders after a frame so the window paints first. */
function load(promise) {
  setStatus("Loading…");
  promise
    .then((c) => new Promise((r) => requestAnimationFrame(() => r(c))))
    .then(render, (err) => setStatus(`Cannot view: ${err}`));
}

load(invoke("view_content", { path }));
