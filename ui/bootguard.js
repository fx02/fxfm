// The main window starts hidden and app.js shows it when the UI is ready. If start-up throws
// first, the app would sit in the Dock with no window; this shows the window with the error.
(() => {
  let reported = false;
  function report(what) {
    if (reported || window.__fxfmBooted) return;
    reported = true;
    const box = document.createElement("pre");
    box.className = "boot-error";
    box.textContent = `fxfm could not start:\n\n${what}`;
    const show = () => {
      document.body.prepend(box);
      window.__TAURI__?.window?.getCurrentWindow?.().show?.();
    };
    if (document.body) show();
    else document.addEventListener("DOMContentLoaded", show);
  }
  window.addEventListener("error", (e) => report(e.error?.stack ?? e.message ?? String(e)));
  setTimeout(() => report(`Start-up did not finish within 10 seconds.\nWaiting for backend calls: ${[...(window.__fxfmPending?.values() ?? [])].join(", ") || "none"}`), 10000);
  window.addEventListener("unhandledrejection", (e) => report(e.reason?.stack ?? e.reason?.message ?? String(e.reason)));
})();
