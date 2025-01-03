use std::{slice, sync::Arc};

use windows::{
    core::Interface,
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource,
                ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                D3D11_CREATE_DEVICE_SINGLETHREADED, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
                D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
            },
            Dxgi::{
                IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
                DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
            },
            Gdi::HMONITOR,
        },
    },
};

use crate::{
    video_recorder::{Frame, RecorderWaker},
    XCapError, XCapResult,
};

use super::utils::{bgra_to_rgba, create_d3d11_device};

pub fn texture_to_frame(
    d3d_device: &ID3D11Device,
    d3d_context: &ID3D11DeviceContext,
    source_texture: ID3D11Texture2D,
) -> XCapResult<Frame> {
    unsafe {
        let mut source_desc = D3D11_TEXTURE2D_DESC::default();
        source_texture.GetDesc(&mut source_desc);
        source_desc.BindFlags = 0;
        source_desc.MiscFlags = 0;
        source_desc.Usage = D3D11_USAGE_STAGING;
        source_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;

        let copy_texture = {
            let mut texture = None;
            d3d_device.CreateTexture2D(&source_desc, None, Some(&mut texture))?;
            texture.ok_or(XCapError::new("CreateTexture2D failed"))?
        };

        d3d_context.CopyResource(Some(&copy_texture.cast()?), Some(&source_texture.cast()?));

        let resource: ID3D11Resource = copy_texture.cast()?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        d3d_context.Map(
            Some(&resource.clone()),
            0,
            D3D11_MAP_READ,
            0,
            Some(&mut mapped),
        )?;

        // Get a slice of bytes
        let bgra = slice::from_raw_parts(mapped.pData.cast(), mapped.DepthPitch as usize);

        let bytes_per_pixel = 4;
        let mut bits =
            vec![0u8; (source_desc.Width * source_desc.Height * bytes_per_pixel) as usize];
        for row in 0..source_desc.Height {
            let data_begin = (row * (source_desc.Width * bytes_per_pixel)) as usize;
            let data_end = ((row + 1) * (source_desc.Width * bytes_per_pixel)) as usize;
            let slice_begin = (row * mapped.RowPitch) as usize;
            let slice_end = slice_begin + (source_desc.Width * bytes_per_pixel) as usize;
            bits[data_begin..data_end].copy_from_slice(&bgra[slice_begin..slice_end]);
        }

        d3d_context.Unmap(Some(&resource), 0);

        Ok(Frame::new(
            source_desc.Width,
            source_desc.Height,
            bgra_to_rgba(bits.to_owned()),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct ImplVideoRecorder {
    d3d_device: ID3D11Device,
    d3d_context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    recorder_waker: Arc<RecorderWaker>,
}

impl ImplVideoRecorder {
    pub fn new(h_monitor: HMONITOR) -> XCapResult<Self> {
        unsafe {
            let mut d3d_device = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_SINGLETHREADED,
            )?;

            let dxgi_device = d3d_device.cast::<IDXGIDevice>()?;
            let d3d_context = d3d_device.GetImmediateContext()?;

            let adapter = dxgi_device.GetAdapter()?;

            let mut output_index = 0;
            loop {
                let output = adapter.EnumOutputs(output_index)?;
                output_index += 1;
                let output_desc = output.GetDesc()?;

                let output1 = output.cast::<IDXGIOutput1>()?;
                let duplication = output1.DuplicateOutput(&dxgi_device)?;

                if output_desc.Monitor == h_monitor {
                    return Ok(Self {
                        d3d_device,
                        d3d_context,
                        duplication,
                        recorder_waker: Arc::new(RecorderWaker::new()),
                    });
                }
            }
        }
    }

    pub fn on_frame<F>(&self, on_frame: F) -> XCapResult<()>
    where
        F: Fn(Frame) -> XCapResult<()> + Send + 'static,
    {
        let duplication = self.duplication.clone();
        let d3d_device = self.d3d_device.clone();
        let d3d_context = self.d3d_context.clone();
        let recorder_waker = self.recorder_waker.clone();

        loop {
            recorder_waker.wait()?;

            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;

            unsafe {
                if let Err(err) = duplication.AcquireNextFrame(200, &mut frame_info, &mut resource)
                {
                    // 尝试释放当前帧，不然不能获取到下一帧数据
                    let _ = duplication.ReleaseFrame();
                    if err.code() != DXGI_ERROR_WAIT_TIMEOUT {
                        break Err::<(), XCapError>(XCapError::new("DXGI_ERROR_UNSUPPORTED"));
                    }
                } else {
                    // 如何确定 AcquireNextFrame 执行成功
                    if frame_info.LastPresentTime != 0 {
                        let resource = resource.ok_or(XCapError::new("AcquireNextFrame failed"))?;
                        let source_texture = resource.cast::<ID3D11Texture2D>()?;
                        let frame = texture_to_frame(&d3d_device, &d3d_context, source_texture)?;

                        on_frame(frame)?;
                    }

                    // 最后释放帧，不然获取不到当前帧的数据
                    duplication.ReleaseFrame()?;
                }
            }
        }
    }
    pub fn start(&self) -> XCapResult<()> {
        self.recorder_waker.wake()?;

        Ok(())
    }
    pub fn stop(&self) -> XCapResult<()> {
        self.recorder_waker.sleep()?;

        Ok(())
    }
}
