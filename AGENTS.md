# AGENTS.md — working rules for this repo

Patch and reverse-engineering workspace for **Private Nurse** (AngelSmile / GamelexGS,
2002; US release `NURSEUS.EXE`). The shipped product is a Windows patch that lets the game
run in a scalable window on modern Windows.

`original/` is the pristine game directory — never modify, rename or delete anything in it.
It is git-ignored and hash-pinned by `E00`. `extracted/` is local scratch and is ignored:
game text must never be committed (see "Copyright" below).

## Read first, in order

1. `README.md` — what the patch is and how to build it. Start here.
2. `docs/README.md` — evidence/citation protocol, tool inventory, environment traps, glossary.
3. `docs/00-scouting-report.md` — what the engine is; findings F01–F16.
4. `docs/02-formats.md` — verified file formats.
5. `docs/03-plan.md` — options ladder, recommendation, milestones.
6. `docs/04-l2-presenter.md` — the L2 build spec: loader/presenter design, milestones
   L2-M0…M4 with acceptance criteria, escalation ladder, release shape, traps. Read it
   before any `crates/pn_*` work.

## Ground rules

* **Evidence before claims.** Every factual statement cites an evidence id `E00`–`E19`
  (`docs/evidence/`), and every claim is tagged **OBSERVED / INFERRED / UNKNOWN**. Do not promote an
  INFERRED claim to stated fact without new evidence. Do not invent file offsets.
* **AI-generated docs — verify, don't trust.** All `docs/*.md` are AI-generated. Every
  claim in them must be verified before use or repetition: check the cited evidence id
  (`E00`–`E19`) on the doc, open that file in `docs/evidence/`, and confirm the claim
  matches. If a claim has no citation, add the relevant one when the evidence exists, or
  tag the claim UNKNOWN when it does not.
* **Copyright.** This repository ships no game data. Never commit the game's files, its
  dialogue, its error strings, or a decoded corpus. Evidence files record counts, offsets,
  hashes and identifiers only. `tools/find_english.py` and `tools/decode_isf_text.py` print
  text only when you ask for it locally.
* **Regenerate, don't hand-edit evidence.** `bash tools/regen_evidence.sh` (from the repo root)
  rebuilds **all of** `docs/evidence/E00`–`E19` from `original/`. `E00-sha256.txt` pins the inputs.
  If you add a probe, add its line to that script — a reviewer must be able to rebuild every cited
  file with one command.
* **Verified by execution.** Any doc claim about bytes, counts or strings must be reproducible with a
  command in `tools/`; if it is not, mark it UNKNOWN.
* **One runnable check.** `python tools/unpack.py --selftest` asserts every container invariant
  and the dialogue decoder's code map. Extend it when you add a format reader; never ship a format
  claim without an assertion.
* **Small diffs.** `tools/*.py` are intentionally short single-purpose scripts. Extend an existing
  script before adding a new one; delete exploratory scripts whose findings are now in `docs/`.
* **Small code, full coverage.** Keep tools short and readable, but never shrink a diff by
  dropping a format variant, edge case, or verification. Undecoded bytes and unasserted claims
  are defects, not simplicity.
* **Lint gate.** A commit touching Rust files must pass `cargo fmt --check` and
  `cargo clippy --all-targets` with zero warnings for the affected crate/target first
  (see "Rust" section). Python changes keep the existing bar (`--selftest`, evidence).
* **Ask before changing scope.** Lane changes (see `docs/03-plan.md`) are user decisions.
* **Feature completeness is a product requirement, not a nice-to-have.** The shipped
  presenter must handle the game's features correctly — explicitly the dialogue text box
  and the movie playback path. Whether GDI in place suffices or an escalated sink is needed is
  **decided by testing, not by assumption**; meanwhile no code may hardcode an assumption
  about the movie surface (frame-sink seam, generic source-DC identity). See
  `docs/04-l2-presenter.md` §3 directive and §6 L2-M2.

## Session policy

* **Keep going.** Continue through task boundaries; only stop for a significant
  project decision branch or a blocker that needs human intervention to resolve.
  Reordered steps that converge on the same point are not a decision branch —
  just proceed. Lane changes (`docs/03-plan.md`) still need the user.

## Commands (run from the repo root, Git Bash)

```bash
export PYTHONIOENCODING=utf-8        # the console codepage is cp932 and mangles output
bash tools/regen_evidence.sh         # rebuild docs/evidence/E00..E19 (needs python3 + pefile + strings)
python tools/unpack.py --selftest    # container + decoder + extract assertions -> selftest: PASS
python tools/unpack.py ISF --out extracted        # extract one archive
python tools/decode_isf_text.py      # dialogue statistics (offsets and lengths only)
python tools/decode_isf_text.py --dump extracted/ISF-dialogue.txt   # local corpus; never commit it
python tools/walk_isf.py --scan          # bytecode tiling per file + per gap -> E12
python tools/walk_isf.py --scene A_H11.ISF  # disassemble one scene end to end (U2 acceptance)
python tools/pe_report.py original/NURSEUS.EXE      # PE facts
```

Traps hit already: the console codepage can mangle non-ASCII output (set `PYTHONIOENCODING=utf-8`);
`dumpbin`/`7z` may be absent (use MSYS2 `strings`/`objdump`); PE analysis uses `pefile`;
`unpack.py` writes large files for `GGD` and `VOICE`, so do not point `--out` at a small volume.

Rust (hook DLL and config tool; run from the repo root or the crate dir):

```bash
cargo build --target i686-pc-windows-msvc          # pn_modern.dll (32-bit: the game's bitness)
cargo fmt --check
cargo clippy --all-targets --target i686-pc-windows-msvc    # zero warnings, always
cargo test --target i686-pc-windows-msvc                    # geom + pn_shared INI unit tests
# PN-Patch.exe (crates/pn_config) is x86_64 only, never i686 (UAC virtualization):
cargo clippy --all-targets -p pn_config -p pn_shared        # zero warnings
cargo test -p pn_config                                     # install/uninstall on temp dirs
python tools/release.py     # builds both, tests, asserts PE facts -> target/dist/pn-modern-<v>.zip
# Staged runs use a local, git-ignored staging directory; never run from `original/`.
# See tools/play.py and tools/trace_vm.py --help for the available run recipes.
```
`play.py` reads the layout live (origin, scale, fullscreen letterbox) and needs no env.

`enabled=0` in that INI is the kill switch (engine behaves as if the DLL were not loaded);
`mode=fullscreen` is borderless monitor-sized, `scale=1..4|fit|auto` picks the preset.
`dpi_aware=1` (default) makes the DLL set system DPI awareness at load (A2, session 14).

**Paths.** Tools split into two groups, and the old blanket rule was wrong. `pe_report.py` and
`find_english.py` take **argv paths relative to the cwd**. Everything else resolves `original/`
relative to its own `__file__` and can be run from any directory. Only
`bash tools/regen_evidence.sh` has to be reached by a path that lands you in the repo (it `cd`s
to its own parent).

## Rust (L2 presenter toolchain and lint standard)

* **Toolchain:** Rust stable, targets
  `x86_64-pc-windows-msvc` + `i686-pc-windows-msvc`
  (`rustup target add i686-pc-windows-msvc`). The hook DLL **must** be 32-bit —
  the game is a 32-bit EXE. Components: `clippy`, `rustfmt`
  (`rustup component add clippy rustfmt`). The only C dependency is the MSVC linker
  that ships with VS 2022 Build Tools.
* **No SDL2 dependency.** The presenter draws through GDI and depends on `windows-sys`
  only (`crates/*/Cargo.toml` and `Cargo.lock` are the source of truth). The frame-sink
  seam in `crates/pn_presenter/src/sink.rs` keeps an escalated backend possible, but no
  such backend is built or shipped. Do not add one without a measured reason.
* **Lint standard (copy into every new crate's `Cargo.toml` + `clippy.toml`):**
  ```toml
  [lints.rust]
  unsafe_op_in_unsafe_fn = "warn"
  # missing_safety_doc NOT set: warn-by-default since edition 2024, and the
  # explicit entry errors as unknown-lint on this toolchain
  [lints.clippy]
  all = "warn"
  pedantic = "warn"  # must_use, needless_pass_by_value, missing_panics/errors_doc, casts…
  too_many_lines = "warn"         # function-length cap, see clippy.toml
  cognitive_complexity = "warn"   # default max 25
  unwrap_used = "warn"           # a panic in the hook crashes the game: handle or justify
  expect_used = "warn"
  ```
  ```toml
  # clippy.toml
  too-many-lines-threshold = 80
  cognitive-complexity-threshold = 25
  ```
  `warn`, never `deny`, locally (deny only via the explicit gate command below).
  Nursery group deliberately off (unstable). `missing_docs` deliberately off for
  bins (rely on review, not boilerplate).
* **SOLID mapping (what clippy can't check, review must):** S → the 80-line/25-complexity
  tripwires + one-subsystem-per-module; O/C → behaviour behind traits (e.g. a frame-sink
  trait with GDI and headless impls), not flag arguments; I → small focused traits;
  D → depend on traits, so the presenter is testable without launching the game.
  Handle types are newtypes (`struct Hdc(u32)`), never bare ints — handle mixups are the
  cheapest bug class here.
* **FFI rules (hard):** every `extern "C"` entry point wraps its body in
  `catch_unwind` (a panic across FFI is UB); `unsafe` blocks stay small with a
  `// SAFETY:` comment; no `unwrap`/`expect` on game paths.
* **Seed-code style (brief):** `cargo fmt` clean, always. Names follow convention
  (clippy enforces): `UpperCamelCase` types/traits, `snake_case` fns/vars/modules,
  `SCREAMING_SNAKE_CASE` consts. Errors as values (`Result` + `?`); `unwrap` only
  in tests. One subsystem per module; ≤80-line functions; behaviour behind small
  traits (frame sink, config source); handles as newtypes, never bare `u32`.
  Boring over clever — the hook runs inside someone else's process.
* **Commit gate (hard):** a commit touching `*.rs`, `Cargo.toml/lock` or `clippy.toml`
  must pass `cargo fmt --check` and `cargo clippy --all-targets` with **zero warnings**
  for the affected crate and target (`i686` for the hook DLL) **before** commit.
  The `-D warnings` release-gate variant is for release discipline, not daily work.
* **Commands (run in the crate dir; edition 2024):**
  ```bash
  cargo build --target i686-pc-windows-msvc
  cargo fmt --check
  cargo clippy --all-targets --target i686-pc-windows-msvc
  cargo clippy --all-targets --target i686-pc-windows-msvc -- -D warnings  # release gate only
  ```

## Current state

* Established: stack (F01–F16), archive formats verified, **dialogue text encoding solved**
  (737 989 chars, 20 115 runs, zero unmapped codes — `docs/02-formats.md` §4.1); **GGD pixel
  codec closed (U1)**, `tools/decode_ggd.py` decodes all 1036 entries (E17).
* The shipped patch is complete and user-verified on a real install: windowed and borderless
  fullscreen, scaling, high-DPI, the dialogue box inside the game window, alt-tab.
  `PN-Patch.exe` installs, updates and uninstalls the DLL.
* **U2 static work:** ISA (117 ops, E18) + stub→impl map + operand table for
  ~110 ops (all ground-truth re-checked per impl; `tools/walk_isf.py: EXTRA`).
  `walk_isf.py --scan` (in E12): **19 399/20 115 gaps tile (96.4%)**;
  message-tag tails (1-2B before markers, nonzero ⇒ quoted, 100%) consumed
  as TAG; `--scene` disassembles 332 scenes end to end (0 failures).
* Not established: ~582 operand overruns + 107 ONJS-table overruns (verified
  ops whose operands would extend into genuine text — needs a **runtime fetch
  trace** to distinguish dead code from misidentified ops; static tiling
  deliberately stops rather than consume text), 19 path-dependent ops
  (`GP`/`IF`/`IH`/`CALC`/`HP` + `KIDSET` conditional), 8 singleton ops,
  the text-display op (A0_0 has zero `0x2B`), `NURSEUS.SAV` layout (U3,
  optional), window model / text-box question (U4), registration check (U5),
  `GAD ` header fields (U6), `.txt → .ISF` CLI (U7). Nothing is executed from `original/`
  except staged runs.
* Next step: runtime fetch trace of the VM loop to resolve the overruns. See
  `docs/03-plan.md` §2–3.
