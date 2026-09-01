#!/usr/bin/env python3
"""Strip site identity out of capture output so a finding can be written down.

An opaque label on the file is not enough. A capture log names every host it
fetched from, a tree dump carries class names, ids and CDN URLs, and any one of
those identifies the site as surely as its name would. This replaces them with
stable per-file tokens: two mentions of the same host stay recognisably the same
host, so an error about a script is still traceable to the request that fetched
it, while nothing says which site it was.

Deliberately conservative. It would rather redact a word that did not need it
than leak one that did, because the cost is asymmetric.
"""
import argparse
import re
import sys

URL = re.compile(r"\b[a-zA-Z][a-zA-Z0-9+.-]*://[^\s\"'<>)\]]+")
# A bare hostname: at least two labels and a plausible TLD.
HOST = re.compile(r"\b(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?\.)+[a-zA-Z]{2,24}\b")
# Left alone: these name the engine's own machinery, not the site.
KEEP = {
    "blitz-script", "blitz-dom", "blitz-paint", "blitz-net", "chuzz-gui",
    "127.0.0.1", "localhost", "example.com",
}


def scrubber():
    hosts, urls = {}, {}

    def host_token(name):
        if name in KEEP or name.endswith(".rs") or name.endswith(".js") or name.endswith(".css"):
            return name
        if name not in hosts:
            hosts[name] = f"<host-{len(hosts) + 1}>"
        return hosts[name]

    def line(text):
        def on_url(m):
            raw = m.group(0)
            if raw not in urls:
                urls[raw] = f"<url-{len(urls) + 1}>"
            return urls[raw]

        text = URL.sub(on_url, text)
        return HOST.sub(lambda m: host_token(m.group(0)), text)

    return line, hosts, urls


def main():
    parser = argparse.ArgumentParser()
    # A site's own name appears in its class names and element ids, where no
    # amount of URL matching will find it. A tree dump leaked exactly one such
    # token after hostname scrubbing alone, so the caller passes the words to
    # redact; they come from the local-only corpus map and are never committed.
    parser.add_argument("--identity", action="append", default=[])
    args = parser.parse_args()
    identity = [re.compile(re.escape(word), re.IGNORECASE)
                for word in args.identity if len(word) >= 3]

    scrub, hosts, urls = scrubber()
    out = []
    for raw in sys.stdin:
        line = scrub(raw)
        for pattern in identity:
            line = pattern.sub("<site>", line)
        out.append(line)
    sys.stdout.write("".join(out))
    # To stderr so it never lands in the redacted artifact itself.
    print(f"scrubbed {len(hosts)} hostnames, {len(urls)} URLs", file=sys.stderr)


if __name__ == "__main__":
    main()
