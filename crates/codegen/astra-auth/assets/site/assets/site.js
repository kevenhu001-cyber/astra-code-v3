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

  /* ── Dynamic Halftone "A" Mark (Rotation, Displacement & Idle Waves) ─── */

  function initHeroMark() {
    var markEl = document.querySelector('.hero-mark');
    if (!markEl) return;
    var svg = markEl.querySelector('.hero-mark-svg');
    if (!svg) return;

    var circleNodes = svg.querySelectorAll('circle');
    if (!circleNodes || circleNodes.length === 0) return;

    var reduceMotion = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    var isMobileQuery = window.matchMedia && window.matchMedia('(max-width: 768px)');
    var isTouchDevice = ('ontouchstart' in window) || (navigator.maxTouchPoints > 0);

    var dots = [];
    var centerCx = 7.5;
    var centerCy = 9.0;

    for (var i = 0; i < circleNodes.length; i++) {
      var c = circleNodes[i];
      var cx = parseFloat(c.getAttribute('cx')) || 0;
      var cy = parseFloat(c.getAttribute('cy')) || 0;
      var r = parseFloat(c.getAttribute('r')) || 0.2;
      var op = parseFloat(c.getAttribute('fill-opacity')) || 1.0;
      var k = parseFloat(c.getAttribute('data-k')) || 0.5;

      var nx = cx - centerCx;
      var ny = cy - centerCy;
      var dist = Math.sqrt(nx * nx + ny * ny);
      var angle = Math.atan2(ny, nx);
      // Subtle convex surface curvature for realistic 3D parallax
      var z0 = -(dist * dist) * 0.035 + (1 - k) * 0.6;

      dots.push({
        nx: nx,
        ny: ny,
        z0: z0,
        dist: dist,
        angle: angle,
        baseR: r,
        alpha: op,
        k: k,
        screenX: 0,
        screenY: 0,
        drawR: 0,
        projZ: 0,
        drawAlpha: op
      });
    }

    var canvas = document.createElement('canvas');
    canvas.className = 'hero-mark-canvas';
    canvas.setAttribute('aria-hidden', 'true');
    var ctx = canvas.getContext('2d');
    if (!ctx) return;

    markEl.appendChild(canvas);
    markEl.classList.add('is-canvas-ready');

    var viewW = 0;
    var viewH = 0;
    var scaleFactor = 1;
    var dpr = 1;

    function resize() {
      var rect = markEl.getBoundingClientRect();
      var isMobile = (isMobileQuery && isMobileQuery.matches) || (window.innerWidth <= 768);
      var maxW = isMobile ? 260 : 400;
      var w = Math.round(Math.min(rect.width || maxW, maxW));
      if (w < 180) w = 220;
      var h = Math.round(w * (18 / 15));

      dpr = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      canvas.style.width = w + 'px';
      canvas.style.height = h + 'px';

      viewW = w;
      viewH = h;
      scaleFactor = (w / 15) * 0.88;
    }

    window.addEventListener('resize', resize, { passive: true });
    resize();

    // 3D Rotation & displacement states
    var rotX = 0;
    var rotY = 0;
    var targetRotX = 0;
    var targetRotY = 0;
    var velX = 0;
    var velY = 0;

    var shiftX = 0;
    var shiftY = 0;
    var targetShiftX = 0;
    var targetShiftY = 0;

    var isDragging = false;
    var lastPointerX = 0;
    var lastPointerY = 0;
    var hasActivePointer = false;

    function handlePointerMove(clientX, clientY) {
      var rect = canvas.getBoundingClientRect();
      var centerX = rect.left + rect.width / 2;
      var centerY = rect.top + rect.height / 2;

      var normX = (clientX - centerX) / (rect.width * 0.8 || 1);
      var normY = (clientY - centerY) / (rect.height * 0.8 || 1);

      normX = Math.max(-1.5, Math.min(1.5, normX));
      normY = Math.max(-1.5, Math.min(1.5, normY));

      targetRotY = normX * 0.52;
      targetRotX = -normY * 0.42;

      targetShiftX = normX * 24;
      targetShiftY = normY * 18;
    }

    var heroSection = markEl.closest('.hero') || markEl;

    heroSection.addEventListener('pointermove', function (e) {
      if (isDragging) return;
      hasActivePointer = true;
      handlePointerMove(e.clientX, e.clientY);
    }, { passive: true });

    heroSection.addEventListener('pointerleave', function () {
      hasActivePointer = false;
      targetShiftX = 0;
      targetShiftY = 0;
    }, { passive: true });

    canvas.addEventListener('pointerdown', function (e) {
      isDragging = true;
      lastPointerX = e.clientX;
      lastPointerY = e.clientY;
      velX = 0;
      velY = 0;
      try { canvas.setPointerCapture(e.pointerId); } catch (err) {}
    });

    canvas.addEventListener('pointermove', function (e) {
      if (!isDragging) return;
      var dx = e.clientX - lastPointerX;
      var dy = e.clientY - lastPointerY;
      lastPointerX = e.clientX;
      lastPointerY = e.clientY;

      var sensitivity = 0.01;
      rotY += dx * sensitivity;
      rotX -= dy * sensitivity;
      shiftX += dx * 0.2;
      shiftY += dy * 0.2;

      velY = dx * sensitivity;
      velX = -dy * sensitivity;
    });

    function endDrag(e) {
      if (!isDragging) return;
      isDragging = false;
      try { if (e && e.pointerId) canvas.releasePointerCapture(e.pointerId); } catch (err) {}
    }

    canvas.addEventListener('pointerup', endDrag);
    canvas.addEventListener('pointercancel', endDrag);

    // Mobile device orientation integration
    if (window.DeviceOrientationEvent && isTouchDevice) {
      window.addEventListener('deviceorientation', function (e) {
        if (hasActivePointer || isDragging) return;
        if (e.gamma !== null && e.beta !== null) {
          var gamma = Math.max(-45, Math.min(45, e.gamma));
          var beta = Math.max(-45, Math.min(45, e.beta - 40));
          targetRotY = (gamma / 45) * 0.35;
          targetRotX = (beta / 45) * 0.25;
        }
      }, { passive: true });
    }

    // Visibility-aware requestAnimationFrame loop
    var isVisible = true;
    var rafId = null;
    var startTime = performance.now();

    if ('IntersectionObserver' in window) {
      var observer = new IntersectionObserver(function (entries) {
        isVisible = entries[0].isIntersecting;
        if (isVisible && !rafId) {
          startTime = performance.now();
          rafId = requestAnimationFrame(render);
        } else if (!isVisible && rafId) {
          cancelAnimationFrame(rafId);
          rafId = null;
        }
      }, { threshold: 0.05 });
      observer.observe(markEl);
    }

    document.addEventListener('visibilitychange', function () {
      if (document.hidden && rafId) {
        cancelAnimationFrame(rafId);
        rafId = null;
      } else if (!document.hidden && isVisible && !rafId) {
        startTime = performance.now();
        rafId = requestAnimationFrame(render);
      }
    });

    function render(now) {
      rafId = null;
      if (!isVisible) return;

      var elapsed = (now - startTime) / 1000;

      if (!isDragging) {
        velX *= 0.92;
        velY *= 0.92;
        rotX += velX;
        rotY += velY;

        var idleAmbientY = Math.sin(elapsed * 0.5) * 0.12;
        var idleAmbientX = Math.cos(elapsed * 0.6) * 0.08;

        if (!hasActivePointer) {
          targetRotY = idleAmbientY;
          targetRotX = idleAmbientX;
        }

        rotX += (targetRotX - rotX) * 0.07;
        rotY += (targetRotY - rotY) * 0.07;
        shiftX += (targetShiftX - shiftX) * 0.07;
        shiftY += (targetShiftY - shiftY) * 0.07;
      }

      ctx.clearRect(0, 0, canvas.width, canvas.height);

      var isDark = document.documentElement.dataset.theme === 'dark';
      var dotColor = isDark ? '#ffffff' : '#111111';

      var focal = 24;
      var centerX = (viewW / 2) + shiftX;
      var centerY = (viewH / 2) + shiftY;

      var cosY = Math.cos(rotY), sinY = Math.sin(rotY);
      var cosX = Math.cos(rotX), sinX = Math.sin(rotX);

      // Compute particle positions, wave ripples and 3D projection
      for (var j = 0; j < dots.length; j++) {
        var p = dots[j];

        // Motion 1: Idle wave ripple (fluctuating size & subtle float)
        var wavePhase = p.dist * 0.72 - p.angle * 0.28;
        var wave1 = Math.sin(elapsed * 2.3 - wavePhase);
        var wave2 = Math.cos(elapsed * 1.2 + p.ny * 0.45);
        var sizeMultiplier = reduceMotion ? 1.0 : (0.70 + 0.36 * wave1 + 0.14 * wave2);

        var floatX = reduceMotion ? 0 : Math.sin(elapsed * 1.5 + p.ny * 0.7) * 0.35;
        var floatY = reduceMotion ? 0 : Math.cos(elapsed * 1.7 + p.nx * 0.7) * 0.35;

        var x0 = p.nx + floatX;
        var y0 = p.ny + floatY;
        var z0 = p.z0;

        // Motion 2: 3D rotation & parallax displacement
        var x1 = x0 * cosY + z0 * sinY;
        var z1 = -x0 * sinY + z0 * cosY;

        var y1 = y0 * cosX - z1 * sinX;
        var z2 = y0 * sinX + z1 * cosX;

        var proj = focal / (focal - z2);

        p.screenX = centerX + x1 * scaleFactor * proj;
        p.screenY = centerY + y1 * scaleFactor * proj;
        p.drawR = Math.max(0.4, p.baseR * scaleFactor * sizeMultiplier * proj);
        p.projZ = z2;
        p.drawAlpha = Math.min(1, Math.max(0.08, p.alpha * (0.78 + 0.22 * wave1) * Math.min(1.2, proj)));
      }

      // Depth sorting
      dots.sort(function (a, b) {
        return a.projZ - b.projZ;
      });

      // Draw dots
      ctx.fillStyle = dotColor;
      for (var k = 0; k < dots.length; k++) {
        var d = dots[k];
        ctx.globalAlpha = d.drawAlpha;
        ctx.beginPath();
        ctx.arc(d.screenX * dpr, d.screenY * dpr, d.drawR * dpr, 0, Math.PI * 2);
        ctx.fill();
      }

      rafId = requestAnimationFrame(render);
    }

    rafId = requestAnimationFrame(render);
  }

  initHeroMark();
})();
