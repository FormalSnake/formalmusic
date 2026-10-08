//! The graphic equalizer the audio callback runs: one biquad per band and
//! channel, from the RBJ Audio EQ Cookbook. Setting gains computes new
//! coefficients and keeps the filter state, so a change never allocates and
//! does not click.

use formalmusic_api::EQ_BANDS_HZ;

const BANDS: usize = EQ_BANDS_HZ.len();
/// An octave per band.
const PEAK_Q: f64 = std::f64::consts::SQRT_2;
/// Filter state below this is flushed to zero, so a decaying tail never
/// reaches denormals, which are slow on x86.
const DENORMAL: f64 = 1e-25;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Coefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coefficients {
    const UNITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn band(index: usize, gain_db: f32, rate: u32) -> Self {
        let freq = EQ_BANDS_HZ[index] as f64;
        // A band at or past Nyquist cannot be drawn at this rate.
        if gain_db == 0.0 || freq >= rate as f64 * 0.45 {
            return Self::UNITY;
        }
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let w0 = std::f64::consts::TAU * freq / rate as f64;
        let (sin, cos) = w0.sin_cos();
        let (b0, b1, b2, a0, a1, a2) = if index == 0 || index == BANDS - 1 {
            // Shelf slope 1: as steep as a shelf gets without a bump.
            let alpha = sin / 2.0 * std::f64::consts::SQRT_2;
            let k = 2.0 * a.sqrt() * alpha;
            if index == 0 {
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + k),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - k),
                    (a + 1.0) + (a - 1.0) * cos + k,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - k,
                )
            } else {
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + k),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - k),
                    (a + 1.0) - (a - 1.0) * cos + k,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - k,
                )
            }
        } else {
            let alpha = sin / (2.0 * PEAK_Q);
            (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            )
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }
}

pub(crate) struct Equalizer {
    rate: u32,
    channels: usize,
    coefficients: [Coefficients; BANDS],
    /// Transposed direct form II state, `[z1, z2]` per band, channel-major.
    state: Vec<[[f64; 2]; BANDS]>,
    active: bool,
    /// Linear gain that keeps the loudest boost from clipping.
    preamp: f32,
}

impl Equalizer {
    pub fn new(rate: u32, channels: usize) -> Self {
        Self {
            rate,
            channels,
            coefficients: [Coefficients::UNITY; BANDS],
            state: vec![[[0.0; 2]; BANDS]; channels],
            active: false,
            preamp: 1.0,
        }
    }

    /// `None` bypasses the filters. Never allocates, so the callback can
    /// call it.
    pub fn set(&mut self, gains: Option<[f32; BANDS]>) {
        let Some(gains) = gains else {
            self.active = false;
            self.preamp = 1.0;
            return;
        };
        if !self.active {
            self.state.iter_mut().for_each(|c| *c = [[0.0; 2]; BANDS]);
        }
        for (index, &db) in gains.iter().enumerate() {
            self.coefficients[index] = Coefficients::band(index, db, self.rate);
        }
        let boost = gains.iter().copied().fold(0f32, f32::max);
        self.preamp = 10f32.powf(-boost / 20.0);
        self.active = true;
    }

    pub fn preamp(&self) -> f32 {
        self.preamp
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Filters interleaved `samples` in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        if !self.active {
            return;
        }
        for frame in samples.chunks_exact_mut(self.channels) {
            for (sample, state) in frame.iter_mut().zip(self.state.iter_mut()) {
                let mut x = *sample as f64;
                for (c, z) in self.coefficients.iter().zip(state.iter_mut()) {
                    let y = c.b0 * x + z[0];
                    z[0] = c.b1 * x - c.a1 * y + z[1];
                    z[1] = c.b2 * x - c.a2 * y;
                    x = y;
                }
                *sample = x as f32;
            }
        }
        for z in self.state.iter_mut().flatten().flatten() {
            if z.abs() < DENORMAL {
                *z = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steady-state amplitude of a sine at `freq` through `eq`, one channel.
    fn response(eq: &mut Equalizer, freq: f32) -> f32 {
        let rate = eq.rate as f32;
        let mut samples: Vec<f32> = (0..rate as usize)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / rate).sin())
            .collect();
        eq.process(&mut samples);
        samples[samples.len() / 2..]
            .iter()
            .fold(0f32, |peak, s| peak.max(s.abs()))
    }

    fn db(amplitude: f32) -> f32 {
        20.0 * amplitude.log10()
    }

    #[test]
    fn bands_boost_where_they_sit() {
        let mut eq = Equalizer::new(48_000, 1);
        let mut gains = [0.0; BANDS];
        gains[5] = 6.0;
        eq.set(Some(gains));
        assert!((db(response(&mut eq, 1000.0)) - 6.0).abs() < 0.2);
        assert!(db(response(&mut eq, 100.0)).abs() < 0.5);
        assert!((eq.preamp() - 0.501).abs() < 1e-3);
    }

    #[test]
    fn low_shelf_lifts_the_bass() {
        let mut eq = Equalizer::new(44_100, 1);
        let mut gains = [0.0; BANDS];
        gains[0] = 8.0;
        eq.set(Some(gains));
        assert!((db(response(&mut eq, 15.0)) - 8.0).abs() < 0.5);
        assert!(db(response(&mut eq, 2000.0)).abs() < 0.2);
    }

    #[test]
    fn bypassed_and_past_nyquist_are_untouched() {
        let mut eq = Equalizer::new(22_050, 2);
        let mut samples = [0.25, -0.5, 0.75, 1.0];
        eq.process(&mut samples);
        assert_eq!(samples, [0.25, -0.5, 0.75, 1.0]);
        let mut gains = [0.0; BANDS];
        gains[BANDS - 1] = 6.0;
        eq.set(Some(gains));
        assert_eq!(eq.coefficients[BANDS - 1], Coefficients::UNITY);
        eq.set(None);
        assert!(!eq.is_active());
        assert_eq!(eq.preamp(), 1.0);
    }
}
