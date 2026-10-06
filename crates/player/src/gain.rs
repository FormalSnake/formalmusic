//! Gain math shared by the decode thread and the audio callback.

/// Quietest normalisation gain, -20 dB. A track tagged far louder than the
/// reference is more likely a bad tag than a track that needs that much cut.
const MIN_NORMALISATION_GAIN: f32 = 0.1;

/// Gain that brings a track to YouTube's reference loudness, from the
/// `loudnessDb` of its player response. Like the web player this only
/// attenuates: quiet tracks are not boosted, since boosting would clip
/// without a limiter.
pub fn normalisation_gain(loudness_db: f32) -> f32 {
    if !loudness_db.is_finite() {
        return 1.0;
    }
    10f32
        .powf(-loudness_db / 20.0)
        .clamp(MIN_NORMALISATION_GAIN, 1.0)
}

/// Equal-power crossfade weights `(outgoing, incoming)` at `progress` in
/// `0.0..=1.0`, so the summed power stays constant through the fade.
pub fn crossfade_weights(progress: f32) -> (f32, f32) {
    let angle = progress.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    (angle.cos(), angle.sin())
}

/// A per-frame linear ramp toward a target gain, so volume, mute, pause and
/// normalisation changes never click.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ramp {
    current: f32,
    target: f32,
    step: f32,
}

impl Ramp {
    /// A ramp that crosses the full 0..1 range in `frames` frames.
    pub fn new(initial: f32, frames: u32) -> Self {
        Self {
            current: initial,
            target: initial,
            step: 1.0 / frames.max(1) as f32,
        }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    pub fn current(&self) -> f32 {
        self.current
    }

    pub fn is_settled(&self) -> bool {
        self.current == self.target
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        if self.current < self.target {
            self.current = (self.current + self.step).min(self.target);
        } else if self.current > self.target {
            self.current = (self.current - self.step).max(self.target);
        }
        self.current
    }

    /// Multiplies interleaved `samples` by the ramp, one step per frame.
    pub fn apply(&mut self, samples: &mut [f32], channels: usize) {
        if self.is_settled() {
            let gain = self.current;
            if gain != 1.0 {
                samples.iter_mut().for_each(|s| *s *= gain);
            }
            return;
        }
        for frame in samples.chunks_exact_mut(channels) {
            let gain = self.next();
            frame.iter_mut().for_each(|s| *s *= gain);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudness_maps_to_attenuation() {
        assert!((normalisation_gain(6.0) - 0.501).abs() < 1e-3);
        assert!((normalisation_gain(0.0) - 1.0).abs() < 1e-6);
        assert_eq!(
            normalisation_gain(-4.0),
            1.0,
            "quiet tracks are not boosted"
        );
        assert_eq!(normalisation_gain(40.0), MIN_NORMALISATION_GAIN);
        assert_eq!(normalisation_gain(f32::NAN), 1.0);
    }

    #[test]
    fn crossfade_keeps_power() {
        for i in 0..=10 {
            let (out, inc) = crossfade_weights(i as f32 / 10.0);
            assert!((out * out + inc * inc - 1.0).abs() < 1e-5);
        }
        assert_eq!(crossfade_weights(0.0), (1.0, 0.0));
        let (out, inc) = crossfade_weights(1.0);
        assert!(out.abs() < 1e-6 && (inc - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ramp_reaches_target_without_overshoot() {
        let mut ramp = Ramp::new(0.0, 4);
        ramp.set_target(1.0);
        let steps: Vec<f32> = (0..6).map(|_| ramp.next()).collect();
        assert_eq!(steps, [0.25, 0.5, 0.75, 1.0, 1.0, 1.0]);
        ramp.set_target(0.6);
        assert_eq!(ramp.next(), 0.75);
        assert_eq!(ramp.next(), 0.6);
        assert!(ramp.is_settled());
    }

    #[test]
    fn ramp_applies_per_frame() {
        let mut ramp = Ramp::new(0.0, 2);
        ramp.set_target(1.0);
        let mut samples = [1.0; 6];
        ramp.apply(&mut samples, 2);
        assert_eq!(samples, [0.5, 0.5, 1.0, 1.0, 1.0, 1.0]);
    }
}
