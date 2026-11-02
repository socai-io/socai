//! Measure system suspend independently of wall-clock changes or app scheduling.
use serde::Serialize;

#[derive(Clone, Copy, Debug)]
pub struct SuspendClock {
    boot_ms: u64,
    awake_ms: u64,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct ClockInterval {
    pub elapsed_ms: u64,
    pub awake_ms: u64,
    pub suspend_ms: u64,
}

impl ClockInterval {
    pub fn slept(&self) -> bool {
        self.suspend_ms >= 2_000
    }
}

impl SuspendClock {
    pub fn elapsed(self) -> Option<ClockInterval> {
        Self::sample()?.since(self)
    }

    pub fn since(self, before: Self) -> Option<ClockInterval> {
        let elapsed_ms = self.boot_ms.checked_sub(before.boot_ms)?;
        let awake_ms = self.awake_ms.checked_sub(before.awake_ms)?;
        Some(ClockInterval {
            elapsed_ms,
            awake_ms,
            suspend_ms: elapsed_ms.saturating_sub(awake_ms),
        })
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn sample() -> Option<Self> {
        #[cfg(target_os = "macos")]
        let clocks = (libc::CLOCK_MONOTONIC_RAW, libc::CLOCK_UPTIME_RAW);
        #[cfg(target_os = "linux")]
        let clocks = (libc::CLOCK_BOOTTIME, libc::CLOCK_MONOTONIC);
        fn read(id: libc::clockid_t) -> Option<u64> {
            let mut ts = std::mem::MaybeUninit::uninit();
            // SAFETY: clock_gettime initializes ts on success.
            if unsafe { libc::clock_gettime(id, ts.as_mut_ptr()) } != 0 {
                return None;
            }
            let ts = unsafe { ts.assume_init() };
            Some(ts.tv_sec as u64 * 1_000 + ts.tv_nsec as u64 / 1_000_000)
        }
        Some(Self {
            boot_ms: read(clocks.0)?,
            awake_ms: read(clocks.1)?,
        })
    }

    #[cfg(target_os = "windows")]
    pub fn sample() -> Option<Self> {
        use windows_sys::Win32::System::WindowsProgramming::{
            QueryInterruptTime, QueryUnbiasedInterruptTime,
        };
        let (mut boot, mut awake) = (0, 0);
        // SAFETY: both APIs write 100 ns units to the provided stack storage.
        unsafe {
            QueryInterruptTime(&mut boot);
            if QueryUnbiasedInterruptTime(&mut awake) == 0 {
                return None;
            }
        }
        Some(Self {
            boot_ms: boot / 10_000,
            awake_ms: awake / 10_000,
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    pub fn sample() -> Option<Self> {
        None
    }
}
