/**
 * Injects into the stock HTML5 player settings (gear):
 * — Hides native Quality + Speed rows; adds our Speed (playbackRate) then Quality + Codec
 * — Default stream URL has no quality= (matches server /direct_url?video_id=…)
 *
 * Mini player button: disabled (see commented block at bottom).
 */
(function () {
  "use strict";

  /** IE8–10: no Element#closest */
  function legacyClosest(el, selector) {
    if (!el) return null;
    if (el.closest) return el.closest(selector);
    var matches =
      el.matches ||
      el.msMatchesSelector ||
      el.webkitMatchesSelector ||
      function () {
        return false;
      };
    var node = el;
    while (node && node.nodeType === 1) {
      try {
        if (matches.call(node, selector)) return node;
      } catch (e) {}
      node = node.parentElement || node.parentNode;
    }
    return null;
  }

  function legacyAddListener(el, ev, fn) {
    if (!el) return;
    if (el.addEventListener) el.addEventListener(ev, fn, false);
    else if (el.attachEvent) el.attachEvent("on" + ev, fn);
  }

  var state = {
    quality: "auto",
    codec: "",
    speed: "1",
  };

  function getPlayerRoot() {
    return document.getElementById("movie_player");
  }

  function getMoviePlayerVideo() {
    var root = getPlayerRoot();
    if (!root) return null;
    var v = root.getElementsByTagName("video");
    return v.length ? v[0] : null;
  }

  function waitForVideo(cb, triesLeft) {
    triesLeft = triesLeft === undefined ? 100 : triesLeft;
    var v = getMoviePlayerVideo();
    if (v) {
      cb(v);
      return;
    }
    if (triesLeft <= 0) return;
    setTimeout(function () {
      waitForVideo(cb, triesLeft - 1);
    }, 80);
  }

  function directUrl(base, videoId, quality, codec) {
    var q = "video_id=" + encodeURIComponent(videoId);
    if (quality && quality !== "auto") {
      q += "&quality=" + encodeURIComponent(quality);
    }
    if (codec === "mpeg4") {
      q += "&codec=mpeg4";
    }
    return base.replace(/\/$/, "") + "/direct_url?" + q;
  }

  function singleFmtMap(url) {
    return (
      "url=" +
      encodeURIComponent(url) +
      "&itag=18&type=video%2Fmp4&sig=legacy1"
    );
  }

  function applyPlaybackRate() {
    var rate = parseFloat(state.speed);
    if (isNaN(rate) || rate <= 0) rate = 1;
    waitForVideo(function (video) {
      try {
        video.playbackRate = rate;
      } catch (e) {}
    });
  }

  function rebuildPlayer() {
    var base = window.__YT_LEGACY_BASE__;
    var vid = window.__YT_LEGACY_VIDEO_ID__;
    var tpl = window.__YT_LEGACY_TEMPLATE_CONFIG__;
    if (!base || !vid || !tpl || !window.yt || !window.yt.player || !window.yt.player.Application) {
      return;
    }
    var url = directUrl(base, vid, state.quality, state.codec);
    var cfg = JSON.parse(JSON.stringify(tpl));
    cfg.args = cfg.args || {};
    cfg.args.url_encoded_fmt_stream_map = singleFmtMap(url);
    cfg.args.adaptive_fmts = "";
    cfg.args.dash = "0";
    delete cfg.args.dashmpd;
    var api = document.getElementById("player-api");
    if (!api) return;
    api.innerHTML = "";
    window.ytplayer = window.ytplayer || {};
    window.ytplayer.config = cfg;
    window.yt.player.Application.create("player-api", cfg);
    window.ytplayer.config.loaded = true;
    try {
      window.__YT_LEGACY_TEMPLATE_CONFIG__ = JSON.parse(JSON.stringify(cfg));
    } catch (e) {}
    waitForVideo(function () {
      applyPlaybackRate();
      syncInjectedSelects();
    });
  }

  function syncInjectedSelects() {
    var root = getPlayerRoot();
    if (!root) return;
    var s = root.querySelector('select[data-yt-legacy="speed"]');
    var q = root.querySelector('select[data-yt-legacy="quality"]');
    var c = root.querySelector('select[data-yt-legacy="codec"]');
    if (s) s.value = state.speed;
    if (q) q.value = state.quality;
    if (c) c.value = state.codec || "";
  }

  function rowMenuTitleText(row) {
    var title = row.querySelector(".ytp-menu-title");
    return ((title && title.textContent) || "").replace(/\s+/g, " ").trim();
  }

  /** Hide stock Quality / Speed entries in the root settings list (EN + RU). */
  function stripNativeQualityAndSpeed(menuContent) {
    var rows = menuContent.querySelectorAll(".ytp-menu-row");
    for (var i = 0; i < rows.length; i++) {
      var row = rows[i];
      if (legacyClosest(row, ".yt-legacy-stream-settings-root")) continue;
      var txt = rowMenuTitleText(row).toLowerCase();
      if (!txt) {
        txt = (row.textContent || "").replace(/\s+/g, " ").trim().toLowerCase();
      }
      if (
        txt === "quality" ||
        txt === "speed" ||
        txt.indexOf("playback speed") === 0 ||
        txt === "качество" ||
        txt === "скорость" ||
        txt.indexOf("скорость воспроизведения") === 0 ||
        txt === "this site" ||
        txt.indexOf("this site:") === 0 ||
        txt === "этот сайт" ||
        txt.indexOf("этот сайт:") === 0
      ) {
        row.style.display = "none";
        row.setAttribute("data-yt-legacy-hidden", "1");
      }
    }
  }

  function looksLikeSpeedSubmenu(menuContent) {
    var rows = menuContent.querySelectorAll(".ytp-menu-row");
    if (rows.length < 4) return false;
    var n = 0;
    for (var i = 0; i < rows.length; i++) {
      var t = (rows[i].textContent || "").replace(/\s+/g, " ").trim();
      if (/^(normal|\d+(\.\d+)?x)$/i.test(t)) n++;
    }
    return n >= rows.length * 0.65;
  }

  function looksLikeQualitySubmenu(menuContent) {
    var rows = menuContent.querySelectorAll(".ytp-menu-row");
    if (rows.length < 2) return false;
    var n = 0;
    for (var i = 0; i < rows.length; i++) {
      var t = (rows[i].textContent || "").replace(/\s+/g, " ").trim();
      if (/^\d+p$/i.test(t) || /^auto$/i.test(t) || /^авто$/i.test(t)) n++;
    }
    return n >= rows.length * 0.55;
  }

  function isElementDisplayed(el) {
    if (!el) return false;
    var st = window.getComputedStyle ? window.getComputedStyle(el) : null;
    if (st) {
      if (st.visibility === "hidden" || st.display === "none") return false;
    } else if (el.currentStyle) {
      if (el.currentStyle.visibility === "hidden" || el.currentStyle.display === "none") {
        return false;
      }
    }
    return true;
  }

  function getMainSettingsMenuContent(player) {
    var contents = player.querySelectorAll(".ytp-menu-content");
    var candidates = [];
    for (var i = 0; i < contents.length; i++) {
      var el = contents[i];
      var r = el.getBoundingClientRect();
      if (r.width < 8 || r.height < 8) continue;
      if (!isElementDisplayed(el)) continue;
      var rows = el.querySelectorAll(".ytp-menu-row").length;
      if (rows >= 2 && rows <= 8) {
        candidates.push({ el: el, rows: rows });
      }
    }
    if (!candidates.length) return null;
    candidates.sort(function (a, b) {
      return a.rows - b.rows;
    });
    for (var j = 0; j < candidates.length; j++) {
      var c = candidates[j].el;
      if (looksLikeSpeedSubmenu(c) || looksLikeQualitySubmenu(c)) continue;
      return c;
    }
    return candidates[0].el;
  }

  function buildStreamSettingsMarkup() {
    return (
      '<div class="ytp-menu-row ytp-legacy-stream-row" role="menuitem">' +
      '<div class="ytp-menu-cell ytp-menu-title">Speed</div>' +
      '<div class="ytp-menu-cell ytp-legacy-stream-cell">' +
      '<select data-yt-legacy="speed" class="yt-legacy-stream-select" title="Speed">' +
      '<option value="0.25">0.25x</option>' +
      '<option value="0.5">0.5x</option>' +
      '<option value="0.75">0.75x</option>' +
      '<option value="1" selected>Normal</option>' +
      '<option value="1.25">1.25x</option>' +
      '<option value="1.5">1.5x</option>' +
      '<option value="1.75">1.75x</option>' +
      '<option value="2">2x</option>' +
      "</select></div></div>" +
      '<div class="ytp-menu-row ytp-legacy-stream-row" role="menuitem">' +
      '<div class="ytp-menu-cell ytp-menu-title">Quality</div>' +
      '<div class="ytp-menu-cell ytp-legacy-stream-cell">' +
      '<select data-yt-legacy="quality" class="yt-legacy-stream-select" title="Quality">' +
      '<option value="auto" selected>Auto</option>' +
      '<option value="360">360p</option>' +
      '<option value="480">480p</option>' +
      '<option value="720">720p</option>' +
      '<option value="1080">1080p</option>' +
      "</select></div></div>" +
      '<div class="ytp-menu-row ytp-legacy-stream-row" role="menuitem">' +
      '<div class="ytp-menu-cell ytp-menu-title">Codec</div>' +
      '<div class="ytp-menu-cell ytp-legacy-stream-cell">' +
      '<select data-yt-legacy="codec" class="yt-legacy-stream-select" title="Codec">' +
      '<option value="">Standard</option>' +
      '<option value="mpeg4">MPEG4</option>' +
      "</select></div></div>"
    );
  }

  function bindStreamSelects(container) {
    var s = container.querySelector('select[data-yt-legacy="speed"]');
    var q = container.querySelector('select[data-yt-legacy="quality"]');
    var c = container.querySelector('select[data-yt-legacy="codec"]');
    if (s) {
      s.value = state.speed;
      legacyAddListener(s, "change", function () {
        state.speed = s.value || "1";
        applyPlaybackRate();
      });
    }
    if (q) {
      q.value = state.quality;
      legacyAddListener(q, "change", function () {
        state.quality = q.value || "auto";
        rebuildPlayer();
      });
    }
    if (c) {
      c.value = state.codec || "";
      legacyAddListener(c, "change", function () {
        state.codec = c.value || "";
        rebuildPlayer();
      });
    }
  }

  function injectStreamSettings() {
    var player = getPlayerRoot();
    if (!player) return;
    var menuContent = getMainSettingsMenuContent(player);
    if (!menuContent) return;
    stripNativeQualityAndSpeed(menuContent);
    if (menuContent.querySelector(".yt-legacy-stream-settings-root")) return;
    var wrap = document.createElement("div");
    wrap.className = "yt-legacy-stream-settings-root";
    wrap.innerHTML = buildStreamSettingsMarkup();
    menuContent.appendChild(wrap);
    bindStreamSelects(wrap);
  }

  /* ---------- Mini player (left of fullscreen) — disabled ----------
  function ensureMiniPlayerButton() {
    var player = getPlayerRoot();
    if (!player) return;
    var fs =
      player.querySelector(".ytp-button-fullscreen-enter") ||
      player.querySelector(".ytp-button-fullscreen-exit");
    if (!fs || !fs.parentNode) return;
    if (player.querySelector(".ytp-legacy-miniplayer-button")) return;
    var mini = document.createElement("div");
    mini.className = "ytp-button ytp-legacy-miniplayer-button";
    mini.setAttribute("role", "button");
    mini.setAttribute("tabindex", "6895");
    mini.setAttribute("aria-label", "Mini player");
    var img = document.createElement("img");
    img.src = "/assets/images/miniplayer.png";
    img.alt = "Mini player";
    img.width = 30;
    img.height = 30;
    mini.appendChild(img);
    mini.addEventListener("click", function (e) {
      e.preventDefault();
      e.stopPropagation();
      waitForVideo(function (video) {
        var pipOk =
          document.pictureInPictureEnabled &&
          video.requestPictureInPicture &&
          typeof document.exitPictureInPicture === "function";
        var isIE =
          navigator.userAgent.indexOf("MSIE") !== -1 ||
          navigator.userAgent.indexOf("Trident") !== -1;
        if (!pipOk || isIE) {
          window.alert("The mini-player is not supported by your browser.");
          return;
        }
        if (document.pictureInPictureElement) {
          document.exitPictureInPicture().catch(function () {});
        } else {
          video.requestPictureInPicture().catch(function () {
            window.alert("Couldn't activate the mini-player");
          });
        }
      });
    });
    fs.parentNode.insertBefore(mini, fs);
  }
  ---------- */

  function onMaybeSettingsOpened() {
    injectStreamSettings();
  }

  function startObservers() {
    var player = getPlayerRoot();
    if (!player) return;
    if (window.MutationObserver) {
      var mo = new MutationObserver(function () {
        injectStreamSettings();
      });
      mo.observe(player, {
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: ["class", "style"],
      });
    } else {
      /* IE9 and older: no MutationObserver — poll so gear menu injection still runs */
      window.setInterval(function () {
        injectStreamSettings();
      }, 900);
    }

    legacyAddListener(
      document,
      "click",
      function (e) {
        var t = e.target || e.srcElement;
        if (!t) return;
        if (!getPlayerRoot()) return;
        if (legacyClosest(t, ".ytp-settings-button")) {
          window.setTimeout(onMaybeSettingsOpened, 0);
          window.setTimeout(onMaybeSettingsOpened, 100);
          window.setTimeout(onMaybeSettingsOpened, 350);
        }
      },
      true
    );
  }

  function boot() {
    waitForVideo(function () {
      applyPlaybackRate();
      startObservers();
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
