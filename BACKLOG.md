# Backlog — Apple Container Provider

Ziel: roxy als TLS- und Routing-Layer vor [apple/container](https://github.com/apple/container)
nutzen, analog zu dem, was der Docker-Provider heute für Docker Compose leistet.

Branch: `feat/apple-container-provider` · Upstream: `rbas/roxy` @ `v1.0.2` (`02dd83e`)

---

## Ausgangsstand (verifiziert am 2026-08-05)

### Was ohne Codeänderung schon funktioniert

| Fakt | Beleg |
|---|---|
| `ProxyTarget` akzeptiert `IP:Port` und `hostname:Port` | `src/domain/value_objects/proxy_target.rs:37-56`, Tests ab `:97` |
| Upstream-Connect nutzt System-Resolver (HTTP + WebSocket) | `src/daemon/proxy.rs:372` (`HttpConnector`), `:148` (`TcpStream::connect`) |
| roxy-DNS bindet auf **1053**, nicht 53 — kein Konflikt mit Apples Resolver | `src/config.rs:12-13`, `src/infrastructure/dns/macos.rs:47` |
| Provider-Abstraktion ist erweiterbar ohne Eingriff in Proxy/TLS/DNS | `src/application/ports/registration_provider.rs` (2 Methoden), `src/application/provider_registry.rs` |

Manuell prüfbar: `roxy register myapp.roxy --route "/=192.168.65.6:8080"` sollte
sofort funktionieren. → siehe **A1**.

### Was bricht

Drei Annahmen des Docker-Providers gelten bei Apple Container nicht:

| Annahme in roxy | Realität bei Apple Container |
|---|---|
| Target ist `127.0.0.1:{host_port}` aus Port-Publishing (`src/infrastructure/docker/discovery.rs:38-40`) | Kein Port-Mapping. Jeder Container hat eigene IP (`status.networks[0].ipv4Address`) |
| Qualifikation via `com.docker.compose.project` / `.service` (`discovery.rs:63-65`) | Kein Compose. Nur `roxy.enable` / `roxy.domain` bleibt als Pfad |
| Watcher hängt am bollard-**Event-Stream** (`src/infrastructure/docker/watcher.rs`) | `container` hat **kein** `events`-Subcommand → Polling nötig |

### Umgebung

- `container` CLI 1.2.0 (`/opt/homebrew/bin/container`)
- Container-Subnetz ist **nicht stabil**: derselbe Container `omniroute` lag am 2026-08-05
  auf `192.168.65.6/24` und nach einem Neustart am 2026-08-06 auf `192.168.64.2/24`.
  Weder IP noch Subnetz dürfen hartkodiert oder gecacht werden. Belegt E4 empirisch.
- `container ls -a --format json` liefert `status.networks[]`, `status.state`, `configuration.labels`
- `container run -l key=value` wird unterstützt
- Rust 1.96.1, Crate ist `edition = "2024"`

---

## Offene Entscheidungen

**E1 — Braucht es Apples DNS überhaupt?**
Wenn roxy direkt auf die Container-IP proxyt, ist `container system dns create` für den
Host-Zugriff nicht nötig. Es bleibt nur für Container-zu-Container-Auflösung relevant.
Vorschlag: IP-Targets als Default, Apples DNS explizit ausserhalb des Scopes.

**E2 — Zielport-Ermittlung.**
Ohne Port-Publishing gibt es keine `host_port_mappings`. Woher kommt der Container-Port?
Kandidaten: Label `roxy.port`, `ExposedPorts` aus dem Image-Config, oder Pflichtangabe.
Vorschlag: Label `roxy.port` mit Fallback auf einen einzelnen exposed port; Skip bei
Mehrdeutigkeit mit klarer Meldung.

**E3 — Poll-Intervall und Diffing.**
Kein Event-Stream heisst: Intervall wählen, gegen letzten Stand diffen, Reload nur bei
echter Änderung. Vorschlag: 2s Default, konfigurierbar, Reload nur wenn sich das
Registrierungs-Set tatsächlich unterscheidet.

**E4 — IP-Wechsel bei Restart.**
Apple Container vergibt beim Neustart potenziell eine andere IP. Der Poller muss
Änderungen an bestehenden Registrierungen erkennen, nicht nur Add/Remove.

---

## Backlog

### A — Validierung vor dem Bau

- [x] **A1** End-to-End-Test ohne Codeänderung — **bestanden für HTTP** (2026-08-06).
      Details unter „Ergebnis A1". Die Grundannahme trägt: roxy proxyt unverändert
      auf eine Apple-Container-IP.
- [ ] **A1b** HTTPS-Pfad gegen ein Container-Ziel. Ungetestet, weil `roxy install` die
      Root-CA erst nach einem Root-Check anlegt (`src/application/install.rs:43-51`) —
      braucht `sudo`. Risiko gering: TLS-Terminierung ist unabhängig davon, ob das
      Upstream-Ziel `127.0.0.1` oder eine Container-IP ist.
- [ ] **A2** Prüfen, ob roxy-Resolver (`/etc/resolver/roxy`) und ein Apple-Container-Resolver
      koexistieren. Erwartung laut Code: ja. Gegenprobe mit `scutil --dns`. Braucht `sudo`.
- [ ] **A3** Verhalten bei gestopptem/neu gestartetem Container beobachten: bleibt die
      Registrierung stehen, was liefert der Proxy (Fehlerseite vs. Hänger)?
- [ ] **A4** Hostname-Ziel (statt IP) gegen eine Apple-DNS-Domain testen. Bisher nur
      IP-Ziele verifiziert; der Code-Pfad ist derselbe, der Resolver-Pfad nicht.

#### Ergebnis A1

Aufbau ohne `sudo`: eigene Config mit `http_port = 8080`, `https_port = 8443`,
`dns_port = 15353` und allen `[paths]` in einem Scratch-Verzeichnis, dann
`roxy -c <config> start --foreground`. Registrierung über die normale CLI:

```
roxy -c <config> register omniroute.roxy --route "/=192.168.64.2:20128"
```

| Prüfung | Ergebnis |
|---|---|
| Direkt an den Container (Baseline) | `307` |
| Durch roxy, `Host: omniroute.roxy` | `307`, identische Upstream-Header, korrekter Body |
| Durch roxy, `Host: nope.roxy` (Kontrolle) | `404` — Routing greift namensbasiert, kein Zufallstreffer |

Nebenbefund: `roxy register` funktioniert ohne Root und warnt sauber, dass HTTPS
mangels CA deaktiviert bleibt, statt abzubrechen. Das unprivilegierte Setup taugt
damit als Testharness für B und C, ohne das System anzufassen.

### B — Container-Adapter

- [ ] **B1** `src/infrastructure/apple_container/cli.rs` — dünner Wrapper um
      `container ls -a --format json`. Serde-Modelle nur für die tatsächlich gebrauchten
      Felder (`configuration.id`, `configuration.labels`, `status.state`,
      `status.networks[].ipv4Address`). CIDR-Suffix (`/24`) abschneiden.
      Typisierte Fehler für: CLI nicht gefunden, Exit != 0, JSON-Parse-Fehler.
- [ ] **B2** `discovery.rs` — `evaluate_container`-Äquivalent gegen Container-IP statt
      `127.0.0.1:host_port`. Qualifikation über `roxy.enable=true` + `roxy.domain`.
      Skip mit Begründung, wenn Container nicht läuft oder keine IPv4 hat.
- [ ] **B3** `provider.rs` — `RegistrationProvider` implementieren, Shape analog
      `DockerProvider` (Zustand in `state()`, damit der Watcher ihn teilen kann).
- [ ] **B4** Unit-Tests gegen eingefrorene JSON-Fixtures aus echtem `container ls`-Output.
      Kein Aufruf der CLI im Test.

### C — Watcher

- [ ] **C1** Poll-Loop mit `CancellationToken`, Intervall aus Config (E3).
- [ ] **C2** Diffing gegen den letzten Stand; Reload nur bei echter Änderung des
      Registrierungs-Sets — inklusive IP-Änderung bei gleichem Container (E4).
- [ ] **C3** Fehler im Poll-Loop loggen und weiterlaufen, nicht abbrechen. Ein temporär
      nicht erreichbarer `container`-Daemon darf den roxy-Daemon nicht mitreissen.

### D — Konfiguration und Verdrahtung

- [ ] **D1** `AppleContainerConfig { enabled: bool, poll_interval_secs: u64 }` in
      `src/config.rs` neben `DockerConfig:102-106`, Default `enabled = false`.
- [ ] **D2** Wiring in `src/daemon/lifecycle.rs:56-73` und `:93-105` — gleiche Struktur wie
      der Docker-Zweig, inklusive Warn-and-Continue, wenn die CLI fehlt.
- [ ] **D3** Kollisionsverhalten bei gleichem Domainnamen aus mehreren Providern klären.
      `ProviderRegistry::load` konkateniert nur (`provider_registry.rs:40-57`) — wer gewinnt?
      Aktuell ungeklärt, muss vor dem Merge entschieden sein.

### E — Dokumentation

- [ ] **E1** `docs/apple-container.md` analog zu `docs/docker.md`.
- [ ] **E2** README-Feature-Tabelle und Roadmap-Abschnitt ergänzen.
- [ ] **E3** CHANGELOG-Eintrag (Repo nutzt `git-cliff`, Conventional Commits sind Pflicht).

---

## Nicht im Scope

- Container-zu-Container-Auflösung über roxy
- Upstream-PR an `rbas/roxy` — erst nach A1–C3, und dann müssten alle Texte auf Englisch
- Ersatz oder Änderung des bestehenden Docker-Providers
- Apples `container system dns`-Integration (siehe E1 unter Offene Entscheidungen)

---

## Aufwandsschätzung

| Block | LOC (inkl. Tests) |
|---|---|
| B — Adapter | 180–220 |
| C — Watcher | 80–120 |
| D — Config/Wiring | 40–60 |
| **Summe** | **300–400** |

---

## Verifikationsnotizen

- Alle Datei-/Zeilenangaben gegen `v1.0.2` (`02dd83e`) geprüft. Bei Rebase auf Upstream
  neu verifizieren.
- `cargo build --release` läuft durch (Rust 1.96.1, ~75s kalt).
- Noch **nicht** geprüft: ob `container ls --format json` bei mehreren Containern und
  beim Netzwerk-Typ `bridge` dasselbe Schema liefert (getestet nur mit einem Container,
  `variant: reserved`). Ebenso ungeprüft: WebSocket-Weiterleitung an ein Container-Ziel.
- Die Einordnung von `portless`, `dev-bind` und `rust-rpxy` stammt aus einer Recherche
  ausserhalb dieses Repos und ist nicht gegen deren Quellcode verifiziert.
