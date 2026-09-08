# lvr — Agent Conventions & Established Rules

## Commits
- Make **frequent, atomic commits** after every meaningful change.
- Commit message format: `<type>: <short description>` (e.g. `fix:`, `refactor:`, `cleanup:`, `build:`, `ui:`).

## Code Cleanliness — Hard Rules

### No Legacy / Compatibility Code
The build script (`build.rs`) enforces this at **compile time**.  
Any line in `src/` containing the following keywords (case-insensitive) **fails the build**:

> `legacy`, `compatibility`, `compat`, `backward`, `migration`, `migrate`, `migrat`, `deprecated`

**Allowlisted third-party names** (Steam paths/VDF keys we don't control):
- `compatdata`
- `CompatToolMapping`
- `compat_tool`

**Policy**: Remove dead code entirely. Do not comment it out.

### File Length Limit
Also enforced in `build.rs` at compile time: **no `.rs` file in `src/` may exceed 1000 lines**.

---

## Domain / Blocklist Category Naming Convention

**Strict canonical names — no aliases, no singular forms:**

| Canonical name | Maps from (VRChat API / other sources) |
|----------------|----------------------------------------|
| `Videos`       | `Video`, `urlList`                     |
| `Images`       | `Image`, `imageHostUrlList`            |
| `Strings`      | `String`, `stringHostUrlList`          |
| `Shared`       | `Rest`, cross-category duplicates      |
| `<verbatim>`   | Any other name (e.g. `Analytics`)      |

**Where the convention is enforced:**
- `assets/lists/domains.json` — source file always has canonical keys.
- `vrchat_config.rs` — `parse_vrchat_config_to_categories()` emits canonical keys.
- `from_name()` in `domain_block/mod.rs` — exact `match` only, no aliases or `eq_ignore_ascii_case` fallbacks.

**Do not** add runtime normalization/aliasing in load or sync code. If a data source uses non-canonical names, fix the source data.

---

## Architecture

### Block State
`BlockState` in `domain_block/mod.rs` is the **sole in-memory source of truth** for what is blocked.  
- No reading from disk files (hosts, rules) to determine state.
- Toggling a category immediately calls `sync_all` which writes `shield_rules.txt` and updates yt-dlp stubs.

### DNS Shield
- `dns_shield.rs` writes `shield_rules.txt` to all three paths simultaneously:
  1. `$XDG_RUNTIME_DIR/lvr/shield_rules.txt` (primary — read by `.so`)
  2. `~/.cache/lvr/shield_rules.txt` (persistent fallback)
  3. `/tmp/lvr_shield_rules.txt` (last-resort fallback)
- `liblvr_dns_shield.so` is compiled from `c_src/dns_shield.c` by `build.rs`.

### Hosts File
**Fully removed.** Do not re-introduce any code that reads or writes the Proton prefix `hosts` file.

### Domain Data Loading Priority
1. Fresh `~/.cache/lvr/domains.json` (< 1 hour old)
2. Remote `assets/lists/domains.json` from GitHub
3. Live VRChat API (`https://api.vrchat.cloud/api/1/config`)
4. Embedded compile-time fallback (`assets/vrchat_config_fallback.json`)

---

## Steam Integration
- All Steam-related code lives in `src/steam/`.
- `localconfig.vdf` parsing is in `src/steam/vdf.rs`.
- DNS Shield install/uninstall controls belong in the **Settings** tab, not the Dashboard.

## UI Conventions
- Dashboard domain category buttons: show **name + count** only, colored red (blocked) / green (unblocked). No `BLOCKED`/`ALLOWED` text on the button itself (tooltip still has it).
- Settings "Domains blocked" table shows both **loaded count** and **actually blocked count** with color coding.

## Deploy
Use `./scripts/build.sh --deploy` (optionally `--no-autostart`).  
The script runs clippy `-D warnings`, the full test suite, then a release build before installing.
