/**
 * Load subscriptions into both:
 * - #subscriptions-sidebar-content (right sidebar, original format)
 * - #guide-subscriptions-list (left sidebar guide, new format under "Best of YouTube")
 * via /api/subscriptions_session.
 * ES5 / IE7-compatible: no arrow functions, no const/let, no fetch, no template literals.
 */
(function () {
  function escapeHtml(s) {
    if (s == null) return '';
    var str = String(s);
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function log(msg) {
    if (window.console && window.console.log) {
      try {
        window.console.log('[Subscriptions] ' + msg);
      } catch (e) {}
    }
  }

  function getCookie(name) {
    var value = '; ' + document.cookie;
    var parts = value.split('; ' + name + '=');
    if (parts.length === 2) {
      return parts.pop().split(';').shift();
    }
    return null;
  }

  function loadSubscriptions() {
    var container = document.getElementById('subscriptions-sidebar-content');
    var guideContainer = document.getElementById('guide-subscriptions-list');
    
    log('Starting...');
    log('Right container: ' + (container ? 'found' : 'not found'));
    log('Left container: ' + (guideContainer ? 'found' : 'not found'));
    
    // If neither container exists, exit
    if (!container && !guideContainer) {
      log('No containers, exiting');
      return;
    }

    var xhr;
    try {
      xhr = new XMLHttpRequest();
      log('XHR created');
    } catch (e) {
      log('XHR failed, trying ActiveX');
      try {
        xhr = new ActiveXObject('Msxml2.XMLHTTP');
      } catch (e2) {
        try {
          xhr = new ActiveXObject('Microsoft.XMLHTTP');
        } catch (e3) {
          log('ERROR: Cannot create XHR');
          return;
        }
      }
    }
    
    log('Opening request...');
    
    // Get session token from cookie
    var sessionId = getCookie('session_id');
    log('Session ID: ' + (sessionId ? 'found (' + sessionId.length + ' chars)' : 'not found'));
    
    // Build URL with token parameter for old IE compatibility
    var url = '/api/subscriptions_session';
    if (sessionId) {
      url += '?token=' + encodeURIComponent(sessionId);
      log('Using token parameter');
    } else {
      log('No session ID, using cookies');
    }
    
    xhr.open('GET', url, true);
    // Don't use withCredentials in old IE as it may cause issues
    try {
      xhr.withCredentials = true;
    } catch (e) {
      log('withCredentials not supported');
    }
    
    log('Cookies: ' + (document.cookie || 'none'));

    xhr.onreadystatechange = function () {
      if (xhr.readyState !== 4) return;
      
      log('Status: ' + xhr.status);
      log('Response length: ' + (xhr.responseText ? xhr.responseText.length : 0));
      
      // Log response content
      if (xhr.responseText) {
        log('Response: ' + xhr.responseText.substring(0, 200));
      }
      
      var html = '';
      var guideHtml = '';
      try {
        if (xhr.status === 200 && xhr.responseText) {
          log('Parsing response...');
          var data;
          try {
            data = JSON.parse(xhr.responseText);
            log('JSON parsed OK');
          } catch (parseErr) {
            log('JSON parse failed, trying eval');
            try {
              data = eval('(' + xhr.responseText + ')');
              log('Eval OK');
            } catch (e2) {
              log('ERROR: Parse failed');
              data = { subscriptions: [], main_url: '' };
            }
          }
          
          var mainUrl = '';
          if (data.main_url != null) {
            mainUrl = String(data.main_url);
            while (mainUrl.length > 0 && mainUrl.charAt(mainUrl.length - 1) === '/') {
              mainUrl = mainUrl.substring(0, mainUrl.length - 1);
            }
          }
          log('Main URL: ' + mainUrl);
          
          var subs = data.subscriptions;
          log('Subscriptions count: ' + (subs ? subs.length : 0));
          
          if (subs && subs.length > 0) {
            log('Processing ' + subs.length + ' subscriptions');
            var i, sub, channelUrl, iconSrc, titleEsc, channelIdEsc, handleEnc;
            
            // HTML for right sidebar (original format)
            if (container) {
              html = '<ul class="branded-page-related-channels-list">';
            }
            
            // HTML for left sidebar guide (new format)
            if (guideContainer) {
              guideHtml = '';
            }
            
            for (i = 0; i < subs.length; i++) {
              sub = subs[i];
              handleEnc = encodeURIComponent(sub.title || '');
              channelUrl = mainUrl + '/channel?handle=' + handleEnc;
              
              // Safe thumbnail selection for old IE
              iconSrc = '/assets/images/pixel-vfl3z5WfW.gif';
              if (sub.local_thumbnail && sub.local_thumbnail.length > 0) {
                iconSrc = sub.local_thumbnail;
              } else if (sub.thumbnail && sub.thumbnail.length > 0) {
                iconSrc = sub.thumbnail;
              }
              
              titleEsc = escapeHtml(sub.title || '');
              channelIdEsc = escapeHtml(sub.channel_id || '');
              
              // Right sidebar format
              if (container) {
                html += '<li class="branded-page-related-channels-item spf-link clearfix" data-external-id="' + channelIdEsc + '">';
                html += '<span class="yt-lockup clearfix yt-lockup-channel yt-lockup-mini">';
                html += '<div class="yt-lockup-thumbnail" style="width: 34px;">';
                html += '<a href="' + escapeHtml(channelUrl) + '" class="ux-thumb-wrap yt-uix-sessionlink spf-link">';
                html += '<span class="video-thumb yt-thumb yt-thumb-34 g-hovercard">';
                html += '<span class="yt-thumb-square"><span class="yt-thumb-clip">';
                html += '<img src="' + escapeHtml(iconSrc) + '" alt="Thumbnail" width="34" height="34">';
                html += '<span class="vertical-align"></span></span></span></span></a></div>';
                html += '<div class="yt-lockup-content">';
                html += '<span class="qualified-channel-title ellipsized"><span class="qualified-channel-title-wrapper">';
                html += '<span dir="ltr" class="qualified-channel-title-text g-hovercard">';
                html += '<h3 class="yt-lockup-title"><a class="yt-uix-sessionlink yt-uix-tile-link spf-link" dir="ltr" title="' + titleEsc + '" href="' + escapeHtml(channelUrl) + '">' + titleEsc + '</a></h3>';
                html += '</span></span></span></div></span></li>';
              }
              
              // Left sidebar guide format
              if (guideContainer) {
                guideHtml += '<li class="vve-check guide-channel overflowable-list-item">';
                guideHtml += '<a class="guide-item yt-uix-sessionlink yt-valign spf-link" href="' + escapeHtml(channelUrl) + '" title="' + titleEsc + '">';
                guideHtml += '<span class="yt-valign-container">';
                guideHtml += '<span class="thumb"><span class="video-thumb yt-thumb yt-thumb-20"><span class="yt-thumb-square"><span class="yt-thumb-clip">';
                guideHtml += '<img src="' + escapeHtml(iconSrc) + '" width="20" height="20" alt="">';
                guideHtml += '<span class="vertical-align"></span></span></span></span></span>';
                guideHtml += '<span class="display-name no-count"><span>' + titleEsc + '</span></span>';
                guideHtml += '</span></a></li>';
              }
            }
            
            if (container) {
              html += '</ul>';
            }
            log('HTML generated for ' + subs.length + ' items');
          } else {
            log('No subscriptions in response');
            if (container) {
              html = '<p class="subscriptions-loading">No subscriptions</p>';
            }
            if (guideContainer) {
              guideHtml = '<li class="vve-check guide-channel overflowable-list-item">';
              guideHtml += '<a class="guide-item yt-uix-sessionlink yt-valign spf-link" href="#">';
              guideHtml += '<span class="yt-valign-container">';
              guideHtml += '<img src="/assets/images/pixel-vfl3z5WfW.gif" class="thumb" alt="">';
              guideHtml += '<span class="display-name no-count"><span>No subscriptions</span></span>';
              guideHtml += '</span></a></li>';
            }
          }
        } else {
          log('Request failed or no response');
          if (container) {
            html = '<p class="subscriptions-loading">No subscriptions</p>';
          }
          if (guideContainer) {
            guideHtml = '<li class="vve-check guide-channel overflowable-list-item">';
            guideHtml += '<a class="guide-item yt-uix-sessionlink yt-valign spf-link" href="#">';
            guideHtml += '<span class="yt-valign-container">';
            guideHtml += '<img src="/assets/images/pixel-vfl3z5WfW.gif" class="thumb" alt="">';
            guideHtml += '<span class="display-name no-count"><span>No subscriptions</span></span>';
            guideHtml += '</span></a></li>';
          }
        }
      } catch (err) {
        log('ERROR: ' + (err.message || err));
        if (container) {
          html = '<p class="subscriptions-loading">Error loading</p>';
        }
        if (guideContainer) {
          guideHtml = '<li class="vve-check guide-channel overflowable-list-item">';
          guideHtml += '<a class="guide-item yt-uix-sessionlink yt-valign spf-link" href="#">';
          guideHtml += '<span class="yt-valign-container">';
          guideHtml += '<img src="/assets/images/pixel-vfl3z5WfW.gif" class="thumb" alt="">';
          guideHtml += '<span class="display-name no-count"><span>Error</span></span>';
          guideHtml += '</span></a></li>';
        }
      }
      
      log('Updating DOM...');
      if (container) {
        container.innerHTML = html;
        log('Right sidebar updated');
      }
      if (guideContainer) {
        guideContainer.innerHTML = guideHtml;
        log('Left sidebar updated');
      }
      log('Done');
    };

    log('Sending request...');
    xhr.send();
  }

  if (typeof window.addEventListener === 'function') {
    window.addEventListener('load', loadSubscriptions);
  } else if (typeof window.attachEvent === 'function') {
    window.attachEvent('onload', loadSubscriptions);
  } else {
    window.onload = loadSubscriptions;
  }
})();
