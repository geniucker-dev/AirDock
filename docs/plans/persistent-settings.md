# Persistent Settings

## Problem

Every restart resets LiveSettings to hardcoded defaults (2560x1440, software decoder, etc.). Users must reconfigure resolution and decoder mode each launch.

## Design Decisions

| Decision | Choice |
|---|---|
| **Scope** | All 7 LiveSettings fields: mirror_width, mirror_height, hevc_enabled, max_fps, refresh_rate, mirror_hwaccel, vsync_enabled |
| **File location** | `%APPDATA%/AirPlay-Windows/settings.ini` (Win32 `SHGetKnownFolderPath(FOLDERID_RoamingAppData)`) |
| **Format** | INI-style key=value (no library dependency, ~50 lines parser) |
| **Save trigger** | On app exit only (snapshot LiveSettings in shutdown path) |
| **First-run default** | Auto-detect monitor resolution via `SDL_GetCurrentDisplayMode()` when no settings file exists |
| **CLI vs saved** | CLI flags override saved settings; saved settings are the fallback when no CLI flag is given |
| **Cross-platform** | Windows-only (`#if defined(_WIN32)`). Linux builds skip persistence. |
| **Identity key** | Unchanged — stays in working directory |

## File Format

```ini
mirror_width=3440
mirror_height=1440
hevc_enabled=1
max_fps=60
refresh_rate=60
mirror_hwaccel=1
vsync_enabled=1
```

## Implementation

### New file: `src/settings.h` / `src/settings.cpp`

```
namespace ap {

struct PersistentSettings {
    int  mirror_width    = 0;   // 0 = auto-detect
    int  mirror_height   = 0;
    bool hevc_enabled    = true;
    int  max_fps         = 60;
    int  refresh_rate    = 60;
    bool mirror_hwaccel  = false;
    bool vsync_enabled   = true;
};

// Returns %APPDATA%/AirPlay-Windows/settings.ini (creates dir if needed).
std::string settings_path();

// Load from disk. Returns defaults if file doesn't exist or is malformed.
PersistentSettings load_settings();

// Write current values to disk.
void save_settings(const PersistentSettings& s);

// Snapshot LiveSettings into PersistentSettings for saving.
PersistentSettings snapshot(const ap::airplay::LiveSettings& live);

} // namespace ap
```

### Changes to `main.cpp`

Load order:
1. `PersistentSettings saved = load_settings();`
2. If `saved.mirror_width == 0` (no file existed): call `SDL_GetCurrentDisplayMode()` to get native monitor resolution, use that as default.
3. Parse CLI args — any explicit `--mirror-res` or `--mirror-hwaccel` overrides the saved value.
4. Seed `LiveSettings` from the merged result.
5. On shutdown (after the main loop exits, before `renderer.stop()`): `save_settings(snapshot(live_settings));`

### Auto-detect flow (first run)

```
SDL_Init(SDL_INIT_VIDEO);
SDL_DisplayMode dm;
if (SDL_GetCurrentDisplayMode(0, &dm) == 0) {
    mirror_w = dm.w;
    mirror_h = dm.h;
}
```

Note: SDL_Init for video must happen before this query. Currently the renderer calls SDL_Init internally — may need to move SDL_Init(SDL_INIT_VIDEO) earlier in main() or call it twice (SDL_Init is idempotent for the same subsystems).

### Files touched

- **New**: `src/settings.h`, `src/settings.cpp`
- **Modified**: `src/main.cpp` (load/save integration, early SDL_Init for display detection)
- **Modified**: `CMakeLists.txt` (add `src/settings.cpp` to sources)
