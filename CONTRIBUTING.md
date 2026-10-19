# Contributing

Thank you for your interest. This project is largely complete, so changes are reviewed
slowly and only when they fix a defect or improve compatibility.

## Reporting a problem

Open an issue and include:

1. Your Windows version.
2. The game release (US release is the only tested one).
3. The display mode you used (`windowed` or `fullscreen`) and the scale value.
4. The last 50 lines of `%TEMP%\pn_modern\pn_modern.log`.

Do not attach game files.

## Sending a change

1. Open an issue first for anything larger than a typo fix.
2. Keep the change small. One subject per pull request.
3. Run the checks in the README under "Build and test". All of them must pass with zero
   warnings.
4. Do not add game data, game text or extracted game files. `git diff --stat` must not
   show any file under `original/` or `extracted/`.

## Code style

- Rust: `cargo fmt`, and `cargo clippy --all-targets` with zero warnings. The lint levels
  live in `Cargo.toml` and `clippy.toml`.
- Python: keep `tools/*.py` short and single-purpose. Extend an existing script before you
  add a new one. A format claim needs an assertion in `tools/unpack.py --selftest`.
- Documentation: use simple English. Short sentences, active voice, one instruction per
  step.

## Licence of your contribution

By sending a pull request you agree that your contribution is available under
MIT, the same terms as the rest of the repository.
