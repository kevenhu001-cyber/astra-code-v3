/* Astra site auth helpers.
   Thin wrapper around the JSON API plus a few DOM helpers. No emoji in
   user-facing strings; the layout leaves the iconography to inline SVG.
   Resilient to cookie blockers: when the HttpOnly `sid` cookie is dropped
   (Brave, Safari ITP, uBlock, "Block all cookies"), the session token
   returned by /login is persisted in localStorage and sent as
   Authorization: Bearer <sid> on every request. */
(function () {
  'use strict';

  var API = '/api/auth';
  var SID_KEY = 'astra_sid';

  function getSid() {
    try { return localStorage.getItem(SID_KEY); } catch (e) { return null; }
  }
  function setSid(tok) {
    try {
      if (tok) localStorage.setItem(SID_KEY, tok);
      else localStorage.removeItem(SID_KEY);
    } catch (e) {}
  }
  function clearSid() { setSid(null); }

  (function bootstrapFromUrl() {
    try {
      var m = window.location.hash.match(/sid=([a-f0-9]{48})/);
      if (m) {
        setSid(m[1]);
        history.replaceState(null, '', window.location.pathname + window.location.search);
      }
      var q = new URLSearchParams(window.location.search).get('sid');
      if (q && /^[a-f0-9]{48}$/.test(q)) {
        setSid(q);
        var url = new URL(window.location.href);
        url.searchParams.delete('sid');
        history.replaceState(null, '', url.pathname + url.search + url.hash);
      }
      var stored = getSid();
      if (stored) {
        var h = document.documentElement;
        if (h) h.setAttribute('data-astra-auth', 'token');
      }
    } catch (e) {}
  })();

  async function api(path, opts) {
    opts = opts || {};
    var headers = { 'Content-Type': 'application/json' };
    var sid = getSid();
    if (sid) {
      headers['Authorization'] = 'Bearer ' + sid;
      headers['X-Session-Token'] = sid;
    }
    // Account endpoints live under /api/auth, while administrator endpoints
    // have their own /api/admin namespace.
    var endpoint = path.indexOf('/admin/') === 0 ? '/api' + path : API + path;
    var res = await fetch(endpoint, {
      method: opts.method || 'POST',
      headers: headers,
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
    if (path === '/logout') {
      clearSid();
    }
    if (data && data.token) {
      setSid(data.token);
    }
    var sidHeader = res.headers.get('x-session-token');
    if (sidHeader && /^[a-f0-9]{48}$/.test(sidHeader)) {
      setSid(sidHeader);
    }
    return data;
  }

  async function me() {
    try {
      var d = await api('/me', { method: 'GET' });
      if (d.user) return d.user;
      var sid = getSid();
      if (sid) {
        try {
          var check = await fetch(API + '/me', {
            method: 'GET',
            headers: { 'Authorization': 'Bearer ' + sid, 'X-Session-Token': sid },
            credentials: 'omit',
          });
          var jd = await check.json();
          if (jd.user) return jd.user;
        } catch (e) {}
      }
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

  function loginDestination() {
    var next = qs('next');
    if (next === '/admin' || next === '/account#admin' || next === '/authorize') return next;
    return '/account';
  }

  function setTheme(theme) {
    var next = theme === 'dark' ? 'dark' : 'light';
    document.documentElement.dataset.theme = next;
    try { localStorage.setItem('astra_theme', next); } catch (e) {}
    document.querySelectorAll('[data-theme-toggle]').forEach(function (button) {
      var target = next === 'light' ? 'dark' : 'light';
      var label = target === 'dark' ? 'Dark' : 'Light';
      button.setAttribute('aria-label', 'Switch to ' + target + ' mode');
      button.setAttribute('aria-pressed', String(next === 'dark'));
      var text = button.querySelector('[data-theme-label]');
      if (text) text.textContent = label;
    });
  }

  setTheme(document.documentElement.dataset.theme || 'light');
  document.querySelectorAll('[data-theme-toggle]').forEach(function (button) {
    button.addEventListener('click', function () {
      setTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark');
    });
  });

  function redirectLogin(next) {
    if (window.location.pathname === '/login') return;
    var destination = next || (window.location.pathname === '/authorize' ? '/authorize' : '/account');
    if (destination !== '/admin' && destination !== '/account#admin' && destination !== '/authorize') destination = '/account';
    window.location.replace('/login?next=' + encodeURIComponent(destination));
  }

  function redirectAccount() {
    window.location.replace('/account');
  }

  async function logout() {
    try { await api('/logout'); } catch (e) {}
    clearSid();
  }

  window.Astra = {
    api: api,
    me: me,
    qs: qs,
    setErr: setErr,
    setOk: setOk,
    redirectLogin: redirectLogin,
    redirectAccount: redirectAccount,
    loginDestination: loginDestination,
    getSid: getSid,
    setSid: setSid,
    clearSid: clearSid,
    logout: logout,
    setTheme: setTheme,
  };
})();
