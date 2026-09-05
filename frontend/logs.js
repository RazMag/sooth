// Live journal tail. A `<pre data-log-stream data-stream-url="...">` gets an
// EventSource that appends lines and keeps it pinned to the bottom (unless the
// user has scrolled up). Replaces the hand-rolled inline <script> the logs
// page used to render.
export function initLogs() {
  document.querySelectorAll("[data-log-stream]").forEach((pre) => {
    if (pre.dataset.logStreamInit) return;
    pre.dataset.logStreamInit = "1";

    const es = new EventSource(pre.dataset.streamUrl);
    es.onmessage = (e) => {
      const pinned = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 4;
      pre.textContent += e.data + "\n";
      if (pinned) pre.scrollTop = pre.scrollHeight;
    };

    const close = () => es.close();
    window.addEventListener("beforeunload", close);
    pre.addEventListener("htmx:beforeCleanupElement", close);
  });
}
