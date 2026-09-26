// Live journal tail. A `#log-output[data-log-stream data-stream-url="..."]`
// gets an EventSource whose messages are server-rendered `.log-line`
// fragments; they're appended and kept pinned to the bottom (unless the user
// has scrolled up). Replaces the hand-rolled inline <script> the logs page
// used to render.
//
// Lines are grouped into runs by `data-inv` (systemd invocation ID). When a
// line from a new run arrives, everything before it is marked `.log-old` and a
// divider is inserted -- the same marking the server applies to the initial
// tail -- so the "Latest run only" checkbox (`[data-log-latest]`, which just
// toggles `.latest-only`) hides the previous config's output live.
const LATEST_KEY = "sooth.logs.latestOnly";

const pad = (n) => String(n).padStart(2, "0");

// The server renders UTC; show the viewer's local time instead.
function localizeTimes(root) {
  root.querySelectorAll("time[datetime]").forEach((el) => {
    const d = new Date(el.dateTime);
    if (Number.isNaN(d.getTime())) return;
    el.textContent =
      `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
      `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
    el.title = el.dateTime;
  });
}

function runDivider(line) {
  const div = document.createElement("div");
  div.className = "log-run-divider";
  const span = document.createElement("span");
  span.append("New run · ");
  const time = line.querySelector("time");
  if (time) span.append(time.cloneNode(true));
  div.append(span);
  return div;
}

function readLatestOnly() {
  try {
    return localStorage.getItem(LATEST_KEY) !== "0";
  } catch {
    return true;
  }
}

function writeLatestOnly(on) {
  try {
    localStorage.setItem(LATEST_KEY, on ? "1" : "0");
  } catch {
    // Storage unavailable (private window etc.): the toggle still works.
  }
}

export function initLogs() {
  document.querySelectorAll("[data-log-stream]").forEach((out) => {
    if (out.dataset.logStreamInit) return;
    out.dataset.logStreamInit = "1";
    localizeTimes(out);
    out.scrollTop = out.scrollHeight;

    const es = new EventSource(out.dataset.streamUrl);
    es.onmessage = (e) => {
      const tpl = document.createElement("template");
      tpl.innerHTML = e.data;
      const line = tpl.content.firstElementChild;
      if (!line) return;
      localizeTimes(line);

      const pinned = out.scrollTop + out.clientHeight >= out.scrollHeight - 4;
      out.querySelector("[data-log-empty]")?.remove();
      const inv = line.dataset.inv;
      if (inv && inv !== out.dataset.latestInv) {
        if (out.dataset.latestInv) {
          out
            .querySelectorAll(".log-line, .log-run-divider")
            .forEach((el) => el.classList.add("log-old"));
          out.append(runDivider(line));
        }
        out.dataset.latestInv = inv;
      }
      out.append(line);
      if (pinned) out.scrollTop = out.scrollHeight;
    };

    const close = () => es.close();
    window.addEventListener("beforeunload", close);
    out.addEventListener("htmx:beforeCleanupElement", close);
  });

  document.querySelectorAll("[data-log-latest]").forEach((box) => {
    if (box.dataset.logLatestInit) return;
    box.dataset.logLatestInit = "1";
    const out = document.getElementById("log-output");
    if (!out) return;
    const apply = () => {
      out.classList.toggle("latest-only", box.checked);
      out.scrollTop = out.scrollHeight;
    };
    box.checked = readLatestOnly();
    apply();
    box.addEventListener("change", () => {
      writeLatestOnly(box.checked);
      apply();
    });
  });
}
