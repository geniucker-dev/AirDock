#include "settings.h"
#include "airplay/live_settings.h"
#include "log.h"

#include <fstream>
#include <sstream>
#include <string>

#if defined(_WIN32)
#include <shlobj.h>
#endif

namespace ap {

std::string settings_path() {
#if defined(_WIN32)
    PWSTR wide_path = nullptr;
    if (FAILED(SHGetKnownFolderPath(FOLDERID_RoamingAppData, 0, nullptr, &wide_path))) {
        CoTaskMemFree(wide_path);
        return {};
    }
    int len = WideCharToMultiByte(CP_UTF8, 0, wide_path, -1, nullptr, 0, nullptr, nullptr);
    std::string base(len - 1, '\0');
    WideCharToMultiByte(CP_UTF8, 0, wide_path, -1, base.data(), len, nullptr, nullptr);
    CoTaskMemFree(wide_path);

    std::string dir = base + "\\AirPlay-Windows";
    CreateDirectoryA(dir.c_str(), nullptr);
    return dir + "\\settings.ini";
#else
    return {};
#endif
}

PersistentSettings load_settings() {
    PersistentSettings s;
    const std::string path = settings_path();
    if (path.empty()) return s;

    std::ifstream in(path);
    if (!in.is_open()) return s;

    std::string line;
    while (std::getline(in, line)) {
        if (line.empty() || line[0] == '#') continue;
        auto eq = line.find('=');
        if (eq == std::string::npos) continue;

        std::string key = line.substr(0, eq);
        std::string val = line.substr(eq + 1);

        try {
            if      (key == "mirror_width")   s.mirror_width   = std::stoi(val);
            else if (key == "mirror_height")  s.mirror_height  = std::stoi(val);
            else if (key == "hevc_enabled")   s.hevc_enabled   = std::stoi(val) != 0;
            else if (key == "max_fps")        s.max_fps        = std::stoi(val);
            else if (key == "refresh_rate")   s.refresh_rate   = std::stoi(val);
            else if (key == "mirror_hwaccel") s.mirror_hwaccel = std::stoi(val) != 0;
            else if (key == "vsync_enabled")  s.vsync_enabled  = std::stoi(val) != 0;
            else if (key == "fullscreen")     s.fullscreen     = std::stoi(val) != 0;
            else if (key == "chromeless")     s.chromeless     = std::stoi(val) != 0;
        } catch (...) {
            LOG_WARN << "settings: malformed value for " << key << "=" << val;
        }
    }
    LOG_INFO << "settings: loaded from " << path;
    return s;
}

void save_settings(const PersistentSettings& s) {
    const std::string path = settings_path();
    if (path.empty()) return;

    std::ofstream out(path, std::ios::trunc);
    if (!out.is_open()) {
        LOG_WARN << "settings: could not write " << path;
        return;
    }

    out << "mirror_width="   << s.mirror_width   << '\n'
        << "mirror_height="  << s.mirror_height  << '\n'
        << "hevc_enabled="   << s.hevc_enabled   << '\n'
        << "max_fps="        << s.max_fps        << '\n'
        << "refresh_rate="   << s.refresh_rate   << '\n'
        << "mirror_hwaccel=" << s.mirror_hwaccel << '\n'
        << "vsync_enabled="  << s.vsync_enabled  << '\n'
        << "fullscreen="     << s.fullscreen     << '\n'
        << "chromeless="     << s.chromeless     << '\n';

    LOG_INFO << "settings: saved to " << path;
}

PersistentSettings snapshot(const ap::airplay::LiveSettings& live) {
    PersistentSettings s;
    s.mirror_width   = live.mirror_width.load(std::memory_order_relaxed);
    s.mirror_height  = live.mirror_height.load(std::memory_order_relaxed);
    s.hevc_enabled   = live.hevc_enabled.load(std::memory_order_relaxed);
    s.max_fps        = live.max_fps.load(std::memory_order_relaxed);
    s.refresh_rate   = live.refresh_rate.load(std::memory_order_relaxed);
    s.mirror_hwaccel = live.mirror_hwaccel.load(std::memory_order_relaxed);
    s.vsync_enabled  = live.vsync_enabled.load(std::memory_order_relaxed);
    s.fullscreen     = live.fullscreen.load(std::memory_order_relaxed);
    s.chromeless     = live.chromeless.load(std::memory_order_relaxed);
    return s;
}

} // namespace ap
