// APEX-OS Firefox default preferences.
//
// Installed as /usr/lib64/firefox/browser/defaults/preferences/zz-apex-default-prefs.js
// — the SAME mechanism Fedora uses, with a name that sorts after theirs. Files
// in that directory are read in sorted order and later files win, and Fedora's
// is `firefox-redhat-default-prefs.js`, so `zz-` is what puts APEX last.
//
// WHAT THIS FIXES
// Fedora's firefox package ships these two lines:
//
//   pref("browser.startup.homepage",
//        "data:text/plain,browser.startup.homepage=https://start.fedoraproject.org/");
//   pref("browser.newtabpage.pinned",
//        '[{"url":"https://start.fedoraproject.org/","title":"Fedora Project - Start Page"}]');
//
// The first is why every new Firefox profile on APEX-OS opened on the literal
// text `data:text/plain,browser.startup.homepage=https://start.fedoraproject.org/`
// in the URL bar. It is a Fedora packaging quirk — the homepage is set to a
// data: URL whose *content* is a pref assignment — and it looks like a bug in
// the OS, on the browser APEX-OS bakes in as the default. The second pins a
// Fedora Start Page tile onto the new-tab page.
//
// Neither belongs in a distro that is not Fedora. `about:home` is Firefox's own
// default: the Firefox Home page with the search field.
//
// These are DEFAULTS, not locks. Anyone who sets a homepage in Preferences, or
// pins their own tiles, overrides them and keeps that across updates.
pref("browser.startup.homepage", "about:home");
pref("browser.newtabpage.pinned", "[]");

// Fedora blanks this too, but be explicit: an empty override URL means no
// "what's new" page hijacking the first start after an update. On an image-based
// OS the browser is updated by the image, so a per-browser upgrade page is noise.
pref("startup.homepage_override_url", "");
