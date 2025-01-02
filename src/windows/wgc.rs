use image::RgbaImage;
use std::sync::mpsc::channel;
use windows::{
    core::{factory, IInspectable, Interface},
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                D3D11_CREATE_DEVICE_SINGLETHREADED, D3D11_SDK_VERSION,
            },
            Dxgi::IDXGIDevice,
            Gdi::HMONITOR,
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
    },
};

use crate::{video_recorder::Frame, XCapError, XCapResult};

use super::impl_video_recorder::texture_to_frame;

#[allow(unused)]
pub fn wgc_capture(item: GraphicsCaptureItem) -> XCapResult<Frame> {
    unsafe {
        let mut d3d_device = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_SINGLETHREADED,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d_device),
            None,
            None,
        )?;
        let d3d_device = d3d_device.ok_or(XCapError::new("Call D3D11CreateDevice failed"))?;
        let d3d_context = d3d_device.GetImmediateContext()?;
        let dxgi_device = d3d_device.cast::<IDXGIDevice>()?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)?;
        let direct_3d_device = inspectable.cast::<IDirect3DDevice>()?;

        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct_3d_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            1,
            item.Size()?,
        )?;

        let session = frame_pool.CreateCaptureSession(&item)?;

        let (sender, receiver) = channel();
        frame_pool.FrameArrived(
            &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new({
                move |frame_pool, _| {
                    let frame_pool = frame_pool.as_ref().unwrap();
                    let frame = frame_pool.TryGetNextFrame()?;
                    sender.send(frame).unwrap();
                    Ok(())
                }
            }),
        )?;
        session.SetIsBorderRequired(false)?;
        session.StartCapture()?;

        let frame = receiver.recv().unwrap();

        session.Close()?;
        direct_3d_device.Close()?;

        let surface = frame.Surface()?;
        let access = surface.cast::<IDirect3DDxgiInterfaceAccess>()?;
        let source_texture = access.GetInterface()?;

        let frame = texture_to_frame(&d3d_device, &d3d_context, source_texture)?;

        Ok(frame)
    }
}

#[allow(unused)]
pub fn wgc_capture_monitor(hmonitor: HMONITOR) -> XCapResult<RgbaImage> {
    unsafe {
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item = interop.CreateForMonitor(hmonitor)?;

        let frame = wgc_capture(item)?;

        RgbaImage::from_raw(frame.width, frame.height, frame.raw)
            .ok_or(XCapError::new("RgbaImage::from_raw failed"))
    }
}

#[allow(unused)]
pub fn wgc_capture_window(hwnd: HWND) -> XCapResult<RgbaImage> {
    unsafe {
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item = interop.CreateForWindow(hwnd)?;
        let frame = wgc_capture(item)?;

        RgbaImage::from_raw(frame.width, frame.height, frame.raw)
            .ok_or(XCapError::new("RgbaImage::from_raw failed"))
    }
}
