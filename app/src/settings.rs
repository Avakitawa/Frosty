use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppSettings {
    pub enabled: bool,
    pub window_snow: bool,
    pub accumulation: bool,
    pub intensity: f32,
    pub falling_speed: f32,
    pub wind_strength: f32,
    pub wind_turbulence: f32,
    pub wind_direction: f32,
    pub weather_mode: u8,
    pub accumulation_speed: f32,
    pub particle_count: u32,
    pub particles_in_windows: bool,
    pub window_wind_draft: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            window_snow: true,
            accumulation: true,
            // The default matches the Calm preset so the highlighted tile and
            // the sliders agree on a fresh install.
            intensity: 0.8,
            falling_speed: 1.0,
            wind_strength: 0.012,
            wind_turbulence: 0.008,
            wind_direction: 0.0,
            weather_mode: 1,
            accumulation_speed: 0.5,
            particle_count: 1_500,
            particles_in_windows: false,
            window_wind_draft: false,
        }
    }
}

impl AppSettings {
    pub fn validate(mut self) -> Self {
        let defaults = Self::default();
        if !self.intensity.is_finite() {
            self.intensity = defaults.intensity;
        }
        if !self.falling_speed.is_finite() {
            self.falling_speed = defaults.falling_speed;
        }
        if !self.wind_strength.is_finite() {
            self.wind_strength = defaults.wind_strength;
        }
        if !self.wind_turbulence.is_finite() {
            self.wind_turbulence = defaults.wind_turbulence;
        }
        if !self.wind_direction.is_finite() {
            self.wind_direction = defaults.wind_direction;
        }
        if !self.accumulation_speed.is_finite() {
            self.accumulation_speed = defaults.accumulation_speed;
        }
        self.intensity = self.intensity.clamp(0.2, 1.0);
        self.falling_speed = self.falling_speed.clamp(0.25, 2.0);
        self.wind_strength = self.wind_strength.clamp(0.0, 0.08);
        self.wind_turbulence = self.wind_turbulence.clamp(0.0, 0.05);
        self.wind_direction = self.wind_direction.rem_euclid(std::f32::consts::TAU);
        self.accumulation_speed = self.accumulation_speed.clamp(0.0, 1.0);
        self.particle_count = self.particle_count.clamp(500, 20_000);
        // 0..=2 are the built-in presets; 3 means the physical sliders were tuned
        // by hand, so no preset tile is highlighted.
        self.weather_mode = self.weather_mode.min(3);
        self
    }

    fn parse(contents: &str) -> Result<Self, String> {
        let mut settings = Self::default();
        let mut version = None;
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err("settings file contains an invalid line".to_owned());
            };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "version" => {
                    version = Some(
                        value
                            .parse::<u32>()
                            .map_err(|_| "settings version is invalid".to_owned())?,
                    );
                }
                "enabled" => settings.enabled = parse_bool(value)?,
                "window_snow" => settings.window_snow = parse_bool(value)?,
                "accumulation" => settings.accumulation = parse_bool(value)?,
                "intensity" => settings.intensity = parse_number(key, value)?,
                "falling_speed" => settings.falling_speed = parse_number(key, value)?,
                "wind_strength" => settings.wind_strength = parse_number(key, value)?,
                "wind_turbulence" => settings.wind_turbulence = parse_number(key, value)?,
                "wind_direction" => settings.wind_direction = parse_number(key, value)?,
                "accumulation_speed" => settings.accumulation_speed = parse_number(key, value)?,
                "particle_count" => {
                    settings.particle_count = value
                        .parse::<u32>()
                        .map_err(|_| "settings particle count is invalid".to_owned())?
                }
                "particles_in_windows" => settings.particles_in_windows = parse_bool(value)?,
                "window_wind_draft" => settings.window_wind_draft = parse_bool(value)?,
                "weather_mode" => {
                    settings.weather_mode = value
                        .parse::<u8>()
                        .map_err(|_| "settings weather mode is invalid".to_owned())?
                }
                _ => {}
            }
        }
        match version {
            Some(5) => Ok(settings.validate()),
            Some(4) => Ok(settings.validate()),
            Some(3) => Ok(settings.validate()),
            Some(2) => Ok(settings.validate()),
            Some(1) => Ok(settings.validate()),
            Some(0) => Ok(settings.validate()),
            Some(version) => Err(format!("unsupported settings version {version}")),
            None => Err("settings file has no version".to_owned()),
        }
    }

    fn encode(self) -> String {
        format!(
            "version=5\nenabled={}\nwindow_snow={}\naccumulation={}\nintensity={}\nfalling_speed={}\nwind_strength={}\nwind_turbulence={}\nwind_direction={}\nweather_mode={}\naccumulation_speed={}\nparticle_count={}\nparticles_in_windows={}\nwindow_wind_draft={}\n",
            self.enabled,
            self.window_snow,
            self.accumulation,
            self.intensity,
            self.falling_speed,
            self.wind_strength,
            self.wind_turbulence,
            self.wind_direction,
            self.weather_mode,
            self.accumulation_speed,
            self.particle_count,
            self.particles_in_windows,
            self.window_wind_draft,
        )
    }
}

pub fn load() -> Result<Option<AppSettings>, String> {
    let path = settings_path()?;
    match fs::read_to_string(path) {
        Ok(contents) => AppSettings::parse(&contents).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Keep the previous brand's settings on first launch after renaming.
            let legacy = legacy_settings_path()?;
            match fs::read_to_string(legacy) {
                Ok(contents) => AppSettings::parse(&contents).map(Some),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(format!("cannot read settings: {error}")),
            }
        }
        Err(error) => Err(format!("cannot read settings: {error}")),
    }
}

pub fn save(settings: AppSettings) -> Result<(), String> {
    let path = settings_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| "settings directory path is invalid".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create settings directory: {error}"))?;

    let temporary_path = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = File::create(&temporary_path)
        .map_err(|error| format!("cannot create temporary settings file: {error}"))?;
    file.write_all(settings.validate().encode().as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write settings: {error}"))?;
    fs::rename(&temporary_path, &path)
        .map_err(|error| format!("cannot replace settings file {}: {error}", path.display()))
}

fn settings_path_for(app_name: &str) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_owned())?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join(app_name)
        .join("settings.conf"))
}

fn settings_path() -> Result<PathBuf, String> {
    settings_path_for("Frosty")
}

fn legacy_settings_path() -> Result<PathBuf, String> {
    settings_path_for("Winter")
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("settings boolean value is invalid".to_owned()),
    }
}

fn parse_number(key: &str, value: &str) -> Result<f32, String> {
    value
        .parse::<f32>()
        .ok()
        .filter(|number| number.is_finite())
        .ok_or_else(|| format!("settings value for {key} is invalid"))
}

#[cfg(test)]
mod tests {
    use super::AppSettings;

    #[test]
    fn settings_round_trip_and_keep_valid_ranges() {
        let settings = AppSettings {
            intensity: 1.4,
            falling_speed: 0.5,
            wind_strength: 0.04,
            accumulation_speed: 0.0,
            particle_count: 30_000,
            particles_in_windows: true,
            ..AppSettings::default()
        };
        assert_eq!(
            AppSettings::parse(&settings.encode()).unwrap(),
            settings.validate()
        );
    }

    #[test]
    fn accumulation_speed_accepts_zero_and_clamps_to_supported_range() {
        let disabled = AppSettings {
            accumulation_speed: 0.0,
            ..AppSettings::default()
        }
        .validate();
        assert_eq!(disabled.accumulation_speed, 0.0);

        let too_fast = AppSettings {
            accumulation_speed: 1.5,
            ..AppSettings::default()
        }
        .validate();
        assert_eq!(too_fast.accumulation_speed, 1.0);

        let too_slow = AppSettings {
            accumulation_speed: -0.5,
            ..AppSettings::default()
        }
        .validate();
        assert_eq!(too_slow.accumulation_speed, 0.0);
    }

    #[test]
    fn version_zero_settings_migrate_using_defaults() {
        let settings = AppSettings::parse("version=0\nenabled=false\n").unwrap();
        assert!(!settings.enabled);
        assert!(settings.window_snow);
        assert!(settings.accumulation);
        assert_eq!(settings.particle_count, 1_500);
        assert_eq!(settings.accumulation_speed, 0.5);
    }

    #[test]
    fn version_one_settings_migrate_and_new_settings_round_trip() {
        let migrated =
            AppSettings::parse("version=1\nenabled=false\nwind_strength=0.04\n").unwrap();
        assert!(!migrated.enabled);
        assert_eq!(migrated.weather_mode, 1);
        assert_eq!(migrated.wind_direction, 0.0);
        assert_eq!(migrated.accumulation_speed, 0.5);

        let settings = AppSettings {
            weather_mode: 2,
            wind_direction: 1.5,
            ..AppSettings::default()
        };
        assert_eq!(
            AppSettings::parse(&settings.encode()).unwrap(),
            settings.validate()
        );
    }

    #[test]
    fn version_two_settings_migrate_to_new_control_defaults() {
        let migrated = AppSettings::parse(
            "version=2\nenabled=true\nwindow_snow=true\naccumulation=true\nintensity=0.8\n",
        )
        .unwrap();
        assert_eq!(migrated.accumulation_speed, 0.5);
        assert_eq!(migrated.particle_count, 1_500);
        assert!(!migrated.particles_in_windows);
    }

    #[test]
    fn version_three_settings_migrate_after_menu_bar_particles_become_unconditional() {
        let migrated =
            AppSettings::parse("version=3\nmenu_bar_particles=false\nparticles_in_windows=true\n")
                .unwrap();
        assert!(migrated.particles_in_windows);
        assert!(migrated.encode().starts_with("version=5\n"));
        assert!(!migrated.encode().contains("menu_bar_particles"));
    }

    #[test]
    fn wind_turbulence_defaults_and_round_trips() {
        assert_eq!(AppSettings::default().wind_turbulence, 0.008);
        let tuned = AppSettings {
            wind_turbulence: 0.032,
            ..AppSettings::default()
        };
        assert_eq!(AppSettings::parse(&tuned.encode()).unwrap(), tuned);
        assert_eq!(
            AppSettings {
                wind_turbulence: 0.2,
                ..AppSettings::default()
            }
            .validate()
            .wind_turbulence,
            0.05
        );
    }

    #[test]
    fn custom_weather_mode_is_kept_after_manual_tuning() {
        let custom = AppSettings {
            weather_mode: 9,
            ..AppSettings::default()
        }
        .validate();
        assert_eq!(custom.weather_mode, 3);
    }

    #[test]
    fn unknown_versions_and_corrupt_values_are_rejected() {
        assert!(AppSettings::parse("version=99\n").is_err());
        assert!(AppSettings::parse("version=1\nwind_strength=NaN\n").is_err());
    }

    #[test]
    fn window_wind_draft_setting_defaults_off_and_round_trips() {
        assert!(!AppSettings::default().window_wind_draft);
        let enabled = AppSettings {
            window_wind_draft: true,
            ..AppSettings::default()
        };
        assert_eq!(AppSettings::parse(&enabled.encode()).unwrap(), enabled);
        assert!(!AppSettings::parse("version=4\n").unwrap().window_wind_draft);
    }

    #[test]
    fn blank_lines_comments_and_spaces_do_not_invalidate_a_settings_file() {
        let settings =
            AppSettings::parse("# Winter\nversion=5\n\nenabled = false\n  particle_count=4000  \n")
                .unwrap();
        assert!(!settings.enabled);
        assert_eq!(settings.particle_count, 4_000);
        assert!(AppSettings::parse("version=5\nnot a setting\n").is_err());
    }
}
