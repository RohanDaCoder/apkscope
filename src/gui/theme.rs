//! Colour themes, the palette drawn from them, and the saved settings.

use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Theme {
    Dark,
    Midnight,
    Light,
    Solarized,
}

impl Theme {
    pub(super) const ALL: [Theme; 4] =
        [Theme::Dark, Theme::Midnight, Theme::Light, Theme::Solarized];

    pub(super) fn name(self) -> &'static str {
        match self {
            Theme::Dark => "Dark",
            Theme::Midnight => "Midnight",
            Theme::Light => "Light",
            Theme::Solarized => "Solarized light",
        }
    }

    pub(super) fn dark(self) -> bool {
        matches!(self, Theme::Dark | Theme::Midnight)
    }

    /// (background, panel, accent, text) and code colours
    pub(super) fn palette(self) -> Palette {
        let c = Color32::from_rgb;
        match self {
            Theme::Dark => Palette {
                bg: c(24, 25, 28),
                panel: c(32, 33, 37),
                accent: c(77, 144, 254),
                text: c(220, 222, 226),
                dim: c(140, 144, 150),
                code: [
                    c(212, 212, 212),
                    c(106, 153, 85),
                    c(206, 145, 120),
                    c(86, 156, 214),
                    c(181, 206, 168),
                    c(220, 160, 90),
                ],
                mark: c(78, 66, 22),
            },
            Theme::Midnight => Palette {
                bg: c(13, 17, 23),
                panel: c(22, 27, 34),
                accent: c(88, 166, 255),
                text: c(201, 209, 217),
                dim: c(125, 133, 144),
                code: [
                    c(201, 209, 217),
                    c(139, 148, 158),
                    c(165, 214, 255),
                    c(255, 123, 114),
                    c(121, 192, 255),
                    c(255, 166, 87),
                ],
                mark: c(56, 52, 20),
            },
            Theme::Light => Palette {
                bg: c(250, 250, 251),
                panel: c(240, 241, 244),
                accent: c(37, 99, 235),
                text: c(28, 30, 34),
                dim: c(100, 104, 112),
                code: [
                    c(30, 30, 30),
                    c(0, 128, 0),
                    c(163, 21, 21),
                    c(0, 0, 220),
                    c(9, 134, 88),
                    c(170, 90, 0),
                ],
                mark: c(255, 236, 150),
            },
            Theme::Solarized => Palette {
                bg: c(253, 246, 227),
                panel: c(238, 232, 213),
                accent: c(38, 139, 210),
                text: c(88, 110, 117),
                dim: c(147, 161, 161),
                code: [
                    c(88, 110, 117),
                    c(147, 161, 161),
                    c(42, 161, 152),
                    c(133, 153, 0),
                    c(211, 54, 130),
                    c(203, 75, 22),
                ],
                mark: c(240, 220, 160),
            },
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub(super) bg: Color32,
    pub(super) panel: Color32,
    pub(super) accent: Color32,
    pub(super) text: Color32,
    pub(super) dim: Color32,
    /// plain, comment, string, keyword, number, directive/warning
    pub(super) code: [Color32; 6],
    pub(super) mark: Color32,
}

#[derive(Clone)]
pub(super) struct Settings {
    pub(super) theme: Theme,
    pub(super) code_font: f32,
    pub(super) ui_scale: f32,
    pub(super) deobf: bool,
    pub(super) threads: usize,
    pub(super) use_cache: bool,
    pub(super) export_ctx: usize,
    pub(super) export_method: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Theme::Dark,
            code_font: 13.0,
            ui_scale: 1.0,
            deobf: true,
            threads: 0,
            use_cache: true,
            export_ctx: 3,
            export_method: false,
        }
    }
}

impl Settings {
    pub(super) fn file() -> std::path::PathBuf {
        let base = std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        base.join("apkscope").join("settings.txt")
    }

    pub(super) fn load() -> Settings {
        let mut s = Settings::default();
        let Ok(text) = std::fs::read_to_string(Self::file()) else {
            return s;
        };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k.trim() {
                "theme" => {
                    s.theme = Theme::ALL
                        .into_iter()
                        .find(|t| t.name() == v.trim())
                        .unwrap_or(s.theme)
                }
                "code_font" => s.code_font = v.trim().parse().unwrap_or(s.code_font),
                "ui_scale" => s.ui_scale = v.trim().parse().unwrap_or(s.ui_scale),
                "deobf" => s.deobf = v.trim() == "true",
                "threads" => s.threads = v.trim().parse().unwrap_or(0),
                "use_cache" => s.use_cache = v.trim() == "true",
                "export_ctx" => s.export_ctx = v.trim().parse().unwrap_or(3),
                "export_method" => s.export_method = v.trim() == "true",
                _ => {}
            }
        }
        s
    }

    pub(super) fn save(&self) {
        let p = Self::file();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let text = format!(
            "theme={}\ncode_font={}\nui_scale={}\ndeobf={}\nthreads={}\nuse_cache={}\nexport_ctx={}\nexport_method={}\n",
            self.theme.name(),
            self.code_font,
            self.ui_scale,
            self.deobf,
            self.threads,
            self.use_cache,
            self.export_ctx,
            self.export_method
        );
        let _ = std::fs::write(p, text);
    }
}

pub(super) fn mb(b: u64) -> String {
    match b {
        0..1_048_576 => format!("{} KB", b.div_ceil(1024)),
        _ => format!("{:.1} MB", b as f64 / 1048576.0),
    }
}
