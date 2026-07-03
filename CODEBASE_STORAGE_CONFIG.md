# Codebase: Storage and Config

## Storage Paths

**`maki-storage/src/paths.rs`**

Directory resolution via `etcetera` crate with fallback:

| Function | Default | Fallback |
|---|---|---|
| `config_dir()` | `~/.config/maki` | `~/.maki` |
| `data_dir()` | `~/.local/share/maki` | `~/.maki` |
| `state_dir()` | `~/.local/state/maki` | `~/.maki` |
| `logs_dir()` | `~/.local/logs/maki` | `~/.maki` |
| `cache_dir()` | `~/.cache/maki` | `~/.maki` |

Fallback triggers when `~/.maki` exists as a directory (backward compatibility).

**Path utilities:**
- `normalize_path()` — lexical resolve of `..`/`.` without FS access
- `canonicalize_clean()` — symlink-aware but strips Windows `\?\` prefix
- `incremental_canonicalize()` — security-safe