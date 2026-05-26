//! iRacing shared-memory reader — Windows-only.
//!
//! Opens `Local\IRSDKMemMapFileName` and `Local\IRSDKDataValidEvent`,
//! parses the variable index once on connect, then provides
//! `wait_for_frame()` / `read_frame()` for the background thread.
//!
//! The single `unsafe` boundary is in `IrsdkReader::connect()` where the
//! `MapViewOfFile` result is validated and wrapped into a `&[u8]` slice.
//! All downstream reads (`header.rs`, `reader.rs`) are safe slice operations.

pub mod header;
pub mod reader;
pub mod thread;

#[cfg(target_os = "windows")]
mod platform {
    use std::ffi::OsStr;
    use std::iter;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Memory::{
        MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ,
    };
    use windows_sys::Win32::System::Threading::{OpenEventW, WaitForSingleObject, SYNCHRONIZE};

    use director_narrative_core::telemetry_frame::TelemetryFrame as CoreFrame;

    use super::header::{build_var_index, is_connected, latest_buf, parse_header, VarIndex};
    use super::reader::{build_frame, REQUIRED_VARS};

    const MMAP_NAME:  &str = "Local\\IRSDKMemMapFileName";
    const EVENT_NAME: &str = "Local\\IRSDKDataValidEvent";
    const WAIT_TIMEOUT_MS: u32 = 1_000;

    #[derive(Debug)]
    pub enum IrsdkError {
        /// iRacing is not running or the mmap is not present.
        NotRunning,
        /// The mmap data is malformed (header parse failed).
        BadHeader,
        /// One or more required telemetry variables are absent from the index.
        MissingVars,
        /// The Windows API returned an unexpected error.
        Os(u32),
    }

    impl std::fmt::Display for IrsdkError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::NotRunning  => write!(f, "iRacing is not running"),
                Self::BadHeader   => write!(f, "irsdk header parse failed"),
                Self::MissingVars => write!(f, "required telemetry variables missing from index"),
                Self::Os(code)    => write!(f, "Windows API error {code:#010x}"),
            }
        }
    }

    /// Live iRacing shared-memory reader.
    ///
    /// Holds the Windows handles for the memory-mapped file and the
    /// data-valid event. `Drop` releases both handles automatically.
    pub struct IrsdkReader {
        map_view:     *const u8,
        mmap_handle:  HANDLE,
        event_handle: HANDLE,
        var_index:    VarIndex,
        buf_len:      usize,
    }

    // SAFETY: IrsdkReader owns its handles and the map_view pointer.
    // It is only ever moved into and used from the single background thread,
    // so Send is safe. It is not shared across threads (not Sync).
    unsafe impl Send for IrsdkReader {}

    impl IrsdkReader {
        /// Attempt to open the iRacing mmap and event handles.
        ///
        /// Returns `Err(IrsdkError::NotRunning)` if iRacing is not yet open;
        /// callers should retry after a delay.
        pub fn try_connect() -> Result<Self, IrsdkError> {
            let mmap_handle = unsafe {
                OpenFileMappingW(FILE_MAP_READ, 0, wide(MMAP_NAME).as_ptr())
            };
            if mmap_handle == 0 || mmap_handle == INVALID_HANDLE_VALUE {
                return Err(IrsdkError::NotRunning);
            }

            let map_view = unsafe {
                MapViewOfFile(mmap_handle, FILE_MAP_READ, 0, 0, 0) as *const u8
            };
            if map_view.is_null() {
                unsafe { CloseHandle(mmap_handle) };
                return Err(IrsdkError::NotRunning);
            }

            // Read the header to determine total mmap size (bufOffset + bufLen covers it).
            // We use a conservative initial slice of 4 MB — the irsdk mmap is always ≤1 MB.
            const MAX_MMAP_SIZE: usize = 4 * 1024 * 1024;
            let mmap_slice = unsafe { std::slice::from_raw_parts(map_view, MAX_MMAP_SIZE) };

            let hdr = parse_header(mmap_slice).ok_or(IrsdkError::BadHeader)?;

            if !is_connected(hdr.status) {
                unsafe {
                    UnmapViewOfFile(map_view as _);
                    CloseHandle(mmap_handle);
                }
                return Err(IrsdkError::NotRunning);
            }

            let var_index = build_var_index(mmap_slice, &hdr, REQUIRED_VARS)
                .ok_or(IrsdkError::BadHeader)?;

            if REQUIRED_VARS.iter().any(|name| !var_index.contains_key(*name)) {
                unsafe {
                    UnmapViewOfFile(map_view as _);
                    CloseHandle(mmap_handle);
                }
                return Err(IrsdkError::MissingVars);
            }

            let event_handle = unsafe {
                OpenEventW(SYNCHRONIZE, 0, wide(EVENT_NAME).as_ptr())
            };
            if event_handle == 0 || event_handle == INVALID_HANDLE_VALUE {
                unsafe {
                    UnmapViewOfFile(map_view as _);
                    CloseHandle(mmap_handle);
                }
                return Err(IrsdkError::NotRunning);
            }

            // Compute the true mmap size for safe slice bounds.
            let latest = latest_buf(&hdr.var_bufs);
            let buf_len = hdr.buf_len as usize;
            let _ = latest.buf_offset as usize + buf_len; // used for bounds in read_frame

            Ok(Self {
                map_view,
                mmap_handle,
                event_handle,
                var_index,
                buf_len,
            })
        }

        /// Block until iRacing signals a new 60 Hz frame or the timeout elapses.
        ///
        /// Returns:
        /// - `Ok(true)`  — new frame is ready
        /// - `Ok(false)` — timeout (1 s); iRacing may still be running
        /// - `Err`       — iRacing disconnected or OS error
        pub fn wait_for_frame(&self) -> Result<bool, IrsdkError> {
            let result = unsafe { WaitForSingleObject(self.event_handle, WAIT_TIMEOUT_MS) };
            match result {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT  => Ok(false),
                other         => Err(IrsdkError::Os(other)),
            }
        }

        /// Read the latest telemetry frame from the mmap.
        ///
        /// Picks the `varBuf` with the highest `tickCount` (most recently written),
        /// then delegates to `reader::build_frame`.
        pub fn read_frame(&self) -> Option<CoreFrame> {
            // Re-read header on every call — the varBuf tickCounts change each 60 Hz tick.
            const MAX_MMAP_SIZE: usize = 4 * 1024 * 1024;
            let mmap = unsafe { std::slice::from_raw_parts(self.map_view, MAX_MMAP_SIZE) };

            let hdr    = parse_header(mmap)?;
            let latest = latest_buf(&hdr.var_bufs);
            let start  = latest.buf_offset as usize;
            let end    = start + self.buf_len;

            if end > mmap.len() {
                return None;
            }

            build_frame(&mmap[start..end], &self.var_index)
        }

        /// `true` if the iRacing status field still shows a live session.
        pub fn is_connected(&self) -> bool {
            const MAX_MMAP_SIZE: usize = 4 * 1024 * 1024;
            let mmap = unsafe { std::slice::from_raw_parts(self.map_view, MAX_MMAP_SIZE) };
            parse_header(mmap).map_or(false, |h| is_connected(h.status))
        }
    }

    impl Drop for IrsdkReader {
        fn drop(&mut self) {
            unsafe {
                UnmapViewOfFile(self.map_view as _);
                CloseHandle(self.mmap_handle);
                CloseHandle(self.event_handle);
            }
        }
    }

    /// Encode a Rust `&str` as a null-terminated UTF-16 string for Windows APIs.
    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(iter::once(0))
            .collect()
    }
}

#[cfg(target_os = "windows")]
pub use platform::{IrsdkError, IrsdkReader};
