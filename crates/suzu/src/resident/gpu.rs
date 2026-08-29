//! GPU utilization — the report's third slot.
//!
//! Windows: the GPU Engine performance counters, the same source Task
//! Manager reads — vendor-agnostic (NVIDIA, AMD, Intel, USB display
//! adapters all publish here). The 3D engines are summed and clamped;
//! that is the honest "how busy is the graphics silicon" number.
//!
//! No GPU, no counters, non-Windows: `None` — and the face draws a
//! dash, because "not measured" and "zero" are different truths.

#[cfg(windows)]
pub fn capture() -> Option<u8> {
    pdh::capture()
}

#[cfg(not(windows))]
pub fn capture() -> Option<u8> {
    None
}

#[cfg(windows)]
mod pdh {
    use std::sync::OnceLock;
    use std::sync::Mutex;
    use windows_sys::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
        PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W,
        PDH_FMT_DOUBLE,
    };

    const COUNTER_PATH: &str = "\\GPU Engine(*)\\Utilization Percentage";

    struct Query {
        query: isize,
        counter: isize,
    }

    // The query lives for the process: performance counters need a
    // collection interval between reads, which the ground cadence
    // (~2 s) provides after the prime tick.
    static QUERY: OnceLock<Mutex<Option<Query>>> = OnceLock::new();

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn capture() -> Option<u8> {
        let slot = QUERY.get_or_init(|| Mutex::new(None));
        let mut guard = slot.lock().ok()?;
        if guard.is_none() {
            let mut query: isize = 0;
            let mut counter: isize = 0;
            unsafe {
                if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                    return None;
                }
                let path = wide(COUNTER_PATH);
                if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) != 0 {
                    PdhCloseQuery(query);
                    return None;
                }
            }
            *guard = Some(Query { query, counter });
        }
        let q = guard.as_ref()?;

        unsafe {
            // Prime tick: one collect has no interval behind it, so a
            // formatted read right now is "not measured" by definition.
            if PdhCollectQueryData(q.query) != 0 {
                return None;
            }
            let mut size: u32 = 0;
            let mut count: u32 = 0;
            let status = PdhGetFormattedCounterArrayW(
                q.counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                std::ptr::null_mut(),
            );
            if status != 0x8000_07D2 {
                // PDH_MORE_DATA expected; anything else has no array.
                return None;
            }
            let mut buffer = vec![0u8; size as usize];
            let items = buffer.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(
                q.counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                items,
            ) != 0
            {
                return None;
            }

            // Each item names an engine instance (WIDE strings):
            // pid_…_luid_…_phys_…_eng_…_engtype_3D. Sum the 3D engines
            // (the graphics work; copy/codec engines would double-count
            // the same silicon) and clamp — the face is a percentage.
            let mut sum: f64 = 0.0;
            for i in 0..count as usize {
                let item = &*items.add(i);
                let mut len = 0usize;
                unsafe {
                    while *item.szName.add(len) != 0 {
                        len += 1;
                    }
                }
                let name = String::from_utf16_lossy(unsafe {
                    std::slice::from_raw_parts(item.szName, len)
                })
                .to_lowercase();
                if name.contains("engtype_3d") && item.FmtValue.CStatus == 0 {
                    sum += unsafe { item.FmtValue.Anonymous.doubleValue };
                }
            }
            Some(sum.clamp(0.0, 100.0).round() as u8)
        }
    }
}
