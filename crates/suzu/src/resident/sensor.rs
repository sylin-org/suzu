//! The sensor domain — the built-in environment capture. Ground-source
//! zero: it speaks for the machine with no producers and no users.

use super::events::HouseEvent;
use serde::Serialize;
use std::time::Duration;
use sysinfo::System;
use tokio::sync::broadcast::Sender;

#[derive(Debug, Clone, Serialize)]
pub struct MachineReport {
    pub name: String,
    pub uptime_s: u64,
    pub cpu: u8,
    pub mem: u8,
    pub disk: u8,
    /// `None` is "not measured" — the face draws a dash, never a zero.
    pub gpu: Option<u8>,
}

const FAST_TICK: Duration = Duration::from_millis(200);
const GROUND_EVERY: u64 = 10; // one ground publish per ~2 s

pub struct Sensor {
    events: Sender<HouseEvent>,
    sys: System,
    disks: sysinfo::Disks,
    last: Option<MachineReport>,
}

impl Sensor {
    pub fn new(events: Sender<HouseEvent>) -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let disks = sysinfo::Disks::new_with_refreshed_list();
        // Prime the cpu reading — the first sample is always 0.
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_cpu_usage();
        Self {
            events,
            sys,
            disks,
            last: None,
        }
    }

    fn capture(&mut self) -> MachineReport {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.disks.refresh_list();

        let cpu = self.sys.global_cpu_info().cpu_usage().clamp(0.0, 100.0) as u8;
        let mem = if self.sys.total_memory() > 0 {
            (self.sys.used_memory() * 100 / self.sys.total_memory()) as u8
        } else {
            0
        };
        // The biggest mounted volume speaks for storage.
        let mut biggest: Option<(u64, u64)> = None; // (total, available)
        for d in self.disks.list() {
            if biggest.is_none_or(|(t, _)| d.total_space() > t) {
                biggest = Some((d.total_space(), d.available_space()));
            }
        }
        let disk = biggest
            .map(|(t, a)| ((t - a) * 100 / t.max(1)) as u8)
            .unwrap_or(0);

        MachineReport {
            name: System::host_name().unwrap_or_else(|| "unnamed".into()),
            uptime_s: System::uptime(),
            cpu,
            mem,
            disk,
            gpu: super::gpu::capture(),
        }
    }

    pub async fn run(mut self) {
        // Prime the CPU statistics with a real interval — the first
        // sample is otherwise a 100% lie.
        let _prime = self.capture();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut ticks: u64 = 0;
        let mut audio: u8 = 40; // the stub capture: decay + noise
        let mut rng: u32 = 0x9e37_79b9;
        loop {
            ticks += 1;

            // ── fast lane: pulse atoms (cheap capture, cheap show) ──
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let attack = ((rng >> 16) % 30) as u8;
            audio = ((audio as u32 * 3 / 4) + attack as u32).min(100) as u8;
            let _ = self.events.send(HouseEvent::Pulse {
                axis: "audio.level",
                value: audio,
            });

            // ── slow lane: the ground, on drift ──
            if ticks.is_multiple_of(GROUND_EVERY) {
                let captured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || self.capture(),
                ));
                let report = match captured {
                    Ok(report) => report,
                    Err(_) => {
                        // A capture that panics costs one cycle, not
                        // the sensor: the house hears why and the
                        // next tick tries again.
                        let _ = self.events.send(HouseEvent::Degraded {
                            domain: "sensor",
                            reason: "capture panicked — cycle skipped, retrying".into(),
                        });
                        tokio::time::sleep(FAST_TICK).await;
                        continue;
                    }
                };
                // Only publish on change — the ground drifts silently.
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
                    let _ = self.events.send(HouseEvent::GroundChanged {
                        name: report.name,
                        uptime_s: report.uptime_s,
                        cpu: report.cpu,
                        mem: report.mem,
                        disk: report.disk,
                        gpu: report.gpu,
                    });
                }
            }
            tokio::time::sleep(FAST_TICK).await;
        }
    }
}
