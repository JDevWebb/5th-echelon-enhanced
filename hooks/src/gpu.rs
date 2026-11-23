//! The graphics card and its driver, logged once at start (0.5.6's AE): the DX9 Alt+Enter freeze
//! turned out to be the graphics driver (Intel's, on the Surface), and players' logs didn't say
//! which card or driver they had. Each adapter DXGI lists: its name, vendor and device ids, its
//! memory, and the driver's version (the user-mode driver's, as DXGI gives it). Nothing is sent
//! but the log itself, which goes only where the player sends their logs.

use tracing::info;
use windows::core::Interface as _;
use windows::Win32::Graphics::Dxgi::CreateDXGIFactory1;
use windows::Win32::Graphics::Dxgi::DXGI_ADAPTER_DESC1;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Dxgi::IDXGIFactory1;

/// A driver version as DXGI packs it into 64 bits: four 16-bit parts, high first.
fn driver_version(packed: i64) -> String {
    let v = packed as u64;
    format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xffff, (v >> 16) & 0xffff, v & 0xffff)
}

/// Logs the graphics adapters (at most 4) once.
pub fn log_adapters() {
    let spawned = std::thread::Builder::new().name("fe-gpu".into()).spawn(|| {
        // SAFETY: plain DXGI calls on this thread's own factory.
        let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
            info!("Graphics: DXGI didn't list the adapters");
            return;
        };
        for i in 0..4 {
            // SAFETY: as above; an index past the last fails and ends the list.
            let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else { break };
            let mut desc = DXGI_ADAPTER_DESC1::default();
            // SAFETY: as above, into our own struct.
            if unsafe { adapter.GetDesc1(&mut desc) }.is_err() {
                continue;
            }
            let end = desc.Description.iter().position(|c| *c == 0).unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..end]);
            // SAFETY: as above; IDXGIDevice's interface id is a static.
            let driver = unsafe { adapter.CheckInterfaceSupport(&IDXGIDevice::IID) }.map_or_else(|_| String::from("?"), driver_version);
            info!(
                "Graphics: adapter {i}: {name} (vendor {:04x}, device {:04x}), {} MB of its own memory, driver {driver}",
                desc.VendorId,
                desc.DeviceId,
                desc.DedicatedVideoMemory / (1024 * 1024)
            );
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("Graphics: couldn't look at the adapters ({e})");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_driver_version_reads_as_windows_shows_it() {
        // Intel's on the Surface: 27.20.100.8681.
        let packed = (27i64 << 48) | (20 << 32) | (100 << 16) | 8681;
        assert_eq!(super::driver_version(packed), "27.20.100.8681");
    }
}
