// Turns published host-port numbers into links to the same host on that
// port. The dashboard is server-rendered and can't know the hostname the
// browser used to reach it, so the href is filled in here from `location`
// -- except for another machine's ports, which carry its host in
// `data-host`.
// Idempotent (marks what it has done with `data-port-linked`) and re-run
// after every htmx swap.
export function initPortLinks() {
  const scheme = location.protocol === "https:" ? "https:" : "http:";
  const here = location.hostname;

  for (const el of document.querySelectorAll(
    "[data-host-port]:not([data-port-linked])",
  )) {
    el.dataset.portLinked = "1";
    const port = el.dataset.hostPort;
    const name = el.dataset.host || here;
    if (!port || !name) continue;
    // An IPv6 address goes in brackets in a URL (location.hostname already
    // has them).
    const host = name.includes(":") && !name.startsWith("[") ? `[${name}]` : name;

    const a = document.createElement("a");
    a.className = "port-link";
    a.href = `${scheme}//${host}:${port}`;
    a.target = "_blank";
    a.rel = "noopener";
    a.title = `Open ${host}:${port} in a new tab`;
    a.textContent = el.textContent;
    el.replaceWith(a);
  }
}
