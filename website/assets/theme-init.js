/* Applied before the stylesheet so the first paint uses the saved theme. */
(function () {
  var theme = 'dark';
  try {
    var forced = new URLSearchParams(window.location.search).get('theme');
    var stored = localStorage.getItem('muxterm-theme');
    if (forced === 'light' || forced === 'dark') {
      theme = forced;
    } else if (stored === 'light' || stored === 'dark') {
      theme = stored;
    }
  } catch (_) {
    // Storage and URLSearchParams can be unavailable in restricted contexts.
  }
  document.documentElement.dataset.theme = theme;
})();
