<p align="center"><img src="assets/icon.png" width="112" alt="ed2k-vault icon"></p>

# ed2k-vault

Generate ed2k links for a whole folder tree, **without eMule**. Point it at a folder (local, external drive, NAS/UNC) and get your files back as links named exactly like they are on disk: a tiny personal backup you can reload into eMule after a disaster.

[Version française](README_FR.md)

## Quick start (no programming needed)

1. Open the **Releases** section on the right of the GitHub page and click the latest version.
2. Under **Assets**, download the archive for your system:
   - Windows: `ed2k-vault-vX.Y.Z-x86_64-pc-windows-msvc.zip`
   - Linux: `ed2k-vault-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
   - Mac with an Apple chip (M1 and later): `...aarch64-apple-darwin.tar.gz`
   - Mac with an Intel chip: `...x86_64-apple-darwin.tar.gz`
3. Extract the archive anywhere (nothing to install).
4. Double-click **`ed2k-vault-gui.exe`** (on Linux and Mac, run `ed2k-vault-gui`). Windows may warn that the program is unsigned: click *More info*, then *Run anyway*.
5. Click **Browse...** to pick the folder to scan, **Choose...** to pick where the result is saved, then **Start scan**.
6. To reload your files in eMule, pick the *eMule collection* format and double-click the resulting file. With the *HTML page* format, open the page in a browser and click the links.

The first scan reads every file, so a big disk takes a while. Next scans only read new or modified files.

## Features

- Recursive scan with filters: extensions, name/path globs, excluded folders, min/max size, hidden files.
- ed2k (MD4) and AICH (SHA-1) hashes computed in a **single read** of each file.
- Incremental cache (path + size + modification time): a second pass over 140 000 files is nearly instant, and an interrupted scan resumes where it stopped.
- Streaming output, so huge trees never pile up in memory. Output order is deterministic (sorted walk).
- Formats: plain links, HTML (`<a href>`), URL tags (`[url=ed2k://...]name[/url]`), CSV (`name,size,link`), `.emulecollection`.
- Options like eMule's: complete hashset (`p=`), AICH root (`h=`).
- Parallel hashing: several files at once **and** the parts of a big file in parallel (about 2.5 GiB/s on a fast SSD here).
- Read-only: no admin rights needed. Unreadable files are listed and skipped, symbolic links are not followed, Windows long paths and UNC paths work.
- Command line and graphical interface.

## Usage

```
ed2k-vault scan D:\Share --out links.html --hashset
ed2k-vault scan \\NAS\media --out links.txt --format links --exclude "*.tmp" --threads 8
ed2k-vault scan D:\Share --out backup.emulecollection
ed2k-vault gui
```

The format is guessed from the extension of `--out` (`.html`, `.csv`, `.emulecollection`, anything else is plain links). Without `--out`, links go to the standard output.

| Option | Meaning |
|---|---|
| `--format` | `links`, `html`, `urltag`, `csv`, `collection` |
| `--hashset` | add `p=hash1:hash2:...` (files with several parts) |
| `--no-aich` | skip AICH (faster, no `h=`) |
| `--include-ext` / `--exclude-ext` | comma separated extensions |
| `--exclude` / `--exclude-dir` | glob, repeatable |
| `--min-size` / `--max-size` | `500k`, `10MB`, `1.5G`... |
| `--hidden` | also scan hidden files and folders |
| `--threads` | hashing threads (default: one per core, up to 16) |
| `--jobs` | files read at once (default 2 to 4; keep it low on HDD/NAS) |
| `--cache` / `--no-cache` | cache file (default: user cache folder) / disable |
| `--quiet` | no progress bar nor summary |

Exit codes: `0` success, `1` fatal error, `3` finished but some files could not be read, `130` cancelled with Ctrl-C (the output is complete up to that point; run the same command again to resume).

Zero-byte files are skipped and reported (eMule does not share them).

## Keep the folder structure

A scan is a flat list by default. Three options keep track of where files live (also available as checkboxes in the GUI, under *Folder structure*):

```
ed2k-vault scan D:\Share --out links.html --group-by-folder
ed2k-vault scan D:\Share --out collections --split-collections
ed2k-vault scan D:\Share --out links.txt --tree structure.txt
```

- `--group-by-folder`: the HTML page gets a heading per folder (`--- A --- / Akim`) before its links, and the CSV gets a `folder` column. Plain links and URL tags stay pure lists.
- `--split-collections`: one `.emulecollection` per folder, written into the `--out` folder and named after the folder path (`--- A --- - Akim.emulecollection`, files of the root go in `<root name>.emulecollection`).
- `--tree FILE`: the full folder tree as text, with the number of files and size directly inside each folder. **Empty folders appear here only**, so this is the complete trace of the structure.

In every output, the files of a folder now come together, before its sub-folders (each level sorted by name).

## Restore file names from a saved list

Lost the names of your files (bad copy, renamed in bulk, recovered disk)? Keep the list made by a scan and let ed2k-vault match your files against it. Files are matched on **size and ed2k hash**, never on name, so a file gets its listed name back whatever it is called now.

```
ed2k-vault compare backup.html D:\Share              # report only, nothing is touched
ed2k-vault compare backup.html D:\Share --rename     # really rename
ed2k-vault compare backup.emulecollection D:\Share --missing-out to-download.txt
```

- The list can be any file holding ed2k links (plain, HTML, CSV, URL tags) or a `.emulecollection`.
- Files are renamed in place, in their own folder. Nothing is ever overwritten: if the listed name is already taken, or if two files have the same content, the file is left alone and reported.
- `--missing-out FILE` saves the links of the listed files that are not in the folder, ready to paste into eMule.
- `--report FILE` writes the full report (every rename, skipped, missing and unlisted file).
- Names are cleaned only for characters the file system forbids (for example `:` or `?` on Windows).
- If you press Ctrl-C, nothing is renamed or written, since an unfinished scan would make every unseen file look missing.
- Same filters, `--threads`, `--jobs` and cache options as `scan`. Only files whose content is identical can be matched.

The GUI does not have this feature yet.

## Screenshots

<p align="center">
  <img src="docs/gui-main.png" width="420" alt="Main window">
  <img src="docs/gui-done.png" width="420" alt="After a scan">
</p>

## Portable build (Windows)

```
powershell -ExecutionPolicy Bypass -File scripts/build-portable.ps1
```

Runs a release build and produces `portable/ed2k-vault-<version>-windows-x64/` (CLI, GUI, licence, readmes) plus a `.zip` of it. The `portable/` folder is git-ignored. Nothing needs installing: copy the folder anywhere.

## Build

```
cargo build --release
```

Produces `ed2k-vault` (CLI) and `ed2k-vault-gui` (Slint, Fluent style) in `target/release`. On Linux the GUI needs the usual development packages (fontconfig, xkbcommon, wayland).

Prebuilt archives for Windows, Linux and macOS (Intel and Apple Silicon) are attached to each release. macOS binaries are not signed: on first launch, right-click the binary and choose *Open*, or run `xattr -d com.apple.quarantine <file>`.

## Accuracy notes

Unit tests check the hashing code against independent constructions, and the output was cross-checked against a separate Python implementation. Two points still need a comparison with links generated by eMule itself:

- the behaviour for files whose size is an **exact multiple of 9 728 000 bytes** (an extra MD4 of an empty block is appended to the part list, following the ed2k specification);
- the exact **percent-encoding of file names** (UTF-8 bytes of everything except ASCII letters, digits and `-_.!~*'()`; eMule decodes any valid encoding, so the links stay usable either way).

Put reference files and the link eMule produced in `tests/fixtures/` (see its README): `cargo test -p ed2k-vault-core --test fixtures` compares them field by field.

## License

MIT, see [LICENSE](LICENSE). The graphical interface is built with [Slint](https://slint.dev) under its royalty-free license, which requires this attribution.
