# Security policy

## Supported versions

The latest release on the Releases page is supported. Older releases do not get fixes.

## What this project does

The patch replaces `fakecdda.dll` in the game folder. That DLL runs inside the game
process and receives all music calls. The project ships no game data and no network code.

The binaries are not code-signed. Windows SmartScreen and some antivirus programs warn
about unsigned binaries. You can build both binaries from source and compare them with the
release zip. `python tools/release.py` builds the same files and checks the PE facts.

## Reporting a vulnerability

Do not open a public issue for a vulnerability.

Use the private reporting form on the Security tab of this repository. Include:

1. The affected version.
2. A short description of the problem.
3. The steps that reproduce it.

Expect a first answer within seven days. This is a spare-time project, so a fix can take
longer.

## Out of scope

- Problems in the game itself.
- Antivirus warnings about unsigned binaries.
- Accounts, personal data or network services. The project has none.
