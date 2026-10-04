// SPDX-License-Identifier: MPL-2.0
//! Locale selection and presentation strings; protocol identifiers remain unchanged.
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "zh-CN", alias = "zh")]
    Chinese,
}
impl Language {
    pub fn resolve(self) -> Self {
        if self != Self::Auto {
            return self;
        }
        #[cfg(windows)]
        {
            if unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff == 4 {
                Self::Chinese
            } else {
                Self::English
            }
        }
        #[cfg(not(windows))]
        {
            Self::from_locale(
                &["LC_ALL", "LC_MESSAGES", "LANG"]
                    .iter()
                    .filter_map(|key| std::env::var(key).ok())
                    .find(|value| !value.is_empty())
                    .unwrap_or_default(),
            )
        }
    }
    pub fn from_locale(locale: &str) -> Self {
        if locale.to_ascii_lowercase().starts_with("zh") {
            Self::Chinese
        } else {
            Self::English
        }
    }
    pub fn text(self, text: &str) -> Option<&'static str> {
        if self.resolve() != Self::Chinese {
            return None;
        }
        Some(match text {
            "Unsupported settings schema version" => "不支持此设置格式版本",
            "Receive" => "接收",
            "Settings" => "设置",
            "Ready" => "就绪",
            "Audio" => "音频",
            "Connected" => "已连接",
            "Hide" => "隐藏",
            "Quit" => "退出",
            "Dismiss" => "关闭提示",
            "Playback details" => "播放诊断",
            "Close" => "关闭",
            "Video" => "视频",
            "Timing" => "时序",
            "Details" => "诊断",
            "Hide details" => "关闭诊断",
            "Disconnect" => "断开连接",
            "Receiver settings" => "接收设置",
            "Receiver" => "接收器",
            "Set how this receiver appears and what your device sends." => {
                "设置接收器名称与发送设备的视频规格。"
            }
            "Receiver name" => "接收器名称",
            "Width (px)" => "宽度（像素）",
            "Height (px)" => "高度（像素）",
            "Frame rate (fps)" => "帧率（fps）",
            "Use 0 for the device's default size. Receiver changes apply on the next connection." => {
                "接收器设置将在下次连接时生效。"
            }
            "Playback" => "播放",
            "Keep decoding fast and the picture in sync with your display." => {
                "设置视频解码与显示同步。"
            }
            "Use hardware decoding" => "使用硬件解码",
            "Allow HEVC video" => "允许 HEVC 视频",
            "Allow HLS playback" => "允许 HLS 播放",
            "Synchronize with display (VSync)" => "与显示器同步（垂直同步）",
            "Rendering GPU" => "渲染显卡",
            "balanced" => "均衡（跟随显示器）",
            "low-power" => "低功耗",
            "high-performance" => "高性能",
            "Balanced follows the display's GPU. GPU and VSync changes require a restart." => {
                "均衡模式优先使用显示器所在的显卡。显卡与垂直同步设置需重启生效。"
            }
            "Audio output" => "音频输出",
            "Choose where you want to hear the stream." => "选择播放声音的设备。",
            "Saved output unavailable" => "已保存的音频设备不可用",
            "Device changes apply and save immediately, without saving other edits." => {
                "音频设备立即切换并保存，不会保存其他未提交的修改。"
            }
            "Ready when audio starts" => "等待音频开始",
            "Follow Windows default output" => "跟随 Windows 默认音频设备",
            "Window and startup" => "窗口与启动",
            "Keep receiving while the player is out of the way." => "窗口隐藏后继续接收。",
            "Close window to tray" => "关闭窗口时隐藏到托盘",
            "Otherwise, closing minimizes to the taskbar. Use Quit to stop receiving." => {
                "不启用时，关闭窗口会最小化到任务栏。点击“退出”才会停止接收。"
            }
            "Minimize to tray" => "最小化到托盘",
            "Start in tray" => "启动时隐藏到托盘",
            "Start with Windows" => "开机启动",
            "Window size and fullscreen preference are remembered automatically." => {
                "自动记住普通窗口大小与全屏状态。"
            }
            "Language" => "界面语言",
            "System language" => "跟随系统",
            "Language changes apply and save immediately." => "语言立即切换并保存。",
            "Saving…" => "正在保存…",
            "Unsaved changes" => "有未保存的修改",
            "All changes saved" => "所有修改已保存",
            "Save settings" => "保存设置",
            "Reset form" => "重置表单",
            "Settings saved" => "设置已保存",
            "Saved; newer edits are still unsaved" => "已保存；后续修改尚未保存",
            "Check the highlighted fields" => "请检查标记的输入项",
            "Enter a name of 1–128 bytes" => "请输入 1–128 字节的名称",
            "Saving audio output preference…" => "正在保存音频设备…",
            "Audio output saved; active output switches automatically" => {
                "音频设备已保存，输出将自动切换"
            }
            "Exit fullscreen" => "退出全屏",
            "Show controls" => "显示界面",
            "Full screen" => "全屏",
            "Fit picture" => "适应画面",
            "Fill picture" => "填满画面",
            "Mute" => "静音",
            "Unmute" => "取消静音",
            "Muted" => "已静音",
            "Video requires a compatible GPU" => "视频播放需要兼容的显卡",
            "Waiting for video. Press Esc or F11 to leave fullscreen." => {
                "等待视频。按 Esc 或 F11 退出全屏。"
            }
            "Waiting for video. Press Ctrl+H to show controls." => "等待视频。按 Ctrl+H 显示界面。",
            "Audio connected" => "音频已连接",
            "Open Screen Mirroring on your iPhone or iPad.\nChoose this receiver from the list." => {
                "在 iPhone 或 iPad 上打开“屏幕镜像”，\n然后选择此接收器。"
            }
            "Connected. Waiting for your device to send video." => "已连接，等待设备发送视频。",
            "Use your Wi-Fi network or a Windows mobile hotspot." => {
                "连接同一局域网，或使用 Windows 移动热点。"
            }
            "Wide-gamut SDR → sRGB" => "广色域 SDR → sRGB",
            "10-bit SDR" => "10 位 SDR",
            "8-bit SDR" => "8 位 SDR",
            "No displayed video" => "暂无视频画面",
            "Waiting for audio" => "等待音频",
            "unavailable" => "不可用",
            "FPS counts new video submissions. Processing excludes sender, network transit and physical screen latency. AV offset is video minus estimated audible audio." => {
                "帧率统计新视频帧的 GPU 提交。处理耗时不包含发送端、网络传输和屏幕显示延迟。音画偏差为视频时间戳减去预估可听音频时间戳。"
            }
            "Show AirDock" => "显示 AirDock",
            "Hide to tray" => "隐藏到托盘",
            "Disconnect device" => "断开设备连接",
            "AirDock — receiver running" => "AirDock — 正在接收",
            "Window mode could not be applied" => "无法应用窗口模式",
            "Video unavailable: no compatible GPU" => "视频不可用：没有兼容的显卡",
            "Receiver changes apply on the next connection." => "接收器设置将在下次连接时生效。",
            _ => return None,
        })
    }
    pub fn translate<'a>(self, text: &'a str) -> Cow<'a, str> {
        if let Some(value) = self.text(text) {
            return Cow::Borrowed(value);
        }
        if self.resolve() != Self::Chinese {
            return Cow::Borrowed(text);
        }
        let prefixes = [
            ("Source:", "来源："),
            ("Codec:", "编码格式："),
            ("Resolution:", "分辨率："),
            ("Decoder:", "解码器："),
            ("Render GPU:", "渲染显卡："),
            ("Colour:", "色彩："),
            ("Decoded:", "已解码："),
            ("Submitted frames:", "已提交帧数："),
            ("Processing P95 / P99:", "处理耗时 P95 / P99："),
            ("Frame interval P95 / P99:", "帧间隔 P95 / P99："),
            ("AV offset:", "音画偏差："),
            ("Pending video frames:", "等待显示的帧数："),
            ("Replaced / late / stale:", "替换 / 迟到 / 过期："),
            ("Total data:", "累计数据："),
            ("Output latency:", "音频输出延迟："),
            ("Underruns:", "缓冲欠载次数："),
            ("Recovered packets:", "已恢复的数据包："),
            ("Packet errors:", "数据包错误："),
            ("Model:", "机型："),
            ("Could not save:", "保存失败："),
            ("Could not change audio output:", "切换音频设备失败："),
            (
                "Could not remember window preferences:",
                "保存窗口偏好失败：",
            ),
            ("Hidden replacements:", "隐藏期间跳过："),
        ];
        let translated = text
            .lines()
            .map(|line| {
                if let Some((prefix, target)) =
                    prefixes.iter().find(|(prefix, _)| line.starts_with(prefix))
                {
                    format!("{}{}", target, &line[prefix.len()..])
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        Cow::Owned(
            translated
                .replace("Processing ", "处理 ")
                .replace(". Stage:", "。窗口：")
                .replace(". Version ", "。版本 ")
                .replace("Skipped ", "跳过 ")
                .replace(" (estimate)", "（估计）")
                .replace("unavailable", "不可用")
                .replace(" channels", " 声道")
                .replace("8-bit SDR", "8 位 SDR")
                .replace("10-bit SDR", "10 位 SDR")
                .replace("Wide-gamut SDR → sRGB", "广色域 SDR → sRGB")
                .replace("No displayed video", "暂无视频画面")
                .replace("Waiting for audio", "等待音频")
                .replace(
                    "Width: enter a whole number from ",
                    "宽度：请输入以下范围的整数：",
                )
                .replace(
                    "Height: enter a whole number from ",
                    "高度：请输入以下范围的整数：",
                )
                .replace(
                    "Frame rate: enter a whole number from ",
                    "帧率：请输入以下范围的整数：",
                ),
        )
    }
}
impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Auto => "System language",
            Self::English => "English",
            Self::Chinese => "简体中文",
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_and_metadata_are_preserved() {
        assert_eq!(Language::from_locale("zh_CN.UTF-8"), Language::Chinese);
        assert_eq!(Language::from_locale("en_US.UTF-8"), Language::English);
        assert_eq!(
            Language::Chinese.translate("Render GPU: NVIDIA RTX 5070 (Vulkan)"),
            "渲染显卡： NVIDIA RTX 5070 (Vulkan)"
        );
        assert_eq!(Language::English.translate("Settings"), "Settings");
        assert_eq!(
            serde_json::to_string(&Language::Chinese).unwrap(),
            "\"zh-CN\""
        );
    }
}
