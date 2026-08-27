//! Tempo & first-beat estimation.
//!
//! BPM comes from autocorrelating an onset envelope; the first downbeat from an
//! onset search near the start. Constant-tempo grid *construction* lives in
//! `bvault_core::BeatGrid` — this module only estimates its inputs.
//!
//! The numeric constants here are tuned against golden rekordbox exports (they
//! recover hundredths like 138.97). Treat them as load-bearing: only the
//! octave-fold bounds are parameterised, and their defaults reproduce the
//! original behaviour.

/// BPM octave-folding range. Autocorrelation frequently locks onto the half- or
/// double-tempo lag; the estimate is folded into `[min, max]` so 4-on-the-floor
/// material lands in a canonical DJ range.
#[derive(Debug, Clone)]
pub struct BpmRange {
    pub min: f64,
    pub max: f64,
}

impl Default for BpmRange {
    fn default() -> Self {
        // Techno/house/most 4-on-the-floor material sits ~90-180 BPM.
        Self {
            min: 90.0,
            max: 180.0,
        }
    }
}

/// Estimate BPM from mono samples. Returns 120.0 for empty/degenerate input:
/// a missing tempo is a default grid, not a failure.
pub fn detect_bpm(samples: &[f32], sample_rate: u32, range: &BpmRange) -> f64 {
    if samples.is_empty() {
        return 120.0;
    }

    // First ~30 s is plenty for a stable tempo.
    let n = samples.len().min((sample_rate * 30) as usize);
    let samples = &samples[..n];

    // Onset envelope via RMS at a ~250 Hz hop (4 ms). Finer than 10 ms so the
    // autocorrelation lag -> BPM mapping resolves hundredths after parabolic
    // interpolation, rather than coarse steps.
    let hop = (sample_rate as usize / 250).max(1);
    let mut env: Vec<f32> = samples
        .chunks(hop)
        .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    if env.is_empty() {
        return 120.0;
    }

    // Normalise the envelope.
    let max_env = env.iter().copied().fold(0.0f32, f32::max);
    if max_env > 0.0 {
        for e in &mut env {
            *e /= max_env;
        }
    }

    // Autocorrelate over lags spanning 60-200 BPM; fold into `range` afterwards.
    let env_rate = sample_rate as f64 / hop as f64;
    let min_lag = (env_rate * 60.0 / 200.0) as usize; // 200 BPM
    let max_lag = (env_rate * 60.0 / 60.0) as usize; //  60 BPM
    let hi = max_lag.min(env.len().saturating_sub(1));

    let mut corr = vec![0.0f32; hi + 1];
    let mut best_lag = min_lag;
    let mut best_corr = 0.0f32;
    for lag in min_lag..=hi {
        let count = env.len() - lag;
        let mut c = 0.0f32;
        for i in 0..count {
            c += env[i] * env[i + lag];
        }
        c /= count as f32;
        corr[lag] = c;
        if c > best_corr {
            best_corr = c;
            best_lag = lag;
        }
    }

    // Parabolic interpolation around the integer peak recovers sub-lag precision;
    // integer lags alone quantise BPM coarsely.
    let refined_lag = if best_lag > min_lag && best_lag < hi {
        let (y0, y1, y2) = (corr[best_lag - 1], corr[best_lag], corr[best_lag + 1]);
        let denom = y0 - 2.0 * y1 + y2;
        if denom.abs() > 1e-9 {
            best_lag as f64 + (0.5 * (y0 - y2) / denom) as f64
        } else {
            best_lag as f64
        }
    } else {
        best_lag as f64
    };

    let mut bpm = env_rate * 60.0 / refined_lag;
    while bpm < range.min {
        bpm *= 2.0;
    }
    while bpm > range.max {
        bpm /= 2.0;
    }
    // No 0.5 quantisation — rekordbox stores hundredths.
    bpm
}

/// Find the first significant onset (the first beat) in milliseconds.
pub fn detect_first_beat(samples: &[f32], sample_rate: u32) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    // Search the first few seconds with 5 ms hops.
    let search = samples.len().min((sample_rate * 5) as usize);
    let hop = (sample_rate as usize / 200).max(1);

    let mut onset = Vec::new();
    let mut prev_energy = 0.0f32;
    for chunk in samples[..search].chunks(hop) {
        let energy: f32 = chunk.iter().map(|s| s * s).sum();
        onset.push((energy - prev_energy).max(0.0));
        prev_energy = energy;
    }
    if onset.is_empty() {
        return 0.0;
    }

    let threshold = onset.iter().copied().fold(0.0f32, f32::max) * 0.3;
    for (i, &strength) in onset.iter().enumerate() {
        if strength > threshold {
            return (i * hop) as f64 / sample_rate as f64 * 1000.0;
        }
    }
    0.0
}
