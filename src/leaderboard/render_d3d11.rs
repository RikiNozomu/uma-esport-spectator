//! Draws the leaderboard into the game's back buffer, from Hachimi's present
//! callback (render thread, just before IDXGISwapChain::Present).
//!
//! Two rules keep the game safe:
//! - No reference to the back buffer is kept between frames. The plugin is
//!   not told about IDXGISwapChain::ResizeBuffers, and a view held across it
//!   would make the game's resize fail. The view is created and dropped
//!   inside each call instead.
//! - Every piece of pipeline state the renderer touches is saved before and
//!   restored after drawing.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use windows::core::Interface;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;

use super::Board;
use crate::api;
use crate::seek::{self, draw::SeekUi};

/// The leaderboard and the seek bar each have their own egui context, so
/// each needs its own renderer (egui texture ids are per context).
struct State {
    device: ID3D11Device,
    renderer: egui_directx11::Renderer,
    board: Board,
    seek_renderer: egui_directx11::Renderer,
    seek_ui: SeekUi,
}

// Only touched from the render thread, behind the mutex.
unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// Set after a failure, so a broken renderer does not spam the log every frame.
static BROKEN: AtomicBool = AtomicBool::new(false);

/// Clears a previous failure (when the user turns the overlay back on).
pub fn retry() {
    BROKEN.store(false, Ordering::Relaxed);
}

/// Registered with `hachimi_register_present_callback`.
pub unsafe extern "C" fn on_present(swap_chain: *mut c_void, _userdata: *mut c_void) {
    if swap_chain.is_null() || BROKEN.load(Ordering::Relaxed) {
        return;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // Cheap exit when there is nothing to draw: no D3D calls at all.
        let frame = super::next_frame();
        let seek_frame = seek::next_frame();
        if frame.is_none() && seek_frame.is_none() {
            return Ok(());
        }
        draw(swap_chain, frame.as_ref(), seek_frame.as_ref())
    }));
    let error = match result {
        Ok(Ok(())) => return,
        Ok(Err(e)) => e,
        Err(_) => "panicked".to_owned(),
    };
    BROKEN.store(true, Ordering::Relaxed);
    seek::set_geometry(None, false);
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let api = api::get();
    api.error(&format!("overlay (leaderboard, seek bar) stopped: {error}"));
    api.notify("Overlay stopped after an error, see hachimi.log");
}

unsafe fn draw(swap_chain: *mut c_void, frame: Option<&super::Frame>, seek_frame: Option<&seek::Frame>) -> Result<(), String> {
    // Borrowed: the swap chain belongs to the game, so no Release on drop.
    let swap_chain = ManuallyDrop::new(IDXGISwapChain::from_raw(swap_chain));
    let device: ID3D11Device = swap_chain.GetDevice().map_err(|e| format!("GetDevice: {e}"))?;
    let back_buffer: ID3D11Texture2D = swap_chain.GetBuffer(0).map_err(|e| format!("GetBuffer: {e}"))?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    back_buffer.GetDesc(&mut desc);
    if desc.Width == 0 || desc.Height == 0 {
        return Ok(());
    }
    let render_target = create_render_target(&device, &back_buffer)?;
    let context = device.GetImmediateContext().map_err(|e| format!("GetImmediateContext: {e}"))?;

    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    // A new device (display change, device lost) needs a new renderer and
    // new textures; egui's are recreated with the board.
    if guard.as_ref().is_none_or(|s| s.device != device) {
        let renderer = egui_directx11::Renderer::new(&device).map_err(|e| format!("renderer init: {e}"))?;
        let seek_renderer = egui_directx11::Renderer::new(&device).map_err(|e| format!("renderer init: {e}"))?;
        *guard = Some(State {
            device: device.clone(),
            renderer,
            board: Board::new(super::image_dir()),
            seek_renderer,
            seek_ui: SeekUi::default(),
        });
    }
    let state = guard.as_mut().expect("just set");

    let board_output = frame.map(|f| state.board.frame(desc.Width, desc.Height, f, super::view()));
    let seek_output = match seek_frame {
        Some(f) => {
            let (output, geom) = state.seek_ui.frame(desc.Width, desc.Height, f);
            // Clicks count once the bar is clearly visible.
            seek::set_geometry(Some((geom, (desc.Width, desc.Height))), f.alpha > 0.3);
            Some(output)
        }
        None => None,
    };

    let mut saved = SavedState::save(&context);
    context.GSSetShader(None, None);
    context.HSSetShader(None, None);
    context.DSSetShader(None, None);
    let mut result = Ok(());
    if let Some(output) = board_output {
        let (renderer_output, _, _) = egui_directx11::split_output(output);
        result = state.renderer.render(&context, &render_target, &state.board.ctx, renderer_output);
    }
    if let (Ok(()), Some(output)) = (&result, seek_output) {
        let (renderer_output, _, _) = egui_directx11::split_output(output);
        result = state.seek_renderer.render(&context, &render_target, &state.seek_ui.ctx, renderer_output);
    }
    saved.restore(&context);
    result.map_err(|e| format!("render: {e}"))
}

/// Non-sRGB view of the back buffer, as egui-directx11 requires.
unsafe fn create_render_target(device: &ID3D11Device, back_buffer: &ID3D11Texture2D) -> Result<ID3D11RenderTargetView, String> {
    let mut view = None;
    let desc = D3D11_RENDER_TARGET_VIEW_DESC {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
        ..Default::default()
    };
    // Same as Hachimi's own GUI; fall back to the buffer's format.
    if device.CreateRenderTargetView(back_buffer, Some(&desc), Some(&mut view)).is_err() {
        device
            .CreateRenderTargetView(back_buffer, None, Some(&mut view))
            .map_err(|e| format!("CreateRenderTargetView: {e}"))?;
    }
    view.ok_or_else(|| "CreateRenderTargetView returned nothing".to_owned())
}

const MAX_VIEWPORTS: usize = D3D11_VIEWPORT_AND_SCISSORRECT_OBJECT_COUNT_PER_PIPELINE as usize;
const MAX_TARGETS: usize = D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as usize;

/// Everything egui-directx11 changes (see `Renderer::render`'s docs), plus
/// the geometry/hull/domain shaders we clear.
struct SavedState {
    render_targets: [Option<ID3D11RenderTargetView>; MAX_TARGETS],
    depth_stencil: Option<ID3D11DepthStencilView>,
    blend: Option<ID3D11BlendState>,
    blend_factor: [f32; 4],
    sample_mask: u32,
    viewports: [D3D11_VIEWPORT; MAX_VIEWPORTS],
    viewport_count: u32,
    scissors: [RECT; MAX_VIEWPORTS],
    scissor_count: u32,
    rasterizer: Option<ID3D11RasterizerState>,
    ps: Option<ID3D11PixelShader>,
    ps_srv: [Option<ID3D11ShaderResourceView>; 1],
    ps_sampler: [Option<ID3D11SamplerState>; 1],
    vs: Option<ID3D11VertexShader>,
    gs: Option<ID3D11GeometryShader>,
    hs: Option<ID3D11HullShader>,
    ds: Option<ID3D11DomainShader>,
    input_layout: Option<ID3D11InputLayout>,
    topology: D3D_PRIMITIVE_TOPOLOGY,
    vertex_buffer: [Option<ID3D11Buffer>; 1],
    vertex_stride: [u32; 1],
    vertex_offset: [u32; 1],
    index_buffer: Option<ID3D11Buffer>,
    index_format: DXGI_FORMAT,
    index_offset: u32,
}

impl SavedState {
    unsafe fn save(c: &ID3D11DeviceContext) -> SavedState {
        let mut s = SavedState {
            render_targets: Default::default(),
            depth_stencil: None,
            blend: None,
            blend_factor: [0.0; 4],
            sample_mask: 0,
            viewports: [D3D11_VIEWPORT::default(); MAX_VIEWPORTS],
            viewport_count: MAX_VIEWPORTS as u32,
            scissors: [RECT::default(); MAX_VIEWPORTS],
            scissor_count: MAX_VIEWPORTS as u32,
            rasterizer: None,
            ps: None,
            ps_srv: Default::default(),
            ps_sampler: Default::default(),
            vs: None,
            gs: None,
            hs: None,
            ds: None,
            input_layout: None,
            topology: D3D_PRIMITIVE_TOPOLOGY::default(),
            vertex_buffer: Default::default(),
            vertex_stride: [0],
            vertex_offset: [0],
            index_buffer: None,
            index_format: DXGI_FORMAT::default(),
            index_offset: 0,
        };
        c.OMGetRenderTargets(Some(&mut s.render_targets), Some(&mut s.depth_stencil));
        c.OMGetBlendState(Some(&mut s.blend), Some(&mut s.blend_factor), Some(&mut s.sample_mask));
        c.RSGetViewports(&mut s.viewport_count, Some(s.viewports.as_mut_ptr()));
        c.RSGetScissorRects(&mut s.scissor_count, Some(s.scissors.as_mut_ptr()));
        s.rasterizer = c.RSGetState().ok();
        c.PSGetShader(&mut s.ps, None, None);
        c.PSGetShaderResources(0, Some(&mut s.ps_srv));
        c.PSGetSamplers(0, Some(&mut s.ps_sampler));
        c.VSGetShader(&mut s.vs, None, None);
        c.GSGetShader(&mut s.gs, None, None);
        c.HSGetShader(&mut s.hs, None, None);
        c.DSGetShader(&mut s.ds, None, None);
        s.input_layout = c.IAGetInputLayout().ok();
        s.topology = c.IAGetPrimitiveTopology();
        c.IAGetVertexBuffers(
            0,
            1,
            Some(s.vertex_buffer.as_mut_ptr()),
            Some(s.vertex_stride.as_mut_ptr()),
            Some(s.vertex_offset.as_mut_ptr()),
        );
        c.IAGetIndexBuffer(Some(&mut s.index_buffer), Some(&mut s.index_format), Some(&mut s.index_offset));
        s
    }

    unsafe fn restore(&mut self, c: &ID3D11DeviceContext) {
        c.OMSetRenderTargets(Some(&self.render_targets), self.depth_stencil.as_ref());
        c.OMSetBlendState(self.blend.as_ref(), Some(&self.blend_factor), self.sample_mask);
        c.RSSetViewports(Some(&self.viewports[..self.viewport_count as usize]));
        c.RSSetScissorRects(Some(&self.scissors[..self.scissor_count as usize]));
        c.RSSetState(self.rasterizer.as_ref());
        c.PSSetShader(self.ps.as_ref(), None);
        c.PSSetShaderResources(0, Some(&self.ps_srv));
        c.PSSetSamplers(0, Some(&self.ps_sampler));
        c.VSSetShader(self.vs.as_ref(), None);
        c.GSSetShader(self.gs.as_ref(), None);
        c.HSSetShader(self.hs.as_ref(), None);
        c.DSSetShader(self.ds.as_ref(), None);
        c.IASetInputLayout(self.input_layout.as_ref());
        c.IASetPrimitiveTopology(self.topology);
        c.IASetVertexBuffers(
            0,
            1,
            Some(self.vertex_buffer.as_ptr()),
            Some(self.vertex_stride.as_ptr()),
            Some(self.vertex_offset.as_ptr()),
        );
        c.IASetIndexBuffer(self.index_buffer.as_ref(), self.index_format, self.index_offset);
    }
}
