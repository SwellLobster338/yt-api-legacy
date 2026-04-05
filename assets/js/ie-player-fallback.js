/**
 * IE Fallback for WEBP player icons
 * Forces PNG sprites in old Internet Explorer versions
 */
(function() {
  // Detect old IE (IE11 and below)
  var isOldIE = false;
  
  // Method 1: Conditional compilation (IE10 and below)
  /*@cc_on
    isOldIE = true;
  @*/
  
  // Method 2: documentMode (IE11)
  if (!isOldIE && !!document.documentMode) {
    isOldIE = true;
  }
  
  if (isOldIE) {
    var style = document.createElement('style');
    style.type = 'text/css';
    
    // List of all player button classes that use player-common sprite
    var selectors = [
      '.ytp-button-play',
      '.ytp-button-pause',
      '.ytp-button-replay',
      '.ytp-button-stop',
      '.ytp-button-next',
      '.ytp-button-prev',
      '.ytp-button-volume',
      '.ytp-button-fullscreen-enter',
      '.ytp-button-fullscreen-exit',
      '.ytp-settings-button',
      '.ytp-settings-button-active',
      '.ytp-subtitles-button',
      '.ytp-subtitles-button-active',
      '.cc-international .ytp-subtitles-button',
      '.cc-international .ytp-subtitles-button-active',
      '.ytp-remote-button',
      '.ytp-remote-button-active',
      '.ytp-button-like',
      '.ytp-button-dislike',
      '.ytp-button-share',
      '.ytp-button-info',
      '.ytp-button-watch-later',
      '.ytp-button-playlist',
      '.playlist-loaded .ytp-button-playlist',
      '.annotation-close-button',
      '.ytp-button-expand .ytp-button-playlist-icon',
      '.ytp-button-collapse .ytp-button-playlist-icon',
      '.ytp-remote-display-status-icon',
      '.ytp-button-remote-maximize-icon',
      '.ytp-size-toggle-small',
      '.ytp-size-toggle-large'
    ];
    
    // Build CSS rules with :focus and :hover states
    var rules = [];
    selectors.forEach(function(selector) {
      rules.push(selector + ',');
      rules.push(selector + ':focus,');
      rules.push(selector + ':hover{background-image:url(/assets/images/player-common-vflp3GS9A.png)!important}');
    });
    
    style.innerHTML = rules.join('');
    document.getElementsByTagName('head')[0].appendChild(style);
    
    // For watch page like/dislike/subscribe icons (<img> tags), replace with visible placeholders
    var watchIcons = document.querySelectorAll('.yt-uix-button-icon-watch-like, .yt-uix-button-icon-watch-dislike, .yt-uix-button-icon-subscribe');
    for (var i = 0; i < watchIcons.length; i++) {
      var wrapper = watchIcons[i].parentNode;
      if (wrapper && wrapper.className.indexOf('yt-uix-button-icon-wrapper') !== -1) {
        wrapper.style.display = 'inline-block';
        wrapper.style.backgroundRepeat = 'no-repeat';
        
        // Determine which icon and set appropriate styles
        if (watchIcons[i].className.indexOf('watch-like') !== -1) {
          // Like icon - thumbs up
          wrapper.style.backgroundImage = 'url(/assets/images/www-hitchhiker-vflLQ2UOr.png)';
          wrapper.style.width = '16px';
          wrapper.style.height = '16px';
          wrapper.style.backgroundPosition = '-138px -138px';
        } else if (watchIcons[i].className.indexOf('watch-dislike') !== -1) {
          // Dislike icon - thumbs down
          wrapper.style.backgroundImage = 'url(/assets/images/www-hitchhiker-vflLQ2UOr.png)';
          wrapper.style.width = '16px';
          wrapper.style.height = '16px';
          wrapper.style.backgroundPosition = '-158px -138px';
        } else if (watchIcons[i].className.indexOf('subscribe') !== -1) {
          // Subscribe icon - plus sign or checkmark
          wrapper.style.backgroundImage = 'url(/assets/images/www-hitchhiker-vflLQ2UOr.png)';
          wrapper.style.width = '16px';
          wrapper.style.height = '16px';
          wrapper.style.backgroundPosition = '-178px -138px';
        }
        
        // Hide the img tag since we're using background
        watchIcons[i].style.display = 'none';
      }
    }
  }
})();
