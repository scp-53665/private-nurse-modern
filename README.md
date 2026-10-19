# Private Nurse — Modern Windows Patch

Run **Private Nurse** (AngelSmile, 2002, US release `NURSEUS.EXE`) on a modern Windows
computer. The game runs in a normal window that you can move and resize, or in borderless
fullscreen. The picture is scaled and stays sharp on high-DPI displays. The screen
resolution does not change. This patch is for the JAST English Download Edition and is
untested on other version.

The patch does not modify `NURSEUS.EXE` or the game's data files. It replaces one small DLL
that the game already loads (`fakecdda.dll`, the CD-music shim). The replacement draws the
game picture scaled and sends the music calls to the original shim.

Download the latest release from the Releases page of this repository.

## For players

### What the patch does

- Runs the game in a window. You can move, resize, minimize and restore it.
- Runs the game in borderless fullscreen on the monitor.
- Scales the 640×480 picture by 1, 2, 3 or 4, or to the largest size that fits.
- Draws the dialogue box inside the game window.
- Keeps the picture sharp at 125 % and 150 % display scaling.
- Fixes the black window after Alt-Tab.

### Install

1. Download `pn-modern-<version>.zip` from the Releases page.
2. Unzip `PN-Patch.exe` and `pn_modern.dll` into the Private Nurse folder. This folder
   contains `NURSEUS.EXE`, for example `C:\Program Files (x86)\Angelsmile\Private Nurse`.
3. Run `PN-Patch.exe` and click **Install**. If the game is in `Program Files`, Windows
   asks for administrator permission.
4. Start the game as usual, or click **Play** in PN-Patch.

The patch is not code-signed. Windows SmartScreen can show a warning on the first run,
and some antivirus programs can flag the DLL. The manual install below does the same work
if you do not want to run PN-Patch.

### Manual install

1. Close the game.
2. Rename `fakecdda.dll` to `fakecdda_real.dll`.
3. Copy `pn_modern.dll` into the game folder as `fakecdda.dll`.

### Configuration

Change the configuration in PN-Patch and click **Save**. The new values apply the next
time the game starts.

PN-Patch writes `pn_modern.ini` into the game folder. If the game folder is read-only,
PN-Patch writes `%APPDATA%\PnModern\config.ini` instead. You can also edit the file in a
text editor:

```ini
enabled=1        # 0 = turn the patch off and use the original 640x480 fullscreen mode
mode=windowed    # windowed | fullscreen (borderless)
scale=auto       # auto (largest whole number) | fit (largest that fits) | 1 | 2 | 3 | 4
filter=auto      # auto | nearest (sharp pixels) | smooth
dpi_aware=1      # 1 = sharp on high-DPI displays; 0 = let Windows scale the window
```

If there is no configuration file, these defaults apply. The patch skips a wrong line and
uses the rest of the file.

### Uninstall

1. Run `PN-Patch.exe`.
2. Click **Uninstall**.

PN-Patch restores the original `fakecdda.dll` and deletes the configuration file.

For a manual uninstall, delete `fakecdda.dll` and rename `fakecdda_real.dll` back to
`fakecdda.dll`.

### Notes

- The patch writes a diagnostic log to `%TEMP%\pn_modern\pn_modern.log`. The file is at
  most about 2 MB, and older entries move to `pn_modern.log.1`.
- Tested on Windows 11 x64 with the US release. ARM64 is not tested.
- The patch contains no game data and does not include the stock FakeCDDA files.

## For developers

### Requirements

- Windows 10 or 11, x64. The build is Windows-only because the code uses Win32 APIs and
  an MSVC linker.
- Visual Studio 2022 Build Tools with the C++ workload.
- Rust stable with both MSVC targets:

```bash
rustup target add x86_64-pc-windows-msvc i686-pc-windows-msvc
rustup component add clippy rustfmt
```

- Python 3 with `pefile`, for the release script:

```bash
python -m pip install pefile
```

You do not need the game to build or test the Rust code.

### Build and test

```bash
cargo build --target i686-pc-windows-msvc      # pn_modern.dll (32-bit: the game's bitness)
cargo test  --target i686-pc-windows-msvc
cargo clippy --all-targets --target i686-pc-windows-msvc
cargo test -p pn_config                        # PN-Patch.exe is x86_64 only
cargo clippy --all-targets -p pn_config -p pn_shared
```

Each crate defines these lint levels in its own `Cargo.toml`, with the thresholds in
`clippy.toml`. A change to Rust code must pass `cargo fmt --check` and `cargo clippy` with
zero warnings.

### Build the release zip

```bash
python tools/release.py
```

This command builds both binaries, runs the tests, checks the PE facts, and writes
`target/dist/pn-modern-<version>.zip`. The zip contains `pn_modern.dll`, `PN-Patch.exe`,
`README.md` and `LICENSE-MIT`.

GitHub Actions runs these Rust checks on `windows-latest` for every push to `main` and
every pull request. The evidence tools need the game files, so they run locally only.

### Repository layout

| Path | Contents |
|---|---|
| `crates/pn_presenter` | The hook DLL. Proxies `fakecdda.dll`, hooks GDI, scales and presents the frame. |
| `crates/pn_config` | `PN-Patch.exe`, the installer and settings window. |
| `crates/pn_shared` | The configuration model and INI parser, shared by both binaries. |
| `tools/` | Reverse-engineering and evidence scripts. |
| `docs/` | Research notes. You do not need them to use or build the patch. |

### Research notes

The `docs/` folder records how the engine works: the archive formats, the scenario
bytecode, the image codec and the build plan. The structural claims cite an evidence file
in `docs/evidence/`. `AGENTS.md` describes the working rules for the repository.

The evidence tools read the original game files from `original/`. That folder is
git-ignored. Put a copy of your own game directory there and run:

```bash
export PYTHONIOENCODING=utf-8
python tools/unpack.py --selftest
bash tools/regen_evidence.sh
```

`regen_evidence.sh` needs Python 3 with `pefile`, plus `strings` from MSYS2. It rebuilds
all evidence files from `original/`.

### Licence

The code in this repository is available under MIT. See `LICENSE-MIT`.

Private Nurse and its content belong to their rights holders. This project is not
affiliated with AngelSmile or GamelexGS. The repository contains no game data, no game
text and no stock FakeCDDA files. You must own a copy of the game to use the patch.
