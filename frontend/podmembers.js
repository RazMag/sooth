// Wires the pod detail page's "N containers" button to open the <dialog>
// listing its members. Closes via the header's close button, native Escape,
// or a click on the backdrop (a plain click listener on the dialog itself,
// since the backdrop isn't a real child element -- a click that lands
// outside the dialog's own box still targets the dialog).

export function initPodMembersDialog() {
  const dialog = document.querySelector("[data-pod-members-dialog]");
  const openBtn = document.querySelector("[data-pod-members-open]");
  if (!dialog || !openBtn) return;

  if (!dialog.dataset.podMembersInit) {
    dialog.dataset.podMembersInit = "1";

    const closeBtn = dialog.querySelector("[data-pod-members-close]");
    if (closeBtn) closeBtn.addEventListener("click", () => dialog.close());

    dialog.addEventListener("click", (e) => {
      const rect = dialog.getBoundingClientRect();
      const inside =
        e.clientX >= rect.left &&
        e.clientX <= rect.right &&
        e.clientY >= rect.top &&
        e.clientY <= rect.bottom;
      if (!inside) dialog.close();
    });
  }

  if (!openBtn.dataset.podMembersInit) {
    openBtn.dataset.podMembersInit = "1";
    openBtn.addEventListener("click", () => dialog.showModal());
  }
}
