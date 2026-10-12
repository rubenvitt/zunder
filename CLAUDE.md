# Zunder

Überblick in `README.md`, Entwicklung und Qualitätsschranken in `docs/entwicklung.md`.

## Commits

Jeder Commit ist ein [Conventional Commit](https://www.conventionalcommits.org/de/v1.0.0/): `typ(bereich): Beschreibung`, Beschreibung auf Deutsch. Die CI prüft das (Job `commits`), und semantic-release leitet daraus die Version ab: `fix:`/`perf:` → Patch, `feat:` → Minor, `typ!:` oder `BREAKING CHANGE:` → Major, alle anderen Typen (`docs`, `test`, `refactor`, `ci`, `build`, `chore`, `style`) → kein Release. Typ ehrlich wählen: Was Nutzer sehen, ist `feat` oder `fix`; reine Tests, Doku oder CI nicht.

Bereiche: `server`, `web`, `app`, `core`, `import`, `local`; weglassen, wenn es mehrere betrifft.
