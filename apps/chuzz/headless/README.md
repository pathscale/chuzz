# Headless bot child

Launch one `chuzz-headless` process for one browser session. Keep that process
alive for the session, read its first stdout line as that process's descriptor,
and terminate it when the session ends. The descriptor is specific to this
process. Do not look up a descriptor by choosing the newest file.

```sh
CHUZZ_HEADLESS_MODE=bot \
CHUZZ_HEADLESS_WALL_TIMEOUT_MS=20000 \
chuzz-headless https://supplier.example/search
```

The target must be an HTTPS URL resolving only to public IP addresses. The
optional `CHUZZ_HEADLESS_ALLOWED_DOMAINS` value is a comma-separated list of
DNS hostnames. Each entry permits that hostname and its subdomains. An empty
value leaves public supplier discovery open while the private-address checks
remain active.

`CHUZZ_HEADLESS_ALLOW_FORM_SUBMIT` defaults to false. A parent may set it to
true for a lookup form it has explicitly authorized. Sensitive controls remain
blocked by the page classifier. Keep stdout reserved for the descriptor line;
the browser sends status and errors to stderr.

Bot-child limits are fixed in the host: a 20-second maximum lifetime, eight
actions, sixteen inspections, twenty-four total control requests, sixteen
queued requests, twelve inspect levels, 512 returned nodes, 20,000 live DOM
nodes, a 60 KiB semantic snapshot, 512 bytes per fill value, a
1.5-million-pixel viewport, sixteen egress tunnels, and 32 MiB through the
egress filter. The filter accepts HTTPS CONNECT tunnels only, checks each
destination and connects to a resolved public address. Plain HTTP requests
are refused.

Cookies and script storage are process-local in this mode. The existing
network provider still owns resource URL handling and its HTTP cache. In
particular, file and data resource URLs do not pass through the egress filter,
and the engine's disk cache is not disabled here. Those provider behaviors need
integrator verification before this launch mode is treated as a complete
network and persistence boundary.
