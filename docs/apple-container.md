# Apple Container Integration

Roxy can automatically discover containers managed by
[apple/container](https://github.com/apple/container) and register
them as `.roxy` domains, so starting a container makes it available
at `https://<name>.roxy` with a trusted certificate.

macOS only — Apple Container does not exist on other platforms.

## Enabling Apple Container Integration

Add the `[apple_container]` section to `/etc/roxy/config.toml`:

```toml
[apple_container]
enabled = true
```

Then restart the daemon:

```bash
sudo roxy restart
```

If the `container` CLI is not on `PATH`, Roxy logs a warning and
keeps running without the integration.

## Quick Start

```bash
# Start a container and opt it in
container run -d --name web -l roxy.enable=true -p 3000:3000 my-image

# Roxy picks it up within one poll interval
open https://web.roxy
```

No `roxy register` needed.

## How Auto-Discovery Works

Every container is evaluated with these rules, in order:

1. **Not running** — skip (there is no address to route to)
2. **`roxy.enable=false`** label — skip (explicit opt-out)
3. **`roxy.enable=true`** or a **`roxy.domain`** label — register
4. **Otherwise** — skip

Unlike Docker there is no Compose metadata to infer intent from, so
registration is always opt-in via labels. Containers you never
labelled are left alone.

Skipped containers are logged at debug level with the reason, which
names the label that would fix it. Run the daemon with `-v` to see
them.

### Requirements for Auto-Discovery

- The container must be **running** and have an IPv4 address
- Roxy must be able to determine a port — see
  [Port Resolution](#port-resolution)
- The domain must be valid, meaning it ends in `.roxy` — see
  [Domain Name Resolution](#domain-name-resolution)

## How This Differs From Docker

Apple Container gives every container its own routable IP address,
so Roxy proxies **directly to the container**:

|                   | Docker                       | Apple Container            |
| ----------------- | ---------------------------- | -------------------------- |
| Proxy target      | `127.0.0.1:<host port>`      | `<container IP>:<port>`    |
| Published port    | Required                     | Optional                   |
| Discovery trigger | Event stream                 | Polling                    |
| Opt-in            | Automatic for Compose        | Always label-driven        |

Two consequences worth knowing:

**Publishing ports is optional.** `-p` is only needed if you also
want to reach the container directly from the host. Roxy does not
need it as long as `roxy.port` is set.

**Discovery is polled, not pushed.** Apple Container has no event
stream, so Roxy re-lists containers on an interval. Expect up to one
interval of delay before a new container is reachable.

## Configuration

| Key                  | Default | Description                             |
| -------------------- | ------- | --------------------------------------- |
| `enabled`            | `false` | Enable Apple Container auto-discovery   |
| `poll_interval_secs` | `2`     | Seconds between `container ls` polls    |

```toml
[apple_container]
enabled = true
poll_interval_secs = 2
```

Each tick costs one `container ls` process invocation, so avoid very
small values. Raising it only delays discovery; it does not affect
proxying of already-registered containers.

## Labels Reference

Set labels with `container run --label` (or `-l`).

| Label           | Values           | Description                            |
| --------------- | ---------------- | -------------------------------------- |
| `roxy.enable`   | `true` / `false` | Force opt-in or opt-out                |
| `roxy.domain`   | e.g. `app.roxy`  | Override the derived domain            |
| `roxy.port`     | e.g. `8080`      | Which container port to proxy          |
| `roxy.wildcard` | `true`           | Register as wildcard (`*.domain`)      |

### Examples

**Opt in, derive everything:**

```bash
container run -d --name web -l roxy.enable=true -p 3000:3000 my-image
# Domain: web.roxy  ->  <container IP>:3000
```

**Custom domain, no published port:**

```bash
container run -d --name api \
  -l roxy.domain=shop.roxy \
  -l roxy.port=8080 \
  my-image
# Domain: shop.roxy  ->  <container IP>:8080
```

A `roxy.domain` label is itself an opt-in, so `roxy.enable=true` is
redundant here.

**Pick one of several published ports:**

```bash
container run -d --name api \
  -l roxy.enable=true -l roxy.port=3000 \
  -p 3000:3000 -p 9090:9090 \
  my-image
```

**Wildcard subdomains:**

```bash
container run -d --name web \
  -l roxy.enable=true -l roxy.wildcard=true \
  -p 3000:3000 my-image
# Registers *.web.roxy
```

**Opt out:**

```bash
container run -d --name db -l roxy.enable=false postgres
```

`roxy.enable=false` wins over every other label, including
`roxy.domain`.

## Domain Name Resolution

Roxy determines the domain in this order:

1. **`roxy.domain` label** — used as-is, must end with `.roxy`
2. **Container name** — `<name>.roxy`

Container names containing characters that are invalid in a domain
(underscores, for example) cannot be derived automatically. Set
`roxy.domain` explicitly for those.

## Port Resolution

Roxy determines the target port in this order:

1. **`roxy.port` label** — used as-is
2. **Exactly one published port** — its *container* port
3. **Otherwise** — skip, with the reason logged

Note that step 2 uses the container port, not the host port. Given
`-p 8080:3000`, Roxy proxies to `<container IP>:3000`, ignoring the
host-side mapping.

A container that publishes several ports without a `roxy.port` label
is ambiguous, so Roxy skips it and lists the candidates in the log.

## Restarts and Changing IP Addresses

Apple Container may assign a container a different IP — sometimes a
different subnet — each time it starts. Roxy compares proxy targets,
not just domain names, so a restarted container is re-registered at
its new address automatically. No action needed.

## Using It Alongside Docker

Both integrations can be enabled at once. If the same domain comes
from more than one source, the first match wins in this order:

1. Config file (`roxy register`)
2. Docker
3. Apple Container

In other words, anything you registered explicitly takes precedence
over auto-discovery.

## Troubleshooting

**A container is not picked up.** Run the daemon with `-v` and check
the log — every skip states its reason. The usual causes are a
missing `roxy.enable` label, no resolvable port, or a container name
that cannot form a valid domain.

**`roxy list` does not show a discovered domain.** Discovered domains
live in the running daemon, not in the config file. `list` queries the
daemon and marks them `[external]`, but falls back to config-only when
the daemon is unreachable — in which case they are missing. Check that
the daemon is running with `roxy status`.

**The domain resolves but the request hangs or fails.** Check that
the service inside the container listens on all interfaces
(`0.0.0.0`) rather than only on `127.0.0.1`. A container-local
loopback binding is not reachable from the host.
