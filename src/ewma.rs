use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Ewma {
    alpha: f32,
    value: f32,
    n: usize,
}

impl Ewma {
    pub fn new(alpha: f32) -> Self {
        Self {
            alpha,
            value: 0.,
            n: 0,
        }
    }
    pub fn record(&mut self, measurement: f32) {
        if self.n == 0 {
            self.value = measurement;
        }
        if self.n < 20 {
            let alpha = self.alpha + (1. / (1 + self.n) as f32) * (1. - self.alpha);
            self.value = alpha * measurement + (1. - alpha) * self.value;
        } else {
            self.value = self.alpha * measurement + (1. - self.alpha) * self.value;
        }

        self.n += 1;
    }

    pub fn reset(&mut self) {
        self.n = 0;
        self.value = 0.;
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}
