#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatencySample {
    name: &'static str,
    start_ns: u64,
    end_ns: u64,
}

impl LatencySample {
    pub fn new(name: &'static str, start_ns: u64, end_ns: u64) -> Self {
        Self {
            name,
            start_ns,
            end_ns,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn elapsed_ns(&self) -> u64 {
        self.end_ns.saturating_sub(self.start_ns)
    }
}
