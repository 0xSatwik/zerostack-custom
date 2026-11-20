# verify-repoint-with-browser-open
- trigger: after repointing the tunnel, web.py fetch still showed the old page (TinyFish fetch caches); the repoint looked broken when it was fine
- rule: TinyFish fetch caches — verify tunnel repoints with browser.open, not web.py fetch
- applies: cloudflare tunnel, any repoint
