//! The equalizer setting, shared by the Settings dialog that writes it and
//! the daemon that plays through it.

use serde::{Deserialize, Serialize};

/// Centre frequencies of the bands, an octave apart. The first band is a
/// low shelf and the last a high shelf, the rest are peaking filters.
pub const EQ_BANDS_HZ: [f32; 10] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

/// The most a band can boost or cut, in dB.
pub const EQ_MAX_DB: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqPreset {
    #[default]
    Off,
    BassBoost,
    BassReducer,
    TrebleBoost,
    Vocal,
    Rock,
    Electronic,
    Acoustic,
    Custom,
}

impl EqPreset {
    pub const ALL: [EqPreset; 9] = [
        EqPreset::Off,
        EqPreset::BassBoost,
        EqPreset::BassReducer,
        EqPreset::TrebleBoost,
        EqPreset::Vocal,
        EqPreset::Rock,
        EqPreset::Electronic,
        EqPreset::Acoustic,
        EqPreset::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EqPreset::Off => "Off",
            EqPreset::BassBoost => "Bass boost",
            EqPreset::BassReducer => "Bass reducer",
            EqPreset::TrebleBoost => "Treble boost",
            EqPreset::Vocal => "Vocal",
            EqPreset::Rock => "Rock",
            EqPreset::Electronic => "Electronic",
            EqPreset::Acoustic => "Acoustic",
            EqPreset::Custom => "Custom",
        }
    }

    /// Band gains in dB; `None` for Custom, whose gains are the user's.
    pub fn gains(self) -> Option<[f32; 10]> {
        Some(match self {
            EqPreset::Off => [0.0; 10],
            EqPreset::BassBoost => [7.0, 6.0, 5.0, 3.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            EqPreset::BassReducer => [-6.0, -5.0, -4.0, -2.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            EqPreset::TrebleBoost => [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0],
            EqPreset::Vocal => [-2.0, -2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 3.0, 1.0, 0.0],
            EqPreset::Rock => [4.0, 3.0, 2.0, 0.0, -1.0, -1.0, 1.0, 2.0, 3.0, 4.0],
            EqPreset::Electronic => [5.0, 4.0, 1.0, 0.0, -2.0, 1.0, 0.0, 1.0, 4.0, 5.0],
            EqPreset::Acoustic => [3.0, 3.0, 2.0, 1.0, 1.0, 1.0, 2.0, 3.0, 2.0, 1.0],
            EqPreset::Custom => return None,
        })
    }
}

/// `config.json`'s `equalizer`: a preset, and the gains Custom plays.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Equalizer {
    pub preset: EqPreset,
    pub custom: [f32; 10],
}

impl Equalizer {
    /// The gains to play, `None` when every band is flat.
    pub fn gains(&self) -> Option<[f32; 10]> {
        let gains = self.preset.gains().unwrap_or(self.custom).map(|db| {
            if db.is_finite() {
                db.clamp(-EQ_MAX_DB, EQ_MAX_DB)
            } else {
                0.0
            }
        });
        gains.iter().any(|&db| db != 0.0).then_some(gains)
    }

    /// The gains shown on the sliders.
    pub fn shown(&self) -> [f32; 10] {
        self.preset.gains().unwrap_or(self.custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_custom() {
        let eq: Equalizer = serde_json::from_str(r#"{"preset":"bass_boost"}"#).unwrap();
        assert_eq!(eq.gains().unwrap()[0], 7.0);
        assert_eq!(Equalizer::default().gains(), None);
        let eq: Equalizer =
            serde_json::from_str(r#"{"preset":"custom","custom":[30,0,0,0,0,0,0,0,0,-1]}"#)
                .unwrap();
        let gains = eq.gains().unwrap();
        assert_eq!((gains[0], gains[9]), (EQ_MAX_DB, -1.0));
        let flat = Equalizer {
            preset: EqPreset::Custom,
            custom: [0.0; 10],
        };
        assert_eq!(flat.gains(), None);
    }
}
