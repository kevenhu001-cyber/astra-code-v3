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
    var res = await fetch(API + path, {
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

  // Sentinel used to break the login↔account bounce loop on browsers
  // (Edge in particular) that replay bfcached pages and re-run their
  // /me poll even when the user just landed here. Without this guard,
  // a session whose cookie was dropped mid-flight can ping-pong between
  // the two pages forever because each one immediately redirects to the
  // other. Tracking the last page we redirected *to* in sessionStorage
  // lets us short-circuit a redirect whose destination matches where we
  // already are.
  var BOUNCE_KEY = 'astra_bounce';
  function markBounce(dest) {
    try { sessionStorage.setItem(BOUNCE_KEY, dest); } catch (e) {}
  }
  function lastBounce() {
    try { return sessionStorage.getItem(BOUNCE_KEY) || ''; } catch (e) { return ''; }
  }
  function clearBounce() {
    try { sessionStorage.removeItem(BOUNCE_KEY); } catch (e) {}
  }

  function redirectLogin() {
    var here = window.location.pathname.replace(/\/+$/, '') || '/';
    var dest = here === '/login' ? '' : 'login';
    if (dest && lastBounce() === dest) {
      // We just bounced here — don't redirect again, the cookie is
      // genuinely missing. Leave the user on this page so they can sign
      // in manually instead of being trapped in a reload loop.
      clearBounce();
      return;
    }
    markBounce('login');
    window.location.replace(dest);
  }

  function redirectAccount() {
    var here = window.location.pathname.replace(/\/+$/, '') || '/';
    var dest = here === '/account' ? '' : 'account';
    if (dest && lastBounce() === dest) {
      clearBounce();
      return;
    }
    markBounce('account');
    window.location.replace(dest);
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
    getSid: getSid,
    setSid: setSid,
    clearSid: clearSid,
    logout: logout,
  };
})();
