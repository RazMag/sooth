// The Settings page's single Save action: the button doubles as the
// "unsaved changes" indicator (starts disabled, only becomes clickable once
// the form actually differs from how the page loaded), and a banner up near
// the top mirrors that same state for anyone who hasn't scrolled down to the
// button yet. Both compare against a snapshot rather than just latching on
// the first edit, so reverting a change (e.g. undoing a typo, or flipping a
// checkbox back) correctly clears them again too.
//
// Both changes happen here in JS, not in the server-rendered HTML, so a page
// load with JS broken or disabled still leaves the button usable -- this is
// progressive enhancement on top of a form that already works without it,
// not a gate on the primary action.
//
// Listens on `document` and matches by `event.target.form` rather than
// scoping to `#settings-form`'s own subtree, so this keeps working even for
// a field that ends up associated with the form via a `form="settings-form"`
// attribute instead of DOM nesting (nothing does today, but the Updates
// card's fields came close to needing that -- see `selfupdate.rs`).
function snapshot(form) {
  const parts = [];
  for (const el of form.elements) {
    if (!el.name) continue;
    if ((el.type === "radio" || el.type === "checkbox") && !el.checked) continue;
    parts.push(el.name + "=" + el.value);
  }
  return parts.join("&");
}

export function initSettingsForm() {
  const form = document.getElementById("settings-form");
  const saveButton = document.getElementById("settings-save");
  const banner = document.getElementById("settings-unsaved-banner");
  if (!form || !saveButton || initSettingsForm._wired === form) return;
  initSettingsForm._wired = form;

  const initial = snapshot(form);
  saveButton.disabled = true;

  const update = (e) => {
    if (e.target.form !== form) return;
    const dirty = snapshot(form) !== initial;
    saveButton.disabled = !dirty;
    if (banner) banner.hidden = !dirty;
  };
  document.addEventListener("input", update);
  document.addEventListener("change", update);

  // Bug: edit a field, navigate away without saving, then come back -- the
  // browser can restore this exact page (edits and all) from its
  // back/forward cache instead of asking the server again, so what's on
  // screen looks like the real saved config when it's actually just the
  // abandoned edit. `pageshow`'s `persisted` flag is true precisely when the
  // page came back from that cache rather than a fresh load; forcing a
  // reload there guarantees this page always reflects what's actually
  // persisted, at the cost of that instant-back-navigation feel.
  window.addEventListener("pageshow", (e) => {
    if (e.persisted) location.reload();
  });
}
