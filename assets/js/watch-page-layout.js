/**
 * Watch page: flexwatch sidebar grid + theater / "full width" (size) button.
 * Stock html5player fires internal SIZE_CLICKED; we mirror layout by toggling
 * #page.watch-stage-mode vs #page.watch-non-stage-mode (same as archived YouTube).
 */
(function () {
  "use strict";

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

  var page = document.getElementById("page");
  var movie = document.getElementById("movie_player");
  if (!page || !movie || !document.getElementById("watch7-main-container")) {
    return;
  }

  function findSizeToggle() {
    return movie.querySelector(".ytp-size-toggle-large, .ytp-size-toggle-small");
  }

  function syncStageModeFromSizeButton() {
    var btn = findSizeToggle();
    if (!btn) {
      return;
    }
    /* Expanded / theater: control shows "collapse" (ytp-size-toggle-small). Default: ytp-size-toggle-large. */
    var stage = btn.classList.contains("ytp-size-toggle-small");
    if (stage) {
      page.classList.remove("watch-non-stage-mode");
      page.classList.add("watch-stage-mode");
    } else {
      page.classList.remove("watch-stage-mode");
      page.classList.add("watch-non-stage-mode");
    }
    try {
      window.dispatchEvent(new Event("resize"));
    } catch (e) {}
  }

  document.addEventListener(
    "click",
    function (e) {
      var t = e.target || e.srcElement;
      if (!t) {
        return;
      }
      var btn =
        legacyClosest(t, ".ytp-size-toggle-large") ||
        legacyClosest(t, ".ytp-size-toggle-small");
      if (!btn || !movie.contains(btn)) {
        return;
      }
      window.setTimeout(syncStageModeFromSizeButton, 0);
      window.setTimeout(syncStageModeFromSizeButton, 80);
    },
    true
  );

  if (window.MutationObserver) {
    var mo = new MutationObserver(function () {
      syncStageModeFromSizeButton();
    });
    mo.observe(movie, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["class"],
    });
  }

  [0, 400, 1200, 2500].forEach(function (ms) {
    window.setTimeout(syncStageModeFromSizeButton, ms);
  });
})();
