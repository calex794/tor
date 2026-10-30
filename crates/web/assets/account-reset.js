// Reset proof stays in the fragment until this same-origin script consumes it.
// Never use storage, analytics, fetch, a query string or an external redirect.
(function () {
  'use strict';
  const code = window.location.hash.slice(1);
  window.history.replaceState(null, '', window.location.pathname);
  if (/^[A-Za-z0-9_-]{43}$/.test(code)) {
    document.getElementById('reset-token').value = code;
  }
})();
