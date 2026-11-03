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

  /* ── Dynamic Dot-Matrix Morphing System: A - S - T - R - A ───────────── */

  function initHeroMark() {
    var markEl = document.querySelector('.hero-mark');
    if (!markEl) return;

    // 经典 9x11 现代科技点阵（每个字符严格 60 点，绝对工整对称）
    var GLYPH_DATA = {
      A: [[-2.0, -5.0], [-1.0, -5.0], [0.0, -5.0], [1.0, -5.0], [2.0, -5.0], [-3.0, -4.0], [-2.0, -4.0], [-1.0, -4.0], [0.0, -4.0], [1.0, -4.0], [2.0, -4.0], [3.0, -4.0], [-4.0, -3.0], [-3.0, -3.0], [3.0, -3.0], [4.0, -3.0], [-4.0, -2.0], [-3.0, -2.0], [3.0, -2.0], [4.0, -2.0], [-4.0, -1.0], [-3.0, -1.0], [3.0, -1.0], [4.0, -1.0], [-4.0, 0.0], [-3.0, 0.0], [-2.0, 0.0], [-1.0, 0.0], [0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0], [4.0, 0.0], [-4.0, 1.0], [-3.0, 1.0], [-2.0, 1.0], [-1.0, 1.0], [0.0, 1.0], [1.0, 1.0], [2.0, 1.0], [3.0, 1.0], [4.0, 1.0], [-4.0, 2.0], [-3.0, 2.0], [3.0, 2.0], [4.0, 2.0], [-4.0, 3.0], [-3.0, 3.0], [3.0, 3.0], [4.0, 3.0], [-4.0, 4.0], [-3.0, 4.0], [3.0, 4.0], [4.0, 4.0], [-4.0, 5.0], [-3.0, 5.0], [-2.0, 5.0], [2.0, 5.0], [3.0, 5.0], [4.0, 5.0]],
      S: [[-2.0, -5.0], [-1.0, -5.0], [0.0, -5.0], [1.0, -5.0], [2.0, -5.0], [-3.0, -4.0], [-2.0, -4.0], [-1.0, -4.0], [0.0, -4.0], [1.0, -4.0], [2.0, -4.0], [3.0, -4.0], [-4.0, -3.0], [-3.0, -3.0], [-2.0, -3.0], [2.0, -3.0], [3.0, -3.0], [4.0, -3.0], [-4.0, -2.0], [-3.0, -2.0], [-2.0, -2.0], [-1.0, -2.0], [-4.0, -1.0], [-3.0, -1.0], [-2.0, -1.0], [-1.0, -1.0], [0.0, -1.0], [1.0, -1.0], [2.0, -1.0], [3.0, -1.0], [-3.0, 0.0], [-2.0, 0.0], [-1.0, 0.0], [0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0], [4.0, 0.0], [1.0, 1.0], [2.0, 1.0], [3.0, 1.0], [4.0, 1.0], [-4.0, 2.0], [-3.0, 2.0], [-2.0, 2.0], [2.0, 2.0], [3.0, 2.0], [4.0, 2.0], [-3.0, 3.0], [-2.0, 3.0], [-1.0, 3.0], [0.0, 3.0], [1.0, 3.0], [2.0, 3.0], [3.0, 3.0], [-2.0, 4.0], [-1.0, 4.0], [0.0, 4.0], [1.0, 4.0], [2.0, 4.0]],
      T: [[-4.0, -5.0], [-3.0, -5.0], [-2.0, -5.0], [-1.0, -5.0], [0.0, -5.0], [1.0, -5.0], [2.0, -5.0], [3.0, -5.0], [4.0, -5.0], [-4.0, -4.0], [-3.0, -4.0], [-2.0, -4.0], [-1.0, -4.0], [0.0, -4.0], [1.0, -4.0], [2.0, -4.0], [3.0, -4.0], [4.0, -4.0], [-4.0, -3.0], [-3.0, -3.0], [-2.0, -3.0], [-1.0, -3.0], [0.0, -3.0], [1.0, -3.0], [2.0, -3.0], [3.0, -3.0], [4.0, -3.0], [-1.0, -2.0], [0.0, -2.0], [1.0, -2.0], [-1.0, -1.0], [0.0, -1.0], [1.0, -1.0], [-1.0, 0.0], [0.0, 0.0], [1.0, 0.0], [-1.0, 1.0], [0.0, 1.0], [1.0, 1.0], [-1.0, 2.0], [0.0, 2.0], [1.0, 2.0], [-2.0, 3.0], [-1.0, 3.0], [0.0, 3.0], [1.0, 3.0], [2.0, 3.0], [-2.0, 4.0], [-1.0, 4.0], [0.0, 4.0], [1.0, 4.0], [2.0, 4.0], [-3.0, 5.0], [-2.0, 5.0], [-1.0, 5.0], [0.0, 5.0], [1.0, 5.0], [2.0, 5.0], [3.0, 5.0], [0.0, 5.0]],
      R: [[-4.0, -5.0], [-3.0, -5.0], [-2.0, -5.0], [-1.0, -5.0], [0.0, -5.0], [1.0, -5.0], [2.0, -5.0], [-4.0, -4.0], [-3.0, -4.0], [2.0, -4.0], [3.0, -4.0], [4.0, -4.0], [-4.0, -3.0], [-3.0, -3.0], [3.0, -3.0], [4.0, -3.0], [-4.0, -2.0], [-3.0, -2.0], [2.0, -2.0], [3.0, -2.0], [4.0, -2.0], [-4.0, -1.0], [-3.0, -1.0], [-2.0, -1.0], [-1.0, -1.0], [0.0, -1.0], [1.0, -1.0], [2.0, -1.0], [-4.0, 0.0], [-3.0, 0.0], [-2.0, 0.0], [-1.0, 0.0], [0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0], [-4.0, 1.0], [-3.0, 1.0], [1.0, 1.0], [2.0, 1.0], [3.0, 1.0], [-4.0, 2.0], [-3.0, 2.0], [2.0, 2.0], [3.0, 2.0], [4.0, 2.0], [-4.0, 3.0], [-3.0, 3.0], [3.0, 3.0], [4.0, 3.0], [-4.0, 4.0], [-3.0, 4.0], [3.0, 4.0], [4.0, 4.0], [-4.0, 5.0], [-3.0, 5.0], [-2.0, 5.0], [2.0, 5.0], [3.0, 5.0], [4.0, 5.0]]
    };

    var WORD_KEYS = ['A', 'S', 'T', 'R', 'A'];
    var currentWordIdx = 0;
    var numParticles = 60;

    var reduceMotion = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    var isMobileQuery = window.matchMedia && window.matchMedia('(max-width: 768px)');
    var isTouchDevice = ('ontouchstart' in window) || (navigator.maxTouchPoints > 0);

    // 初始化粒子群
    var particles = [];
    var initCoords = GLYPH_DATA.A;

    for (var i = 0; i < numParticles; i++) {
      var item = initCoords[i];
      particles.push({
        fromX: item[0],
        fromY: item[1],
        toX: item[0],
        toY: item[1],
        curX: item[0],
        curY: item[1],
        z0: -(item[0] * item[0] + item[1] * item[1]) * 0.025,
        seedX: Math.sin(i * 13.7 + 1.2),
        seedY: Math.cos(i * 29.3 + 2.4),
        screenX: 0,
        screenY: 0,
        drawR: 0,
        projZ: 0,
        drawAlpha: 1
      });
    }

    var canvas = document.createElement('canvas');
    canvas.className = 'hero-mark-canvas';
    canvas.setAttribute('aria-hidden', 'true');
    var ctx = canvas.getContext('2d');
    if (!ctx) return;

    var svg = markEl.querySelector('.hero-mark-svg');
    if (svg) {
      svg.insertAdjacentElement('afterend', canvas);
    } else {
      markEl.appendChild(canvas);
    }
    markEl.classList.add('is-canvas-ready');

    var tabs = markEl.querySelectorAll('.hm-tab');
    function updateTabsUI() {
      tabs.forEach(function (tab, idx) {
        tab.classList.toggle('is-active', idx === currentWordIdx);
      });
    }

    tabs.forEach(function (tab, idx) {
      tab.addEventListener('click', function () {
        setTargetWord(idx);
      });
    });

    var viewW = 0;
    var viewH = 0;
    var scaleFactor = 1;
    var dpr = 1;

    function resize() {
      var rect = markEl.getBoundingClientRect();
      var isMobile = (isMobileQuery && isMobileQuery.matches) || (window.innerWidth <= 768);
      var maxW = isMobile ? 210 : 340;
      var w = Math.round(Math.min(rect.width || maxW, maxW));
      if (w < 160) w = 180;
      var h = w;

      dpr = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      canvas.style.width = w + 'px';
      canvas.style.height = h + 'px';

      viewW = w;
      viewH = h;
      scaleFactor = (w / 12) * 0.92;
    }

    window.addEventListener('resize', resize, { passive: true });
    resize();

    // 变形机制（Morphing Transition）
    var morphProgress = 1.0;
    var isMorphing = false;
    var morphStartTime = 0;
    var morphDuration = 1.0;
    var pauseTimer = performance.now() / 1000;
    var pauseDuration = 3.2;

    function setTargetWord(idx) {
      if (idx === currentWordIdx && !isMorphing) return;
      var key = WORD_KEYS[idx];
      var targetData = GLYPH_DATA[key];
      currentWordIdx = idx;

      for (var p = 0; p < numParticles; p++) {
        var pt = particles[p];
        pt.fromX = pt.curX;
        pt.fromY = pt.curY;
        pt.toX = targetData[p][0];
        pt.toY = targetData[p][1];
        pt.z0 = -(targetData[p][0] * targetData[p][0] + targetData[p][1] * targetData[p][1]) * 0.025;
      }

      morphProgress = 0;
      isMorphing = true;
      morphStartTime = performance.now();
      updateTabsUI();
    }

    function nextWord() {
      var nextIdx = (currentWordIdx + 1) % WORD_KEYS.length;
      setTargetWord(nextIdx);
    }

    // 交互：点击或轻触画布切换下一个字符
    canvas.addEventListener('click', function () {
      nextWord();
    });

    // 3D 旋转与位移状态
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

      targetRotY = normX * 0.48;
      targetRotX = -normY * 0.38;

      targetShiftX = normX * 18;
      targetShiftY = normY * 14;
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
      shiftX += dx * 0.15;
      shiftY += dy * 0.15;

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

    // 移动端陀螺仪支持
    if (window.DeviceOrientationEvent && isTouchDevice) {
      window.addEventListener('deviceorientation', function (e) {
        if (hasActivePointer || isDragging) return;
        if (e.gamma !== null && e.beta !== null) {
          var gamma = Math.max(-45, Math.min(45, e.gamma));
          var beta = Math.max(-45, Math.min(45, e.beta - 40));
          targetRotY = (gamma / 45) * 0.3;
          targetRotX = (beta / 45) * 0.2;
        }
      }, { passive: true });
    }

    // 可视性与帧循环
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

      // 1. 变形插值更新
      if (isMorphing) {
        var elapsedMorph = (now - morphStartTime) / 1000;
        morphProgress = Math.min(1.0, elapsedMorph / morphDuration);

        for (var j = 0; j < numParticles; j++) {
          var pt = particles[j];
          var delay = (j / numParticles) * 0.2;
          var localT = Math.max(0, Math.min(1, (morphProgress - delay) / 0.8));
          var easeT = localT < 0.5 ? 4 * localT * localT * localT : 1 - Math.pow(-2 * localT + 2, 3) / 2;

          var burst = Math.sin(localT * Math.PI);
          var swirlX = pt.seedX * burst * 1.2;
          var swirlY = pt.seedY * burst * 1.2;

          pt.curX = pt.fromX + (pt.toX - pt.fromX) * easeT + swirlX;
          pt.curY = pt.fromY + (pt.toY - pt.fromY) * easeT + swirlY;
        }

        if (morphProgress >= 1.0) {
          isMorphing = false;
          pauseTimer = elapsed;
        }
      } else {
        if (!reduceMotion && elapsed - pauseTimer > pauseDuration) {
          nextWord();
        }
      }

      // 2. 物理旋转与阻尼
      if (!isDragging) {
        velX *= 0.92;
        velY *= 0.92;
        rotX += velX;
        rotY += velY;

        var idleAmbientY = Math.sin(elapsed * 0.5) * 0.1;
        var idleAmbientX = Math.cos(elapsed * 0.6) * 0.07;

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

      // 3. 计算粒子波动与 3D 透视投影
      for (var k = 0; k < numParticles; k++) {
        var p = particles[k];

        // 静止波动态：大大小小的波纹起伏
        var dist = Math.sqrt(p.curX * p.curX + p.curY * p.curY);
        var wavePhase = dist * 0.65;
        var wave1 = Math.sin(elapsed * 2.4 - wavePhase);
        var wave2 = Math.cos(elapsed * 1.2 + p.curY * 0.5);
        var sizeMultiplier = reduceMotion ? 1.0 : (0.74 + 0.32 * wave1 + 0.12 * wave2);

        var floatX = reduceMotion ? 0 : Math.sin(elapsed * 1.5 + p.curY * 0.7) * 0.22;
        var floatY = reduceMotion ? 0 : Math.cos(elapsed * 1.7 + p.curX * 0.7) * 0.22;

        var x0 = p.curX + floatX;
        var y0 = p.curY + floatY;
        var z0 = p.z0;

        // 3D 旋转变换
        var x1 = x0 * cosY + z0 * sinY;
        var z1 = -x0 * sinY + z0 * cosY;

        var y1 = y0 * cosX - z1 * sinX;
        var z2 = y0 * sinX + z1 * cosX;

        var proj = focal / (focal - z2);

        p.screenX = centerX + x1 * scaleFactor * proj;
        p.screenY = centerY + y1 * scaleFactor * proj;
        p.drawR = Math.max(1.8, 3.4 * sizeMultiplier * proj * (scaleFactor / 18));
        p.projZ = z2;
        p.drawAlpha = Math.min(1, Math.max(0.18, (0.82 + 0.18 * wave1) * Math.min(1.2, proj)));
      }

      // 深度排序
      particles.sort(function (a, b) {
        return a.projZ - b.projZ;
      });

      // 4. 批量绘制圆点
      ctx.fillStyle = dotColor;
      for (var m = 0; m < numParticles; m++) {
        var d = particles[m];
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
