// Copyright (c) 2026 Raimo Geisel
// SPDX-License-Identifier: GPL-3.0-only

/* ================================================================== */
/* Durthang — site.js                                                  */
/* Ember particle animation + GDPR cookie consent                      */
/* ================================================================== */

(function () {
  'use strict';

  // ---------------------------------------------------------------- //
  // GDPR consent logic                                                 //
  // ---------------------------------------------------------------- //

  var COOKIE_NAME = 'durthang_consent';
  var COOKIE_DAYS = 365;

  function getCookie(name) {
    var pairs = document.cookie.split(';');
    for (var i = 0; i < pairs.length; i++) {
      var pair = pairs[i].trimStart().split('=');
      if (pair[0] === name) {
        return pair[1] || '';
      }
    }
    return null;
  }

  function setCookie(name, value, days) {
    var expires = new Date(Date.now() + days * 864e5).toUTCString();
    document.cookie =
      name + '=' + value +
      '; expires=' + expires +
      '; path=/' +
      '; SameSite=Lax';
  }

  function hideBanner() {
    var banner = document.getElementById('gdpr-banner');
    if (banner) {
      banner.hidden = true;
    }
  }

  function initConsent() {
    var banner = document.getElementById('gdpr-banner');
    if (!banner) { return; }

    // Show banner only when no consent decision has been stored yet
    var existing = getCookie(COOKIE_NAME);
    if (existing === null) {
      banner.hidden = false;
    }

    var acceptBtn  = document.getElementById('gdpr-accept');
    var dismissBtn = document.getElementById('gdpr-dismiss');

    if (acceptBtn) {
      acceptBtn.addEventListener('click', function () {
        setCookie(COOKIE_NAME, 'accepted', COOKIE_DAYS);
        hideBanner();
      });
    }
    if (dismissBtn) {
      dismissBtn.addEventListener('click', function () {
        setCookie(COOKIE_NAME, 'dismissed', COOKIE_DAYS);
        hideBanner();
      });
    }
  }

  // ---------------------------------------------------------------- //
  // Ember particles                                                    //
  // ---------------------------------------------------------------- //

  function initEmbers() {
    var container = document.getElementById('embers');
    if (!container) { return; }

    var MAX_EMBERS   = 28;
    var active       = 0;

    function spawnEmber() {
      if (active >= MAX_EMBERS) { return; }
      active++;

      var el = document.createElement('div');
      el.className = 'ember-particle';

      var size     = (Math.random() * 3 + 1.5).toFixed(1) + 'px';
      var left     = (Math.random() * 100).toFixed(1) + '%';
      var duration = (Math.random() * 3 + 2.5).toFixed(2) + 's';
      var delay    = (Math.random() * 1.2).toFixed(2) + 's';

      el.style.width    = size;
      el.style.height   = size;
      el.style.left     = left;
      el.style.animationDuration = duration;
      el.style.animationDelay    = delay;
      el.style.opacity  = (Math.random() * 0.6 + 0.4).toFixed(2);

      // Slight colour variation between amber and ember-light
      var hue  = Math.floor(Math.random() * 20 + 15);
      el.style.background = 'hsl(' + hue + ', 90%, 58%)';

      el.addEventListener('animationend', function () {
        container.removeChild(el);
        active--;
      });

      container.appendChild(el);
    }

    // Spawn a batch to start and keep replenishing
    for (var i = 0; i < 12; i++) { spawnEmber(); }
    setInterval(spawnEmber, 350);
  }

  // ---------------------------------------------------------------- //
  // Initialise on DOM ready                                            //
  // ---------------------------------------------------------------- //

  document.addEventListener('DOMContentLoaded', function () {
    initConsent();
    initEmbers();
  });
}());
