//! Hooks for DirectX 11.

use std::ffi::c_void;
use std::mem;
use std::sync::OnceLock;

use imgui::Context;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use tracing::{error, info, trace};
use windows::core::{Error, Interface, Result, HRESULT};
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_NULL, D3D_DRIVER_TYPE_WARP,
    D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDeviceAndSwapChain, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_MODE_DESC, DXGI_MODE_SCALING_UNSPECIFIED,
    DXGI_MODE_SCANLINE_ORDER_UNSPECIFIED, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGISwapChain, DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use super::DummyHwnd;
use crate::mh::MhHook;
use crate::renderer::{D3D11RenderEngine, Pipeline};
use crate::{util, Hooks, ImguiRenderLoop};

type DXGISwapChainPresentType =
    unsafe extern "system" fn(This: IDXGISwapChain, SyncInterval: u32, Flags: u32) -> HRESULT;

struct Trampolines {
    dxgi_swap_chain_present: DXGISwapChainPresentType,
}

static mut TRAMPOLINES: OnceLock<Trampolines> = OnceLock::new();
static mut PIPELINE: OnceCell<Mutex<Pipeline<D3D11RenderEngine>>> = OnceCell::new();
static mut RENDER_LOOP: OnceCell<Box<dyn ImguiRenderLoop + Send + Sync>> = OnceCell::new();

unsafe fn init_pipeline(swap_chain: &IDXGISwapChain) -> Result<Mutex<Pipeline<D3D11RenderEngine>>> {
    let hwnd = util::try_out_param(|v| swap_chain.GetDesc(v)).map(|desc| desc.OutputWindow)?;

    let mut ctx = Context::create();
    let engine = D3D11RenderEngine::new(&swap_chain.GetDevice()?, &mut ctx)?;

    let Some(render_loop) = RENDER_LOOP.take() else {
        error!("Render loop not yet initialized");
        return Err(Error::from_hresult(HRESULT(-1)));
    };

    let pipeline = Pipeline::new(hwnd, ctx, engine, render_loop).map_err(|(e, render_loop)| {
        RENDER_LOOP.get_or_init(move || render_loop);
        e
    })?;

    Ok(Mutex::new(pipeline))
}

fn render(swap_chain: &IDXGISwapChain) -> Result<()> {
    unsafe {
        let pipeline = PIPELINE.get_or_try_init(|| init_pipeline(swap_chain))?;

        let Some(mut pipeline) = pipeline.try_lock() else {
            error!("Could not lock pipeline");
            return Err(Error::from_hresult(HRESULT(-1)));
        };

        pipeline.prepare_render()?;

        let target: ID3D11Texture2D = swap_chain.GetBuffer(0)?;

        pipeline.render(target)?;
    }
    Ok(())
}

unsafe extern "system" fn dxgi_swap_chain_present_impl(
    swap_chain: IDXGISwapChain,
    sync_interval: u32,
    flags: u32,
) -> HRESULT {
    let Trampolines { dxgi_swap_chain_present } =
        TRAMPOLINES.get().expect("DirectX 11 trampolines uninitialized");

    if let Err(e) = render(&swap_chain) {
        error!("Render error: {e:?}");
    }

    trace!("Call IDXGISwapChain::Present trampoline");
    dxgi_swap_chain_present(swap_chain, sync_interval, flags)
}

/// 5th Echelon patch: the ways a dummy device is asked for, in turn. Upstream asks for the
/// first only, which some systems refuse (mpw, 2026-10-18: `0x80004005`; JeezyWeezy,
/// 2026-10-25: `0x80070057`, "the parameter is incorrect"), and the overlay was gone for the
/// run. The software rasteriser (WARP) and the real adapter make a swap chain of the same
/// DXGI, so its `Present` is the one the game's calls go through.
const DUMMY_DEVICES: [(D3D_DRIVER_TYPE, &[D3D_FEATURE_LEVEL], &str); 4] = [
    (D3D_DRIVER_TYPE_NULL, &[D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_11_0], "null"),
    (D3D_DRIVER_TYPE_NULL, &[D3D_FEATURE_LEVEL_11_0], "null, 11.0 only"),
    (D3D_DRIVER_TYPE_WARP, &[D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_0], "WARP"),
    (D3D_DRIVER_TYPE_HARDWARE, &[D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_0], "hardware"),
];

/// `IDXGISwapChain::Present`'s address, from a dummy device and swap chain; an error when
/// the system won't make one in any of [`DUMMY_DEVICES`]' ways, which the caller can live with.
fn try_target_addrs() -> Result<DXGISwapChainPresentType> {
    let dummy_hwnd = DummyHwnd::new();
    let mut first_error = None;
    for (n, (driver, levels, name)) in DUMMY_DEVICES.iter().enumerate() {
        match dummy_swap_chain(&dummy_hwnd, *driver, levels) {
            Ok(swap_chain) => {
                if n > 0 {
                    info!("DirectX 11 dummy device: {name} (the null device was refused: {first_error:?})");
                }
                let present_ptr: DXGISwapChainPresentType = unsafe {
                    mem::transmute::<
                        unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT,
                        DXGISwapChainPresentType,
                    >(swap_chain.vtable().Present)
                };
                return Ok(present_ptr);
            },
            Err(e) => {
                first_error.get_or_insert(e);
            },
        }
    }
    Err(first_error.unwrap_or_else(|| Error::from_hresult(HRESULT(-1))))
}

fn dummy_swap_chain(
    dummy_hwnd: &DummyHwnd,
    driver: D3D_DRIVER_TYPE,
    levels: &[D3D_FEATURE_LEVEL],
) -> Result<IDXGISwapChain> {
    let mut p_device: Option<ID3D11Device> = None;
    let mut p_context: Option<ID3D11DeviceContext> = None;
    let mut p_swap_chain: Option<IDXGISwapChain> = None;

    unsafe {
        D3D11CreateDeviceAndSwapChain(
            None,
            driver,
            None,
            D3D11_CREATE_DEVICE_FLAG(0),
            Some(levels),
            D3D11_SDK_VERSION,
            Some(&DXGI_SWAP_CHAIN_DESC {
                BufferDesc: DXGI_MODE_DESC {
                    // 5th Echelon patch: a size of its own, not the dummy window's, which a
                    // driver may take as nothing to make.
                    Width: 64,
                    Height: 64,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    ScanlineOrdering: DXGI_MODE_SCANLINE_ORDER_UNSPECIFIED,
                    Scaling: DXGI_MODE_SCALING_UNSPECIFIED,
                    ..Default::default()
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 1,
                OutputWindow: dummy_hwnd.hwnd(),
                Windowed: BOOL(1),
                SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, ..Default::default() },
                ..Default::default()
            }),
            Some(&mut p_swap_chain),
            Some(&mut p_device),
            None,
            Some(&mut p_context),
        )?;
    }

    p_swap_chain.ok_or_else(|| Error::from_hresult(HRESULT(-1)))
}

/// 5th Echelon patch: the `Present` [`check`] found, so [`ImguiDx11Hooks::new`] doesn't make the
/// dummy device a second time (on the systems that need a fallback, making one is flaky, and a
/// failure there panics, which ends the game).
static FOUND_PRESENT: OnceLock<usize> = OnceLock::new();

fn get_target_addrs() -> DXGISwapChainPresentType {
    match FOUND_PRESENT.get() {
        Some(&found) => unsafe { mem::transmute::<usize, DXGISwapChainPresentType>(found) },
        None => try_target_addrs().expect("D3D11CreateDeviceAndSwapChain failed"),
    }
}

/// Whether DirectX 11 can be hooked here: the dummy device and swap chain it needs can be
/// made. Check first; [`ImguiDx11Hooks::new`] then hooks the `Present` found here.
pub fn check() -> Result<()> {
    let present = try_target_addrs()?;
    let _ = FOUND_PRESENT.set(present as usize);
    Ok(())
}

/// Hooks for DirectX 11.
pub struct ImguiDx11Hooks([MhHook; 1]);

impl ImguiDx11Hooks {
    /// Construct a set of [`MhHook`]s that will render UI via the
    /// provided [`ImguiRenderLoop`].
    ///
    /// The following functions are hooked:
    /// - `IDXGISwapChain::Present`
    ///
    /// # Safety
    ///
    /// yolo
    pub unsafe fn new<T>(t: T) -> Self
    where
        T: ImguiRenderLoop + Send + Sync + 'static,
    {
        let dxgi_swap_chain_present_addr = get_target_addrs();

        trace!("IDXGISwapChain::Present = {:p}", dxgi_swap_chain_present_addr as *const c_void);
        let hook_present = MhHook::new(
            dxgi_swap_chain_present_addr as *mut _,
            dxgi_swap_chain_present_impl as *mut _,
        )
        .expect("couldn't create IDXGISwapChain::Present hook");

        RENDER_LOOP.get_or_init(|| Box::new(t));
        TRAMPOLINES.get_or_init(|| Trampolines {
            dxgi_swap_chain_present: mem::transmute::<*mut c_void, DXGISwapChainPresentType>(
                hook_present.trampoline(),
            ),
        });

        Self([hook_present])
    }
}

impl Hooks for ImguiDx11Hooks {
    fn from_render_loop<T>(t: T) -> Box<Self>
    where
        Self: Sized,
        T: ImguiRenderLoop + Send + Sync + 'static,
    {
        Box::new(unsafe { Self::new(t) })
    }

    fn hooks(&self) -> &[MhHook] {
        &self.0
    }

    unsafe fn unhook(&mut self) {
        TRAMPOLINES.take();
        PIPELINE.take().map(|p| p.into_inner().take());
        RENDER_LOOP.take(); // should already be null
    }
}
