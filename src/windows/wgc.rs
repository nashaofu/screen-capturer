use std::sync::mpsc::channel;

use image::RgbaImage;
use scopeguard::defer;
use windows::{
    core::{factory, IInspectable, Interface},
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem},
        DirectX::DirectXPixelFormat,
    },
    Win32::{
        Foundation::HWND,
        Graphics::{Direct3D11::D3D11_CREATE_DEVICE_BGRA_SUPPORT, Gdi::HMONITOR},
        System::WinRT::{
            Direct3D11::IDirect3DDxgiInterfaceAccess,
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
    },
};

use crate::{video_recorder::Frame, XCapError, XCapResult};

use super::{
    impl_video_recorder::texture_to_frame,
    utils::{create_d3d11_device, create_direct3d_device},
};

#[allow(unused)]
pub fn wgc_capture(item: GraphicsCaptureItem) -> XCapResult<Frame> {
    unsafe {
        let d3d_device = create_d3d11_device(D3D11_CREATE_DEVICE_BGRA_SUPPORT)?;
        let d3d_context = d3d_device.GetImmediateContext()?;
        let direct_3d_device = create_direct3d_device(&d3d_device)?;
        defer!({
            direct_3d_device.Close();
        });

        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct_3d_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            1,
            item.Size()?,
        )?;
        defer!({
            frame_pool.Close();
        });

        let session = frame_pool.CreateCaptureSession(&item)?;
        defer!({
            session.Close();
        });

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
