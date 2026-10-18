//! The capture buffer used by sweep measurements.
//!
//! The UI drains the input channel every frame and pushes blocks in here; once
//! enough samples have arrived, the measurement is finished.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordState {
    Idle,
    /// Wait for the stream to settle before counting samples as real data.
    Priming { remaining: usize },
    Recording,
    Done,
}

pub struct Recorder {
    pub state: RecordState,
    pub data: Vec<f32>,
    pub target_len: usize,
    pub sr: f32,
    /// Largest absolute sample seen while recording, for the clip warning.
    pub peak: f32,
    pub clipped: bool,
}

impl Recorder {
    pub fn new(sr: f32) -> Self {
        Self {
            state: RecordState::Idle,
            data: Vec::new(),
            target_len: 0,
            sr,
            peak: 0.0,
            clipped: false,
        }
    }

    pub fn arm(&mut self, target_len: usize, prime_samples: usize) {
        self.data.clear();
        self.data.reserve(target_len);
        self.target_len = target_len;
        self.peak = 0.0;
        self.clipped = false;
        self.state = if prime_samples > 0 {
            RecordState::Priming {
                remaining: prime_samples,
            }
        } else {
            RecordState::Recording
        };
    }

    pub fn cancel(&mut self) {
        self.state = RecordState::Idle;
        self.data.clear();
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            RecordState::Priming { .. } | RecordState::Recording
        )
    }

    pub fn progress(&self) -> f32 {
        if self.target_len == 0 {
            0.0
        } else {
            (self.data.len() as f32 / self.target_len as f32).clamp(0.0, 1.0)
        }
    }

    pub fn push(&mut self, block: &[f32]) {
        match self.state {
            RecordState::Priming { remaining } => {
                if block.len() >= remaining {
                    let rest = &block[remaining..];
                    self.state = RecordState::Recording;
                    self.push_data(rest);
                } else {
                    self.state = RecordState::Priming {
                        remaining: remaining - block.len(),
                    };
                }
            }
            RecordState::Recording => self.push_data(block),
            _ => {}
        }
    }

    fn push_data(&mut self, block: &[f32]) {
        for &s in block {
            let a = s.abs();
            if a > self.peak {
                self.peak = a;
            }
            if a >= 0.999 {
                self.clipped = true;
            }
            if self.data.len() < self.target_len {
                self.data.push(s);
            }
        }
        if self.data.len() >= self.target_len {
            self.state = RecordState::Done;
        }
    }

    pub fn take(&mut self) -> Vec<f32> {
        self.state = RecordState::Idle;
        std::mem::take(&mut self.data)
    }
}

impl Recorder {
    /// How much has been captured so far, in seconds.
    pub fn seconds(&self) -> f32 {
        if self.sr <= 0.0 {
            0.0
        } else {
            self.data.len() as f32 / self.sr
        }
    }

    /// How long the armed capture will run for, in seconds.
    pub fn target_seconds(&self) -> f32 {
        if self.sr <= 0.0 {
            0.0
        } else {
            self.target_len as f32 / self.sr
        }
    }
}
