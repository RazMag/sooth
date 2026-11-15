// Unit detail tabs (`templates::detail`): the radios already switch panels
// through CSS. This only keeps the open tab in the URL hash, so a reload,
// a shared link or `.../#logs` opens the same tab.
export function initTabs() {
  const open = () => {
    const id = location.hash.slice(1);
    const radio = id && document.querySelector(`input.tab-radio[value="${CSS.escape(id)}"]`);
    if (radio) radio.checked = true;
  };
  open();
  window.addEventListener("hashchange", open);
  document.addEventListener("change", (e) => {
    if (!e.target.matches("input.tab-radio")) return;
    const hash = e.target.value === "overview" ? " " : `#${e.target.value}`;
    history.replaceState(null, "", hash === " " ? location.pathname + location.search : hash);
  });
}
