/* Astra site auth helpers.
   Thin wrapper around the JSON API plus a few DOM helpers. No emoji in
   user-facing strings; the layout leaves the iconography to inline SVG. */
(function () {
  'use strict';

  var API = '/api/auth';

  async function api(path, opts) {
    opts = opts || {};
    var res = await fetch(API + path, {
      method: opts.method || 'POST',
      headers: { 'Content-Type': 'application/json' },
      credentials: 'same-origin',
      body: opts.body ? JSON.stringify(opts.body) : undefined,
    });
    var data;
    try { data = await res.json(); } catch (e) { data = {}; }
    if (!res.ok) {
      var err = new Error((data && data.error) || ('HTTP ' + res.status));
      err.status = res.status;
      throw err;
    }
    return data;
  }

  async function me() {
    try {
      var d = await api('/me', { method: 'GET' });
      return d.user;
    } catch (e) {
      return null;
    }
  }

  function qs(name) {
    return new URLSearchParams(window.location.search).get(name);
  }

  function setErr(msg) {
    var el = document.getElementById('formErr');
    if (!el) return;
    el.textContent = msg || '';
    el.classList.toggle('show', !!msg);
  }

  function setOk(msg) {
    var el = document.getElementById('formOk');
    if (!el) return;
    el.textContent = msg || '';
    el.classList.toggle('show', !!msg);
  }

  function redirectLogin() {
    window.location.href = 'login';
  }

  function redirectAccount() {
    window.location.href = 'account';
  }

  window.Astra = {
    api: api,
    me: me,
    qs: qs,
    setErr: setErr,
    setOk: setOk,
    redirectLogin: redirectLogin,
    redirectAccount: redirectAccount,
  };
})();
