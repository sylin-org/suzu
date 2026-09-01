//! Built-in host metrics collection.

use super::events::ResidentEvent;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use sysinfo::System;
use tokio::sync::broadcast::Sender;

#[derive(Debug, Clone, Serialize)]
pub struct HostMetrics {
    pub name: String,
    pub uptime_s: u64,
    pub cpu: u8,
    pub mem: u8,
    pub disk: u8,
    /// `None` means "not measured" and is displayed as a dash rather than zero.
    pub gpu: Option<u8>,
}

const FAST_TICK: Duration = Duration::from_millis(200);
const METRICS_EVERY: u64 = 10; // publish host metrics about every 2 seconds

pub struct Sensor {
    events: Sender<ResidentEvent>,
    /// Latest captured host state (ADR-0006). Device sessions read metrics and
    /// scalar updates from this cache on their own interval; events only notify
    /// other components that values changed.
    host_state: Arc<super::devices::HostStateCache>,
    sys: System,
    disks: sysinfo::Disks,
    last: Option<HostMetrics>,
}

impl Sensor {
    pub fn new(events: Sender<ResidentEvent>, host_state: Arc<super::devices::HostStateCache>) -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let disks = sysinfo::Disks::new_with_refreshed_list();
        // Prime CPU measurement because the first sample is always zero.
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_cpu_usage();
        Self {
            events,
            host_state,
            sys,
            disks,
            last: None,
        }
    }

    fn capture(&mut self) -> HostMetrics {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.disks.refresh_list();

        let cpu = self.sys.global_cpu_info().cpu_usage().clamp(0.0, 100.0) as u8;
        let mem = if self.sys.total_memory() > 0 {
            (self.sys.used_memory() * 100 / self.sys.total_memory()) as u8
        } else {
            0
        };
        // Use the largest mounted volume for storage utilization.
        let mut biggest: Option<(u64, u64)> = None; // (total, available)
        for d in self.disks.list() {
            if biggest.is_none_or(|(t, _)| d.total_space() > t) {
                biggest = Some((d.total_space(), d.available_space()));
            }
        }
        let disk = biggest
            .map(|(t, a)| ((t - a) * 100 / t.max(1)) as u8)
            .unwrap_or(0);

        HostMetrics {
            name: System::host_name().unwrap_or_else(|| "unnamed".into()),
            uptime_s: System::uptime(),
            cpu,
            mem,
            disk,
            gpu: super::gpu::capture(),
        }
    }

    pub async fn run(mut self) {
        // Prime CPU statistics across a real interval; the first immediate
        // sample is not meaningful.
        let _prime = self.capture();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut ticks: u64 = 0;
        let mut audio: u8 = 40; // Placeholder scalar source using decay and noise.
        let mut rng: u32 = 0x9e37_79b9;
        loop {
            ticks += 1;

            // High-frequency scalar sensor updates.
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let attack = ((rng >> 16) % 30) as u8;
            audio = ((audio as u32 * 3 / 4) + attack as u32).min(100) as u8;
            let _ = self.events.send(ResidentEvent::Pulse {
                axis: "audio.level",
                value: audio,
            });
            self.host_state.set_pulse("audio.level".into(), audio);

            // Publish host metrics only after a value changes.
            if ticks.is_multiple_of(METRICS_EVERY) {
                let captured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || self.capture(),
                ));
                let report = match captured {
                    Ok(report) => report,
                    Err(_) => {
                        // A capture that panics costs one cycle, not
                        // the sensor: publish the reason and
                        // next tick tries again.
                        let _ = self.events.send(ResidentEvent::Degraded {
                            domain: "sensor",
                            reason: "capture panicked — cycle skipped, retrying".into(),
                        });
                        tokio::time::sleep(FAST_TICK).await;
                        continue;
                    }
                };
                // Only publish changed values.
                let changed = match &self.last {
                    None => true,
                    Some(prev) => {
                        prev.cpu != report.cpu
                            || prev.mem != report.mem
                            || prev.disk != report.disk
                            || prev.gpu != report.gpu
                            || prev.name != report.name
                    }
                };
                if changed {
                    self.last = Some(report.clone());
                    let _ = self.events.send(ResidentEvent::HostMetricsChanged {
                        name: report.name.clone(),
                        uptime_s: report.uptime_s,
                        cpu: report.cpu,
                        mem: report.mem,
                        disk: report.disk,
                        gpu: report.gpu,
                    });
                    self.host_state.set_metrics(Arc::new(report));
                }
            }
            tokio::time::sleep(FAST_TICK).await;
        }
    }
}
