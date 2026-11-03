/* Astra site — shared front-end behaviour for the marketing page.

   Three small jobs, none of which the page can do without scripts:
     1. the composer turns a sentence into the command you run in your shell,
     2. the install cards copy their command to the clipboard,
     3. the header grows a hairline once the page moves under it.

   Everything degrades: without scripts the composer is still a text field,
   the copy buttons are still buttons that select their command, and the
   header simply stays borderless. */
(function () {
  'use strict';

  /* ── Composer ────────────────────────────────────────────────────────── */

  var form = document.querySelector('[data-composer]');
  var hint = document.querySelector('[data-composer-hint]');

  function fallbackCopy(text) {
    var ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.top = '-9999px';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    try { document.execCommand('copy'); } catch (e) {}
    document.body.removeChild(ta);
  }

  function copy(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      return navigator.clipboard.writeText(text);
    }
    fallbackCopy(text);
    return Promise.resolve();
  }

  function say(message, ok) {
    if (!hint) return;
    hint.textContent = message;
    hint.classList.toggle('is-ok', !!ok);
  }

  if (form) {
    var input = form.querySelector('.composer-input');

    // Starter chips fill the field instead of navigating away, so the reader
    // can edit the sentence before copying the command.
    document.querySelectorAll('[data-starter]').forEach(function (chip) {
      chip.addEventListener('click', function () {
        if (!input) return;
        input.value = chip.getAttribute('data-starter') || '';
        input.focus();
        say('Command ready — copy it to run in your terminal.', true);
      });
    });

    form.addEventListener('submit', function (event) {
      event.preventDefault();
      var task = (input && input.value || '').trim();
      if (!task) {
        if (input) input.focus();
        say('Describe the task first.', false);
        return;
      }
      var command = 'astra "' + task.replace(/"/g, '\\"') + '"';
      copy(command).then(function () {
        say('Copied — paste it into your terminal.', true);
      }, function () {
        say(command, true);
      });
    });
  }

  /* ── Install copy buttons ────────────────────────────────────────────── */

  document.querySelectorAll('.copy-btn').forEach(function (btn) {
    btn.addEventListener('click', function () {
      var line = btn.closest('.cmd-line');
      var cmd = line ? line.querySelector('.cmd').textContent.trim() : '';
      var label = btn.querySelector('.copy-label');
      var prev = label ? label.textContent : 'Copy';
      function flash() {
        btn.classList.add('copied');
        if (label) label.textContent = 'Copied';
        setTimeout(function () {
          btn.classList.remove('copied');
          if (label) label.textContent = prev;
        }, 1600);
      }
      copy(cmd).then(flash, function () {
        fallbackCopy(cmd);
        flash();
      });
    });
  });

  /* ── Scroll-aware header ─────────────────────────────────────────────── */

  // The bar's background layer is a pseudo-element on the header, so only its
  // opacity animates: nothing on the page shifts when the class flips.
  var header = document.querySelector('.site-header[data-scroll-aware]');
  if (header) {
    var ticking = false;
    function update() {
      header.classList.toggle('is-scrolled', window.scrollY > 8);
      ticking = false;
    }
    window.addEventListener('scroll', function () {
      if (ticking) return;
      ticking = true;
      window.requestAnimationFrame(update);
    }, { passive: true });
    update();
  }
})();
