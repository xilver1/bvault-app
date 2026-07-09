//! Waveform generation for Pioneer displays
//!
//! Generates both preview (PWAV) and detail (PWV5) waveforms using FFT
//! for frequency band separation (bass/mid/high → red/green/blue).

use rustfft::{FftPlanner, num_complex::Complex};
use rekordbox_core::{Waveform, WaveformPreview, WaveformDetail, WaveformColumn, WaveformColorEntry,
                     WaveformColorPreview, WaveformColorPreviewColumn};

/// Waveform generator with FFT support
pub struct WaveformGenerator {
    sample_rate: u32,
}

impl WaveformGenerator {
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }
    
    /// Generate all waveform types (preview, color preview, and detail)
    pub fn generate(&self, samples: &[f32], duration_secs: f64) -> Waveform {
        let preview = self.generate_preview(samples);
        let color_preview = self.generate_color_preview(samples);
        let detail = self.generate_detail(samples, duration_secs);
        
        Waveform { preview, color_preview, detail }
    }

    /// Generate 1200-column color preview waveform (PWV4 format)
    fn generate_color_preview(&self, samples: &[f32]) -> WaveformColorPreview {
        let mut columns = Vec::with_capacity(1200);

        if samples.is_empty() {
            return WaveformColorPreview {
                columns: vec![WaveformColorPreviewColumn::default(); 1200],
            };
        }

        // FFT setup for frequency analysis
        let fft_size = 1024;
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);

        // Hann window
        let window: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / fft_size as f32).cos()))
            .collect();

        // Frequency bin ranges
        let bin_hz = self.sample_rate as f32 / fft_size as f32;
        let bass_start = (20.0 / bin_hz).ceil() as usize;
        let bass_end = (200.0 / bin_hz) as usize;
        let mid_end = (4000.0 / bin_hz) as usize;
        let high_end = std::cmp::min((20000.0 / bin_hz) as usize, fft_size / 2);

        // Divide samples into 1200 segments
        let segment_size = samples.len() / 1200;
        if segment_size == 0 {
            return WaveformColorPreview {
                columns: vec![WaveformColorPreviewColumn::default(); 1200],
            };
        }

        for i in 0..1200 {
            let start = i * segment_size;
            let end = std::cmp::min(start + segment_size, samples.len());
            
            if start >= samples.len() {
                columns.push(WaveformColorPreviewColumn::default());
                continue;
            }

            // Get FFT window centered on this segment
            let mut fft_buffer: Vec<Complex<f32>> = (0..fft_size)
                .map(|j| {
                    let sample_idx = start + j;
                    let sample = if sample_idx < samples.len() {
                        samples[sample_idx]
                    } else {
                        0.0
                    };
                    Complex::new(sample * window[j], 0.0)
                })
                .collect();

            fft.process(&mut fft_buffer);

            // Calculate energy for each band
            let bass_range = bass_start.max(1)..=bass_end.min(fft_size / 2);
            let mid_range = (bass_end + 1)..=mid_end.min(fft_size / 2);
            let high_range = (mid_end + 1)..=high_end.min(fft_size / 2);

            let bass_energy: f32 = if bass_range.is_empty() { 0.0 } else {
                fft_buffer[bass_range.clone()]
                    .iter()
                    .map(|c| c.norm())
                    .sum::<f32>() / (bass_range.end() - bass_range.start() + 1) as f32
            };

            let mid_energy: f32 = if mid_range.is_empty() { 0.0 } else {
                fft_buffer[mid_range.clone()]
                    .iter()
                    .map(|c| c.norm())
                    .sum::<f32>() / (mid_range.end() - mid_range.start() + 1) as f32
            };

            let high_energy: f32 = if high_range.is_empty() { 0.0 } else {
                fft_buffer[high_range.clone()]
                    .iter()
                    .map(|c| c.norm())
                    .sum::<f32>() / (high_range.end() - high_range.start() + 1) as f32
            };

            // Calculate RMS for height
            let segment = &samples[start..end];
            let rms: f32 = (segment.iter().map(|s| s * s).sum::<f32>() / segment.len() as f32).sqrt();

            // Scale values for PWV4 format (7-bit values, 0-127)
            let boost = 16.0;
            let height = (rms * 127.0 * 4.0).clamp(0.0, 127.0) as u8;
            let luminance = ((bass_energy + mid_energy + high_energy) * boost).clamp(0.0, 127.0) as u8;
            let blue = (bass_energy * boost * 2.0).clamp(0.0, 127.0) as u8;
            let red = (bass_energy * boost).clamp(0.0, 127.0) as u8;
            let green = (mid_energy * boost * 1.5).clamp(0.0, 127.0) as u8;
            let blue2 = (high_energy * boost * 2.0).clamp(0.0, 127.0) as u8;

            columns.push(WaveformColorPreviewColumn {
                height,
                luminance,
                blue,
                red,
                green,
                blue2,
            });
        }

        WaveformColorPreview { columns }
    }
    
    /// Generate 400-column preview waveform (PWAV format)
    fn generate_preview(&self, samples: &[f32]) -> WaveformPreview {
        let mut columns = Vec::with_capacity(400);
        
        if samples.is_empty() {
            return WaveformPreview {
                columns: vec![WaveformColumn { height: 0, whiteness: 0 }; 400],
            };
        }
        
        // Divide samples into 400 segments
        let segment_size = samples.len() / 400;
        if segment_size == 0 {
            return WaveformPreview {
                columns: vec![WaveformColumn { height: 0, whiteness: 0 }; 400],
            };
        }
        
        // First pass: collect per-column RMS and peak.
        // A fixed "* 4.0" boost saturates almost every column at height 31 on
        // loud material (any RMS > 0.25), which renders as a solid block.
        // Instead, normalize against the track's own loud-but-not-outlier level.
        let mut rms_vals: Vec<f32> = Vec::with_capacity(400);
        let mut peak_vals: Vec<f32> = Vec::with_capacity(400);

        for i in 0..400 {
            let start = i * segment_size;
            let end = std::cmp::min(start + segment_size, samples.len());
            let segment = &samples[start..end];

            if segment.is_empty() {
                rms_vals.push(0.0);
                peak_vals.push(0.0);
                continue;
            }

            let rms: f32 = (segment.iter().map(|s| s * s).sum::<f32>()
                           / segment.len() as f32).sqrt();
            let peak: f32 = segment.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
            rms_vals.push(rms);
            peak_vals.push(peak);
        }

        // Reference level = 95th percentile of column RMS, so a few loud
        // transients don't crush the rest of the waveform.
        let mut sorted = rms_vals.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((sorted.len() as f32 * 0.95) as usize).min(sorted.len().saturating_sub(1));
        let reference = sorted.get(idx).copied().unwrap_or(0.0).max(1e-6);

        for i in 0..400 {
            let rms = rms_vals[i];
            let peak = peak_vals[i];

            // Golden PWAV never reaches 31: pooled max=25, p95~=22, and the
            // waveform sits mostly in the mid-20s with dips. Map the 95th
            // percentile to 22 so loud sections land there and only true peaks
            // approach the ceiling. (The old target of 28 still read as a block.)
            let height = ((rms / reference) * 22.0).clamp(0.0, 31.0) as u8;

            // Whiteness (crest factor). Golden caps at 5, so clamp there.
            let crest = if rms > 0.001 { peak / rms } else { 1.0 };
            let whiteness = ((crest - 1.0) / 2.0).clamp(0.0, 5.0) as u8;

            columns.push(WaveformColumn { height, whiteness });
        }
        
        WaveformPreview { columns }
    }
    
    /// Generate detail color waveform (PWV5 format, 150 entries/second)
    fn generate_detail(&self, samples: &[f32], duration_secs: f64) -> WaveformDetail {
        // 150 entries per second
        let num_entries = (duration_secs * 150.0).ceil() as usize;
        let num_entries = num_entries.max(1);
        let mut entries = Vec::with_capacity(num_entries);
        // Raw (bass, mid, high, amplitude) per entry, normalized after the loop.
        let mut raw: Vec<(f32, f32, f32, f32)> = Vec::with_capacity(num_entries);
        
        if samples.is_empty() {
            return WaveformDetail {
                entries: vec![WaveformColorEntry::default(); num_entries],
            };
        }
        
        // FFT setup
        let fft_size = 1024;
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);
        
        // Samples per waveform entry
        let samples_per_entry = self.sample_rate as usize / 150;
        if samples_per_entry == 0 {
            return WaveformDetail {
                entries: vec![WaveformColorEntry::default(); num_entries],
            };
        }
        
        // Hann window
        let window: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / fft_size as f32).cos()))
            .collect();
        
        // Frequency bin ranges for each color
        let bin_hz = self.sample_rate as f32 / fft_size as f32;
        let bass_start = (20.0 / bin_hz).ceil() as usize;
        let bass_end = (200.0 / bin_hz) as usize;
        let mid_end = (4000.0 / bin_hz) as usize;
        let high_end = std::cmp::min((20000.0 / bin_hz) as usize, fft_size / 2);
        
        for entry_idx in 0..num_entries {
            let sample_start = entry_idx * samples_per_entry;
            
            if sample_start >= samples.len() {
                entries.push(WaveformColorEntry::default());
                continue;
            }
            
            // Get FFT window of samples
            let mut fft_buffer: Vec<Complex<f32>> = (0..fft_size)
                .map(|i| {
                    let sample_idx = sample_start + i;
                    let sample = if sample_idx < samples.len() {
                        samples[sample_idx]
                    } else {
                        0.0
                    };
                    Complex::new(sample * window[i], 0.0)
                })
                .collect();
            
            // Run FFT
            fft.process(&mut fft_buffer);
            
            // Calculate magnitude for each frequency band
            let bass_range = bass_start.max(1)..=bass_end.min(fft_size / 2);
            let mid_range = (bass_end + 1)..=mid_end.min(fft_size / 2);
            let high_range = (mid_end + 1)..=high_end.min(fft_size / 2);
            
            let bass_energy: f32 = if bass_range.is_empty() { 0.0 } else {
                fft_buffer[bass_range.clone()]
                    .iter()
                    .map(|c| c.norm())
                    .sum::<f32>() / (bass_range.end() - bass_range.start() + 1) as f32
            };
            
            let mid_energy: f32 = if mid_range.is_empty() { 0.0 } else {
                fft_buffer[mid_range.clone()]
                    .iter()
                    .map(|c| c.norm())
                    .sum::<f32>() / (mid_range.end() - mid_range.start() + 1) as f32
            };
            
            let high_energy: f32 = if high_range.is_empty() { 0.0 } else {
                fft_buffer[high_range.clone()]
                    .iter()
                    .map(|c| c.norm())
                    .sum::<f32>() / (high_range.end() - high_range.start() + 1) as f32
            };
            
            // Collect raw energies; normalization happens in a second pass below.
            let segment_end = std::cmp::min(sample_start + samples_per_entry, samples.len());
            let amplitude = if sample_start < segment_end {
                let segment = &samples[sample_start..segment_end];
                (segment.iter().map(|s| s * s).sum::<f32>() / segment.len() as f32).sqrt()
            } else {
                0.0
            };

            raw.push((bass_energy, mid_energy, high_energy, amplitude));
        }

        // Single shared reference (95th-percentile overall amplitude). Using a
        // SEPARATE reference per band would normalize each colour channel to the
        // same level and destroy the band balance - but golden PWV5 is strongly
        // red-forward on techno (r mean ~6.6, b ~3, g ~2). A shared reference
        // preserves that relative balance.
        let pct95 = |mut v: Vec<f32>| -> f32 {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let i = ((v.len() as f32 * 0.95) as usize).min(v.len().saturating_sub(1));
            v.get(i).copied().unwrap_or(0.0).max(1e-6)
        };
        let r_amp = pct95(raw.iter().map(|e| e.3).collect());
        // Reference band energy: the loudest band's 95th percentile, so the
        // dominant band reaches ~7 and quieter bands scale proportionally.
        let r_band = pct95(
            raw.iter()
                .map(|e| e.0.max(e.1).max(e.2))
                .collect(),
        );

        for (bass_energy, mid_energy, high_energy, amplitude) in raw {
            let red = ((bass_energy / r_band) * 7.0).clamp(0.0, 7.0) as u8;
            let green = ((mid_energy / r_band) * 7.0).clamp(0.0, 7.0) as u8;
            let blue = ((high_energy / r_band) * 7.0).clamp(0.0, 7.0) as u8;

            // Golden PWV5 height: peaks reach 31 but the MEAN is low (~10),
            // i.e. quiet with occasional transients. Map p95 to ~18 so most
            // entries sit low and only transients approach the ceiling.
            let height = ((amplitude / r_amp) * 18.0).clamp(0.0, 31.0) as u8;

            entries.push(WaveformColorEntry { red, green, blue, height });
        }
        
        WaveformDetail { entries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_preview_generation() {
        let gen = WaveformGenerator::new(44100);
        
        // Generate 1 second of sine wave
        let samples: Vec<f32> = (0..44100)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 44100.0).sin())
            .collect();
        
        let preview = gen.generate_preview(&samples);
        
        assert_eq!(preview.columns.len(), 400);
        // All columns should have some amplitude
        assert!(preview.columns.iter().any(|c| c.height > 0));
    }
    
    #[test]
    fn test_detail_generation() {
        let gen = WaveformGenerator::new(44100);
        
        // Generate 1 second of sine wave
        let samples: Vec<f32> = (0..44100)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 44100.0).sin())
            .collect();
        
        let detail = gen.generate_detail(&samples, 1.0);
        
        // 1 second at 150 entries/sec = 150 entries
        assert_eq!(detail.entries.len(), 150);
    }
    
    #[test]
    fn test_empty_samples() {
        let gen = WaveformGenerator::new(44100);
        let waveform = gen.generate(&[], 0.0);
        
        assert_eq!(waveform.preview.columns.len(), 400);
        assert!(waveform.detail.entries.len() >= 1);
    }
}