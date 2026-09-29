# Bewusste Veröffentlichung

Stand: 29. September 2026. Eigentümer und Freigabe: gcolicig.

## Entscheidung

Entwicklung und Stabilisierung sollen privat erfolgen. Eine spätere Veröffentlichung ist geplant, aber weder Termin noch automatische Freigabe sind beschlossen. Commits, grüne CI, Tags und Releases ändern die Sichtbarkeit nicht. Agenten dürfen vorbereiten und prüfen, aber ohne ausdrückliche Freigabe weder öffentlich schalten noch deployen oder Pakete publizieren.

## Freigabekriterien

- [ ] Kernumfang abgeschlossen; keine bekannten kritischen Fehler oder Datenverluste.
- [ ] Reproduzierbarer Build und Installation aus einem frischen Checkout.
- [ ] Kernabläufe und Fehlerpfade getestet; Nachweise an eine vollständige Kandidaten-SHA gebunden.
- [ ] Lizenz, Herkunft, Drittanbieter- und Modellbedingungen vollständig geprüft.
- [ ] README, Voraussetzungen, Beispiele und bekannte Grenzen entsprechen der Implementierung.
- [ ] Alle öffentlich werdenden Branches, Tags, Git-Historie, Issues, PRs und Artefakte auf Geheimnisse, persönliche Daten und interne Konfigurationen geprüft. Nur den aktuellen Dateistand zu prüfen genügt nicht. Exponierte Zugangsdaten rotieren.
- [ ] Öffentlicher Name, Supportumfang und Wartungserwartung entschieden.
- [ ] Eigentümer hat diese SHA und den Veröffentlichungsumfang ausdrücklich freigegeben.

## Ablauf

Privat entwickeln -> Release-Kandidat festlegen -> Kriterien mit Nachweisen prüfen -> Entscheidung des Eigentümers dokumentieren -> manuell öffentlich schalten -> freigegebenen Release veröffentlichen. Neue Änderungen nach der Prüfung erfordern erneute Prüfung der betroffenen Kriterien. Eine spätere Privatstellung entfernt bereits erstellte öffentliche Kopien nicht.

CI erhält keine administrativen Tokens für Sichtbarkeitsänderungen. Diese Checkliste ist eine Arbeitsregel, keine technische Zugriffssperre. Optional kann ein separates öffentliches Release-Repository nur kuratierte, freigegebene Inhalte aufnehmen; das Entwicklungsrepository bleibt dann privat.

## Freigabeprotokoll

Status: **NICHT FREIGEGEBEN**

| Feld | Wert |
|---|---|
| Kandidaten-SHA | offen |
| Öffentlicher Name | offen |
| Prüfbericht | offen |
| Bekannte Grenzen | offen |
| Entscheidung und Datum | offen |

## Projektspezifische Kriterien

- [ ] Apple-Container-Featurebranch prüfen und kontrolliert in den Release-Branch integrieren.
- [ ] Discovery, DNS/HTTPS, Domainkonflikte, Timeouts und Watch-Loop reproduzierbar testen.
- [ ] Upstream-Herkunft dokumentieren und Upstream-PR oder eigenständigen Release bewusst wählen.



## Versionierte Veröffentlichungsabsicht

Die Datei [publication.ini](publication.ini) wird bewusst versioniert und nicht in .gitignore eingetragen. Alle Checkouts und Agenten erhalten damit denselben überprüfbaren Wert. Eine lokale Kopie oder Template-Datei ist nicht erforderlich.

```ini
[publication]
public = false
```

- `false`: keine Veröffentlichung freigegeben; Entwicklung und Stabilisierung sollen privat bleiben.
- `true`: Veröffentlichung vorbereiten und ausdrückliche Eigentümerfreigabe für die konkrete Kandidaten-SHA einholen. Der Wert allein ist keine Freigabe.
- Fehlende Datei, fehlende Sektion oder fehlender Schlüssel, ungültige INI-Syntax sowie jeder Wert ausser exakt `true` gelten als `false`.

Der Wert beschreibt die Absicht, nicht die tatsächliche GitHub-Sichtbarkeit. Insbesondere wird ein bereits öffentliches Repository (aktuell roxy) durch `false` nicht automatisch privat. Auch `true`, grüne CI, Tags und Releases schalten nichts automatisch öffentlich. Eine Auswertung oder Automation ist derzeit nicht implementiert. Vor jeder Publikation müssen Wert und Freigabenachweis an derselben geprüften Kandidaten-SHA kontrolliert werden.
