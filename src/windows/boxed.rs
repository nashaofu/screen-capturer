use std::{ops::Deref, ptr};
use windows::{
    core::PCWSTR,
    Win32::Graphics::Gdi::{CreateDCW, DeleteDC, DeleteObject, GetWindowDC, HBITMAP, HDC},
};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC};
use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
use crate::{XCapError, XCapResult};

use super::{impl_monitor::ImplMonitor, impl_window::ImplWindow};

pub(crate) struct BoxHDC(HDC);

impl Deref for BoxHDC {
    type Target = HDC;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for BoxHDC {
    fn drop(&mut self) {
        unsafe {
            DeleteDC(self.0);
        };
    }
}

impl BoxHDC {
    pub fn new(hdc: HDC) -> Self {
        BoxHDC(hdc)
    }
}

impl From<&[u16; 32]> for BoxHDC {
    fn from(sz_device: &[u16; 32]) -> Self {
        let sz_device_ptr = sz_device.as_ptr();

        let hdc = unsafe {
            CreateDCW(
                PCWSTR(sz_device_ptr),
                PCWSTR(sz_device_ptr),
                PCWSTR(ptr::null()),
                None,
            )
        };

        BoxHDC::new(hdc)
    }
}

impl From<&ImplMonitor> for BoxHDC {
    fn from(impl_monitor: &ImplMonitor) -> Self {
        BoxHDC::from(&impl_monitor.monitor_info_ex_w.szDevice)
    }
}

impl From<&ImplWindow> for BoxHDC {
    fn from(impl_window: &ImplWindow) -> Self {
        let hdc = unsafe { GetWindowDC(impl_window.hwnd) };

        BoxHDC::new(hdc)
    }
}

pub(crate) struct BoxHBITMAP(HBITMAP);

impl Deref for BoxHBITMAP {
    type Target = HBITMAP;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for BoxHBITMAP {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        };
    }
}

impl BoxHBITMAP {
    pub fn new(h_bitmap: HBITMAP) -> Self {
        BoxHBITMAP(h_bitmap)
    }
}

pub(crate) struct BoxMonitorHDC {
    hwnd: HWND,
    hdc: HDC,
}
impl Deref for BoxMonitorHDC {
    type Target = HDC;
    fn deref(&self) -> &Self::Target {
        &self.hdc
    }
}

impl Drop for BoxMonitorHDC {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(self.hwnd, self.hdc);
        };
    }
}

impl BoxMonitorHDC {
    pub unsafe  fn new() -> XCapResult<BoxMonitorHDC> {
        let hwnd = GetDesktopWindow();
        let hdc = GetDC(hwnd);
        if hdc.is_invalid() {
            return Err(XCapError::new("GetDC is failed"))
        }
        Ok(BoxMonitorHDC{
            hwnd,
            hdc,
        })
    }
}