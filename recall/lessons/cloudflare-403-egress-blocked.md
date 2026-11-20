# cloudflare-403-egress-blocked
- trigger: Cloudflare 403 "Your request was blocked" on direct curl AND browser.open AND the cloud browser, even with no auth and even on the site root — the key is never even evaluated
- rule: a 403 on every direct request means the VM's egress is blocked, not the key or credentials; stop retrying direct requests; use the ladder: web.py TinyFish fetch, then browser task, then vault Worker service_request, then Pi sandbox egress
- applies: any site, any API, this VM
