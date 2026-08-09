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

**E2 — Zielport-Ermittlung.** *(Datenlage geklärt in B1, Entscheidung offen)*
Apple Container kennt sehr wohl Port-Publishing: `configuration.publishedPorts[]` mit
`containerPort`, `hostPort`, `hostAddress`, `proto`. Anders als bei Docker ist es aber
**optional** — jeder Container ist ohnehin über seine eigene IP erreichbar. Verifiziert:
ein Container mit `-p` liefert einen Eintrag, ein Container ohne `-p` eine leere Liste.

Damit gibt es zwei brauchbare Quellen und eine Rangfolge ist nötig. Vorschlag:
1. Label `roxy.port` — explizit, gewinnt immer
2. genau ein Eintrag in `publishedPorts` → dessen `containerPort` (Ziel bleibt die
   Container-IP, nicht `127.0.0.1:hostPort`)
3. sonst Skip mit klarer Meldung

Bewusst **nicht** genutzt: `hostPort` als Ziel. Das würde Dockers Umweg nachbauen und
den einzigen strukturellen Vorteil von Apple Container wegwerfen.

**So in B2 implementiert.** Offen bleibt nur, ob Punkt 2 zu grosszügig ist: ein
Container mit genau einem publizierten Port wird ohne weiteres Zutun registriert,
sobald er `roxy.enable=true` trägt.

**E3 — Poll-Intervall.** *(erledigt)*
Default 2s, konfigurierbar über `apple_container.poll_interval_secs`. Ein Tick kostet
einen Prozessaufruf von `container ls`, also nicht beliebig klein wählen. In der
Praxis lag die Latenz von `container run` bis zum ersten erfolgreichen Request bei
~6s, wovon der grössere Teil auf den Container-Start entfiel.

**E4 — IP-Wechsel bei Restart.** *(erledigt)*
Bestätigt und gelöst: der Fingerprint in C2 enthält die Route-Ziele, ein IP-Wechsel
löst deshalb einen Reload aus. Live nachgestellt, siehe „Ergebnis D".

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

- [x] **B1** `src/infrastructure/apple_container/cli.rs` — **erledigt** (2026-08-06).
      Wrapper um `container ls --all --format json`, Serde-Modelle für `id`,
      `configuration.labels`, `configuration.publishedPorts[]`, `status.state`,
      `status.networks[].ipv4Address`. CIDR-Suffix wird abgeschnitten, kaputte
      Adressen fallen einzeln weg statt die Liste zu killen. Typisierte Fehler:
      `NotFound`, `CommandFailed`, `Io`, `Parse`. Parsing ist von der
      Prozessausführung getrennt (`parse_containers`), damit es ohne CLI testbar ist.
      13 Unit-Tests gegen eine echte aufgezeichnete Ausgabe (`fixtures/container_ls.json`,
      ein laufender und ein gestoppter Container) plus ein `#[ignore]`-Test gegen die
      echte CLI, der die Argument-Schreibweise absichert.
- [x] **B2** `discovery.rs` — **erledigt** (2026-08-06). `evaluate_container` gegen
      Container-IP + `containerPort`. Qualifikation: `roxy.enable=true` **oder**
      `roxy.domain` (ein gesetztes Domain-Label ist bereits eindeutige Absicht),
      `roxy.enable=false` sticht beides. Domain aus `roxy.domain`, sonst
      `{container-id}.roxy`. Port nach der in E2 beschlossenen Rangfolge.
      Zusätzlich `roxy.wildcard` wie beim Docker-Provider. Jeder Skip trägt eine
      Begründung, die den fehlenden Knopf benennt. 17 Unit-Tests.
      Registrierungen sind `RegistrationSource::External` mit aktiviertem HTTPS.
- [x] **B3** `provider.rs` — **erledigt** (2026-08-06). `AppleContainerProvider`
      implementiert `RegistrationProvider`, Shape analog `DockerProvider`
      (`state()` für den Watcher, `cli()` für den Poll-Aufruf). Name:
      `apple-container`.
- [x] **B4** Unit-Tests gegen eingefrorene Fixtures — mit B1 erledigt.

### C — Watcher

- [x] **C1** **Erledigt** (2026-08-06). Poll-Loop mit `CancellationToken`, Intervall als
      Parameter (Config-Anbindung folgt in D1). `list_all` läuft in `spawn_blocking`,
      damit der Prozessaufruf den Async-Runtime nicht blockiert.
- [x] **C2** **Erledigt.** Diffing über einen Fingerprint aus Pattern **und** Route-Zielen.
      Reload-Nudge nur bei echter Änderung. Der Docker-Watcher vergleicht an dieser
      Stelle nur Domainnamen (`docker/watcher.rs:226-228`) und würde einen IP-Wechsel
      bei gleichbleibendem Namen übersehen — für Apple Container wäre das der
      Normalfall, siehe E4. Ein Test hält das explizit fest.
- [x] **C3** **Erledigt.** Fehler im Poll-Loop werden geloggt, der letzte gute Zustand
      bleibt stehen, der nächste Tick versucht es erneut.

### D — Konfiguration und Verdrahtung

- [x] **D1** **Erledigt** (2026-08-06). `AppleContainerConfig { enabled, poll_interval_secs }`
      in `src/config.rs`, Default `enabled = false`, Intervall 2s. `validate()` lehnt
      Intervall 0 ab, aber nur wenn aktiviert. Sektion darf komplett fehlen.
- [x] **D2** **Erledigt.** Wiring in `src/daemon/lifecycle.rs` analog zum Docker-Zweig,
      inklusive Warn-and-Continue, wenn die CLI fehlt. Live verifiziert, siehe
      „Ergebnis D".
- [x] **D3** **Entschieden:** first match wins, Reihenfolge Config-Datei > Docker >
      Apple Container. Der Router nimmt den ersten Treffer (`src/daemon/router.rs:46-47`),
      und `ProviderRegistry::load` konkateniert in Registrierungsreihenfolge — der
      Apple-Container-Provider wird deshalb bewusst zuletzt hinzugefügt. Explizite
      Nutzerkonfiguration schlägt damit jede Auto-Discovery.
      Offen als spätere Verbesserung: eine Warnung, wenn zwei Provider dasselbe
      Pattern liefern. Betrifft auch den bestehenden Docker-Provider.

#### Ergebnis D

Live gegen Apple Container 1.2.0, unprivilegierte Sandbox-Config mit
`apple_container.enabled = true` und `poll_interval_secs = 2`:

| Schritt | Beobachtung |
|---|---|
| Daemon gestartet, kein passender Container | `roxytest.roxy` → `404` |
| `container run -l roxy.enable=true` | nach ~6s (Boot + Poll) → `307` |
| Registrierung im Log | `roxytest.roxy => /=192.168.64.3:20128` — Container-IP, `containerPort` |
| Container ohne Labels (`omniroute`) | übersprungen, mit Begründung im Debug-Log |
| `container stop` | nach ~2s wieder `404`, „registration removed" im Log |
| `container start` | Container kam auf `192.168.64.4` zurück, Watcher registrierte neu, Proxy wieder `307` |

Der letzte Fall ist der Beleg für C2: gleicher Name, neue IP. Ein Diff nur über
Domainnamen hätte den Wechsel verschluckt und der Proxy zeigte weiter auf `.3`.
Der Fall trat beim allerersten Neustart von selbst auf.

Ausserdem bestätigt: „added"/„removed" erscheinen genau einmal pro Änderung, nicht
bei jedem Tick — das Diffing arbeitet wie vorgesehen.

Nebenbefund: `roxy list` zeigt nur Registrierungen aus der Config-Datei, nicht die
dynamisch entdeckten. Gilt für den Docker-Provider genauso. Ob das gewollt ist,
wäre in E zu dokumentieren oder als eigener Punkt zu behandeln.

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
- `cargo build --release` läuft durch (Rust 1.96.1, ~75s kalt). `cargo test`: 251 grün.
  `cargo clippy --all-targets` meldet nur Vorbestehendes in `src/application/testkit.rs`.
- Schema für laufende **und** gestoppte Container verifiziert: gestoppt heisst
  `status.state = "stopped"`, `status.networks = []`, kein `startedDate`.
- Noch **nicht** geprüft: Netzwerk-Typ `bridge` (Fixture nur mit `variant: reserved`),
  und WebSocket-Weiterleitung an ein Container-Ziel.
- Die Einordnung von `portless`, `dev-bind` und `rust-rpxy` stammt aus einer Recherche
  ausserhalb dieses Repos und ist nicht gegen deren Quellcode verifiziert.
