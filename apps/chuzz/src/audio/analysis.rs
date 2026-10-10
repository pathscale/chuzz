//! Analyser history and FFT over rendered PCM.

use std::f64::consts::PI;

use super::pcm::Pcm;

pub(crate) struct Analysis {
    history: Vec<f32>,
    position: usize,
    fft_size: usize,
    min_decibels: f64,
    max_decibels: f64,
    smoothing: f64,
    smoothed: Vec<f64>,
    spectrum: Vec<f32>,
    dirty: bool,
}

impl Default for Analysis {
    fn default() -> Self {
        Self {
            history: vec![0.0; 32_768],
            position: 0,
            fft_size: 2_048,
            min_decibels: -100.0,
            max_decibels: -30.0,
            smoothing: 0.8,
            smoothed: vec![0.0; 1_024],
            spectrum: vec![f32::NEG_INFINITY; 1_024],
            dirty: true,
        }
    }
}

impl Analysis {
    pub(crate) fn configure(
        &mut self,
        fft_size: usize,
        min_decibels: f64,
        max_decibels: f64,
        smoothing: f64,
    ) -> Result<(), String> {
        if !(32..=32_768).contains(&fft_size)
            || !fft_size.is_power_of_two()
            || !min_decibels.is_finite()
            || !max_decibels.is_finite()
            || min_decibels >= max_decibels
            || !smoothing.is_finite()
            || !(0.0..=1.0).contains(&smoothing)
        {
            return Err("IndexSizeError: invalid analyser configuration".to_owned());
        }
        if self.fft_size != fft_size {
            self.smoothed = vec![0.0; fft_size / 2];
            self.spectrum = vec![f32::NEG_INFINITY; fft_size / 2];
        }
        self.fft_size = fft_size;
        self.min_decibels = min_decibels;
        self.max_decibels = max_decibels;
        self.smoothing = smoothing;
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn push(&mut self, pcm: &Pcm) {
        for frame in 0..pcm.frames() {
            self.history[self.position] = pcm.at(frame, 0, 1);
            self.position = (self.position + 1) % self.history.len();
        }
        self.dirty = true;
    }

    fn time(&self) -> Vec<f32> {
        let start = (self.position + self.history.len() - self.fft_size) % self.history.len();
        (0..self.fft_size)
            .map(|index| self.history[(start + index) % self.history.len()])
            .collect()
    }

    fn frequency(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let samples = self.time();
        let size = self.fft_size;
        let mut values = vec![(0.0_f64, 0.0_f64); size];
        let bits = size.trailing_zeros();
        let mut window_sum = 0.0;
        for (index, sample) in samples.into_iter().enumerate() {
            let angle = 2.0 * PI * index as f64 / size as f64;
            let window = 0.42 - 0.5 * angle.cos() + 0.08 * (2.0 * angle).cos();
            window_sum += window;
            let reversed = index.reverse_bits() >> (usize::BITS - bits);
            values[reversed] = (f64::from(sample) * window, 0.0);
        }

        let mut width = 2;
        while width <= size {
            for base in (0..size).step_by(width) {
                for index in 0..width / 2 {
                    let angle = -2.0 * PI * index as f64 / width as f64;
                    let (real, imaginary) = values[base + index + width / 2];
                    let product = (
                        real * angle.cos() - imaginary * angle.sin(),
                        real * angle.sin() + imaginary * angle.cos(),
                    );
                    let first = values[base + index];
                    values[base + index] = (first.0 + product.0, first.1 + product.1);
                    values[base + index + width / 2] =
                        (first.0 - product.0, first.1 - product.1);
                }
            }
            width *= 2;
        }

        for (index, &(real, imaginary)) in values.iter().take(size / 2).enumerate() {
            let magnitude = real.hypot(imaginary) / window_sum;
            let smoothed =
                self.smoothing * self.smoothed[index] + (1.0 - self.smoothing) * magnitude;
            self.smoothed[index] = smoothed;
            self.spectrum[index] = if smoothed > 0.0 {
                (20.0 * smoothed.log10()) as f32
            } else {
                f32::NEG_INFINITY
            };
        }
    }

    pub(crate) fn bytes(&mut self, kind: &str) -> Result<Vec<u8>, String> {
        match kind {
            "byteTime" => Ok(self
                .time()
                .into_iter()
                .map(|sample| (128.0 * (1.0 + f64::from(sample))).clamp(0.0, 255.0) as u8)
                .collect()),
            "floatTime" => Ok(self
                .time()
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect()),
            "byteFrequency" => {
                self.frequency();
                Ok(self
                    .spectrum
                    .iter()
                    .map(|&decibels| {
                        (255.0 * (f64::from(decibels) - self.min_decibels)
                            / (self.max_decibels - self.min_decibels))
                            .clamp(0.0, 255.0) as u8
                    })
                    .collect())
            }
            "floatFrequency" => {
                self.frequency();
                Ok(self
                    .spectrum
                    .iter()
                    .copied()
                    .flat_map(f32::to_le_bytes)
                    .collect())
            }
            _ => Err("NotSupportedError: unknown analyser readback".to_owned()),
        }
    }
}
