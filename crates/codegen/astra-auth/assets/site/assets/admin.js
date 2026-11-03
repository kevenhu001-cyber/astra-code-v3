(function () {
  'use strict';

  var A = window.Astra;
  var routes = [];
  var users = [];
  var currentSection = 'routes';
  var initialized = false;
  var routePanel = document.getElementById('routePanel');
  var creditPanel = document.getElementById('creditPanel');
  var workspace = document.getElementById('adminWorkspace');
  var message = document.getElementById('adminMessage');

  function showMessage(text, kind) {
    message.textContent = text || '';
    message.className = 'admin-message' + (kind ? ' is-' + kind : '');
    message.hidden = !text;
  }

  function formatUsd(microusd) {
    var amount = Number(microusd || 0) / 1000000;
    return new Intl.NumberFormat('en-US', {
      style: 'currency', currency: 'USD', minimumFractionDigits: 2, maximumFractionDigits: 2
    }).format(amount);
  }

  function shortEndpoint(value) {
    try {
      var url = new URL(value);
      return url.host + (url.pathname === '/' ? '' : url.pathname);
    } catch (e) {
      return value || '';
    }
  }

  function makeCell(text, className) {
    var cell = document.createElement('td');
    if (className) cell.className = className;
    cell.textContent = text || '';
    return cell;
  }

  function button(text, className, onClick) {
    var el = document.createElement('button');
    el.type = 'button';
    el.className = className;
    el.textContent = text;
    el.addEventListener('click', onClick);
    return el;
  }

  function renderRoutes() {
    var list = document.getElementById('routeRows');
    var empty = document.getElementById('routesEmpty');
    list.innerHTML = '';
    empty.hidden = routes.length > 0;
    routes.forEach(function (route) {
      var row = document.createElement('tr');
      var name = document.createElement('td');
      name.className = 'route-name-cell';
      var label = document.createElement('strong');
      label.textContent = route.display_name;
      name.appendChild(label);
      if (route.is_default) {
        var defaultTag = document.createElement('span');
        defaultTag.className = 'default-tag';
        defaultTag.textContent = 'Default';
        name.appendChild(defaultTag);
      }
      row.appendChild(name);
      row.appendChild(makeCell(route.protocol === 'anthropic-messages' ? 'Anthropic Messages' : 'OpenAI-compatible', 'protocol-cell'));
      row.appendChild(makeCell(route.upstream_model, 'mono-cell'));
      row.appendChild(makeCell(shortEndpoint(route.base_url), 'endpoint-cell mono-cell'));
      var status = document.createElement('td');
      var statusBadge = document.createElement('span');
      statusBadge.className = 'route-status' + (route.enabled ? ' is-active' : ' is-disabled');
      statusBadge.textContent = route.enabled ? 'Active' : 'Disabled';
      status.appendChild(statusBadge);
      row.appendChild(status);
      var action = document.createElement('td');
      action.className = 'table-action-cell';
      action.appendChild(button('Edit', 'table-action', function () { editRoute(route); }));
      action.appendChild(button('Delete', 'table-action is-danger', function () { deleteRoute(route); }));
      row.appendChild(action);
      list.appendChild(row);
    });
  }

  function renderUsers() {
    var list = document.getElementById('userRows');
    var empty = document.getElementById('usersEmpty');
    list.innerHTML = '';
    empty.hidden = users.length > 0;
    users.forEach(function (user) {
      var row = document.createElement('tr');
      var identity = document.createElement('td');
      identity.className = 'user-identity-cell';
      var email = document.createElement('strong');
      email.textContent = user.email;
      identity.appendChild(email);
      if (user.display_name) {
        var displayName = document.createElement('span');
        displayName.textContent = user.display_name;
        identity.appendChild(displayName);
      }
      row.appendChild(identity);
      row.appendChild(makeCell(user.is_admin ? 'Administrator' : 'User', user.is_admin ? 'role-admin' : ''));
      row.appendChild(makeCell(formatUsd(user.balance_microusd), 'balance-cell'));
      row.appendChild(makeCell(new Date(user.created_at).toLocaleDateString(), 'date-cell'));
      var action = document.createElement('td');
      action.className = 'table-action-cell';
      action.appendChild(button('Add credit', 'table-action is-accent', function () { openCredit(user); }));
      row.appendChild(action);
      list.appendChild(row);
    });
  }

  async function loadRoutes() {
    var data = await A.api('/admin/models', { method: 'GET' });
    routes = data.routes || [];
    renderRoutes();
  }

  async function loadUsers() {
    var data = await A.api('/admin/users', { method: 'GET' });
    users = data.users || [];
    renderUsers();
  }

  function closePanels() {
    routePanel.hidden = true;
    creditPanel.hidden = true;
    workspace.classList.remove('with-panel');
  }

  function openRoute() {
    document.getElementById('routeForm').reset();
    document.getElementById('routeId').value = '';
    document.getElementById('routePanelTitle').textContent = 'New route';
    document.getElementById('saveRouteBtn').textContent = 'Save route';
    document.getElementById('routeEnabled').checked = true;
    document.getElementById('routeDefault').checked = routes.length === 0;
    document.getElementById('clearApiKey').checked = false;
    document.getElementById('clearKeyRow').hidden = true;
    document.getElementById('apiKeyHint').textContent = 'Stored by Astra and never returned to the browser.';
    routePanel.hidden = false;
    creditPanel.hidden = true;
    workspace.classList.add('with-panel');
    document.getElementById('routeName').focus();
  }

  function editRoute(route) {
    var form = document.getElementById('routeForm');
    form.reset();
    document.getElementById('routeId').value = route.id;
    document.getElementById('routeName').value = route.display_name;
    document.getElementById('routeProtocol').value = route.protocol;
    document.getElementById('routeBaseUrl').value = route.base_url;
    document.getElementById('routeUpstreamModel').value = route.upstream_model;
    document.getElementById('routeContextWindow').value = route.context_window || 128000;
    document.getElementById('inputPrice').value = route.input_price_usd_per_million;
    document.getElementById('outputPrice').value = route.output_price_usd_per_million;
    document.getElementById('routeEnabled').checked = !!route.enabled;
    document.getElementById('routeDefault').checked = !!route.is_default;
    document.getElementById('routeApiKey').value = '';
    document.getElementById('clearApiKey').checked = false;
    document.getElementById('clearKeyRow').hidden = !route.api_key_set;
    document.getElementById('apiKeyHint').textContent = route.api_key_set
      ? 'A key is saved. Leave this blank to keep it, or enter a new key.'
      : 'Stored by Astra and never returned to the browser.';
    document.getElementById('routePanelTitle').textContent = 'Edit route';
    document.getElementById('saveRouteBtn').textContent = 'Save changes';
    routePanel.hidden = false;
    creditPanel.hidden = true;
    workspace.classList.add('with-panel');
    document.getElementById('routeName').focus();
  }

  async function deleteRoute(route) {
    if (!window.confirm('Delete the route “' + route.display_name + '”?')) return;
    try {
      await A.api('/admin/models', { method: 'DELETE', body: { id: route.id } });
      await loadRoutes();
      showMessage('Route deleted.', 'success');
    } catch (err) {
      showMessage(err.message, 'error');
    }
  }

  function openCredit(user) {
    document.getElementById('creditUserId').value = user.id;
    document.getElementById('creditUserEmail').textContent = user.email;
    document.getElementById('creditAmount').value = '';
    document.getElementById('creditNote').value = '';
    creditPanel.hidden = false;
    routePanel.hidden = true;
    workspace.classList.add('with-panel');
    document.getElementById('creditAmount').focus();
  }

  function selectSection(section) {
    currentSection = section === 'users' ? 'users' : 'routes';
    closePanels();
    document.getElementById('routesView').hidden = currentSection !== 'routes';
    document.getElementById('usersView').hidden = currentSection !== 'users';
    document.getElementById('adminTitle').textContent = currentSection === 'routes' ? 'Model routes' : 'Users & balance';
    document.getElementById('adminDescription').textContent = currentSection === 'routes'
      ? 'Choose how Astra sends CLI requests to providers.'
      : 'Manage account credit for Astra CLI users.';
    document.getElementById('addRouteBtn').hidden = currentSection !== 'routes';
    document.querySelectorAll('[data-admin-section]').forEach(function (item) {
      var active = item.dataset.adminSection === currentSection;
      item.classList.toggle('is-active', active);
      if (active) item.setAttribute('aria-current', 'page');
      else item.removeAttribute('aria-current');
    });
    if (currentSection === 'users') loadUsers().catch(function (err) { showMessage(err.message, 'error'); });
  }

  document.getElementById('addRouteBtn').addEventListener('click', openRoute);
  document.querySelectorAll('[data-open-route]').forEach(function (item) { item.addEventListener('click', openRoute); });
  document.querySelectorAll('[data-close-panel]').forEach(function (item) { item.addEventListener('click', closePanels); });
  document.querySelectorAll('[data-admin-section]').forEach(function (item) {
    item.addEventListener('click', function () { selectSection(item.dataset.adminSection); });
  });

  document.getElementById('routeForm').addEventListener('submit', async function (event) {
    event.preventDefault();
    showMessage('', '');
    var payload = {
      id: document.getElementById('routeId').value || undefined,
      display_name: document.getElementById('routeName').value.trim(),
      protocol: document.getElementById('routeProtocol').value,
      base_url: document.getElementById('routeBaseUrl').value.trim(),
      upstream_model: document.getElementById('routeUpstreamModel').value.trim(),
      context_window: Number(document.getElementById('routeContextWindow').value),
      input_price_usd_per_million: Number(document.getElementById('inputPrice').value || 0),
      output_price_usd_per_million: Number(document.getElementById('outputPrice').value || 0),
      enabled: document.getElementById('routeEnabled').checked,
      is_default: document.getElementById('routeDefault').checked,
      clear_api_key: document.getElementById('clearApiKey').checked,
    };
    var key = document.getElementById('routeApiKey').value.trim();
    if (key) payload.api_key = key;
    try {
      var data = await A.api('/admin/models', { body: payload });
      routes = data.routes || [];
      renderRoutes();
      closePanels();
      showMessage('Route saved. CLI requests will use this route immediately.', 'success');
    } catch (err) {
      showMessage(err.message, 'error');
    }
  });

  document.getElementById('creditForm').addEventListener('submit', async function (event) {
    event.preventDefault();
    showMessage('', '');
    try {
      await A.api('/admin/users/balance', { body: {
        user_id: document.getElementById('creditUserId').value,
        amount_usd: Number(document.getElementById('creditAmount').value),
        note: document.getElementById('creditNote').value.trim() || 'Administrator credit',
      } });
      await loadUsers();
      closePanels();
      showMessage('Account credit added.', 'success');
    } catch (err) {
      showMessage(err.message, 'error');
    }
  });

  window.AstraAdmin = {
    init: function (user) {
      if (initialized || !user || !user.is_admin) return Promise.resolve();
      initialized = true;
      document.getElementById('adminEmail').textContent = user.email;
      return Promise.all([loadRoutes(), loadUsers()]).catch(function (err) {
        showMessage(err.message || 'Could not load administrator data.', 'error');
      });
    }
  };
})();
