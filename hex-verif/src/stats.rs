use std::sync::Mutex;
use std::time::Instant;

/// Shared verification statistics (thread-safe).
pub struct Stats {
    inner: Mutex<StatsInner>,
    start_time: Instant,
}

struct StatsInner {
    iterations_completed: usize,
    iterations_failed: usize,
    build_failures: usize,
    mismatches: usize,
    instructions_tested: usize,
    packets_tested: usize,
}

impl Stats {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(StatsInner {
                iterations_completed: 0,
                iterations_failed: 0,
                build_failures: 0,
                mismatches: 0,
                instructions_tested: 0,
                packets_tested: 0,
            }),
            start_time: Instant::now(),
        }
    }

    pub fn record_success(&self, packets: usize, instructions: usize) {
        let mut inner = self.inner.lock().unwrap();
        inner.iterations_completed += 1;
        inner.packets_tested += packets;
        inner.instructions_tested += instructions;
    }

    pub fn record_build_failure(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.iterations_completed += 1;
        inner.build_failures += 1;
        inner.iterations_failed += 1;
    }

    pub fn record_mismatch(&self, packets: usize, instructions: usize) {
        let mut inner = self.inner.lock().unwrap();
        inner.iterations_completed += 1;
        inner.mismatches += 1;
        inner.iterations_failed += 1;
        inner.packets_tested += packets;
        inner.instructions_tested += instructions;
    }

    /// Generate a summary report.
    pub fn report(&self) -> String {
        let inner = self.inner.lock().unwrap();
        let elapsed = self.start_time.elapsed();
        let throughput = if elapsed.as_secs() > 0 {
            inner.iterations_completed as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        format!(
            "Verification Summary\n\
             ====================\n\
             Duration:            {:.1}s\n\
             Iterations:          {} ({:.1}/s)\n\
             Passed:              {}\n\
             Build failures:      {}\n\
             Mismatches:          {}\n\
             Packets tested:      {}\n\
             Instructions tested: {}",
            elapsed.as_secs_f64(),
            inner.iterations_completed,
            throughput,
            inner.iterations_completed - inner.iterations_failed,
            inner.build_failures,
            inner.mismatches,
            inner.packets_tested,
            inner.instructions_tested,
        )
    }
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_basic() {
        let stats = Stats::new();
        stats.record_success(10, 30);
        stats.record_success(5, 15);
        stats.record_mismatch(8, 24);
        stats.record_build_failure();

        let report = stats.report();
        assert!(report.contains("Iterations:          4"));
        assert!(report.contains("Passed:              2"));
        assert!(report.contains("Mismatches:          1"));
        assert!(report.contains("Build failures:      1"));
    }
}
