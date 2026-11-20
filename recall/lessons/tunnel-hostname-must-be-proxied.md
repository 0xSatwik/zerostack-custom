# tunnel-hostname-must-be-proxied
- trigger: tunnel URL gave ERR_CONNECTION_RESET; the DNS CNAME was created with proxied:false (DNS-only)
- rule: a tunnel hostname MUST be proxied (orange cloud) so Cloudflare's edge routes it to the tunnel; DNS-only gives ERR_CONNECTION_RESET
- applies: cloudflare tunnel, dpdns.org, any tunnel hostname
