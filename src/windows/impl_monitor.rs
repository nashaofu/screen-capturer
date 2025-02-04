use std::{mem, ptr};

use image::RgbaImage;
use scopeguard::guard;
use windows::{
    core::{s, w, HRESULT, PCWSTR},
    Win32::{
        Foundation::{BOOL, LPARAM, POINT, RECT, TRUE},
        Graphics::Gdi::{
            CreateDCW, DeleteDC, EnumDisplayMonitors, EnumDisplaySettingsW, GetDeviceCaps,
            GetMonitorInfoW, MonitorFromPoint, DESKTOPHORZRES, DEVMODEW, DMDO_180, DMDO_270,
            DMDO_90, DMDO_DEFAULT, ENUM_CURRENT_SETTINGS, HDC, HMONITOR, HORZRES, MONITORINFO,
            MONITORINFOEXW, MONITOR_DEFAULTTONULL,
        },
        System::{LibraryLoader::GetProcAddress, Threading::GetCurrentProcess},
        UI::WindowsAndMessaging::MONITORINFOF_PRIMARY,
    },
};

use crate::error::{XCapError, XCapResult};

use super::{
    capture::capture_monitor,
    impl_video_recorder::ImplVideoRecorder,
    utils::{get_monitor_name, get_process_is_dpi_awareness, load_library},
};

// A 函数与 W 函数区别
// https://learn.microsoft.com/zh-cn/windows/win32/learnwin32/working-with-strings

#[derive(Debug, Clone)]
pub(crate) struct ImplMonitor {
    #[allow(unused)]
    pub h_monitor: HMONITOR,
    #[allow(unused)]
    pub monitor_info_ex_w: MONITORINFOEXW,
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub rotation: f32,
    pub scale_factor: f32,
    pub frequency: f32,
    pub is_primary: bool,
}

extern "system" fn monitor_enum_proc(
    h_monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    state: LPARAM,
) -> BOOL {
    unsafe {
        let state = Box::leak(Box::from_raw(state.0 as *mut Vec<HMONITOR>));
        state.push(h_monitor);

        TRUE
    }
}

fn get_dev_mode_w(monitor_info_exw: &MONITORINFOEXW) -> XCapResult<DEVMODEW> {
    let sz_device = monitor_info_exw.szDevice.as_ptr();
    let mut dev_mode_w = DEVMODEW {
        dmSize: mem::size_of::<DEVMODEW>() as u16,
        ..DEVMODEW::default()
    };

    unsafe {
        EnumDisplaySettingsW(PCWSTR(sz_device), ENUM_CURRENT_SETTINGS, &mut dev_mode_w).ok()?;
    };

    Ok(dev_mode_w)
}

// 定义 GetDpiForMonitor 函数的类型
type GetDpiForMonitor = unsafe extern "system" fn(
    h_monitor: HMONITOR,
    dpi_type: u32,
    dpi_x: *mut u32,
    dpi_y: *mut u32,
) -> HRESULT;

fn get_hi_dpi_scale_factor(h_monitor: HMONITOR) -> XCapResult<f32> {
    unsafe {
        let current_process_is_dpi_awareness: bool =
            get_process_is_dpi_awareness(GetCurrentProcess())?;

        // 当前进程不感知 DPI，则回退到 GetDeviceCaps 获取 DPI
        if !current_process_is_dpi_awareness {
            return Err(XCapError::new("Process not DPI aware"));
        }

        let scope_guard_hmodule = load_library(w!("Shcore.dll"))?;

        let get_dpi_for_monitor_proc_address =
            GetProcAddress(*scope_guard_hmodule, s!("GetDpiForMonitor"))
                .ok_or(XCapError::new("GetProcAddress GetDpiForMonitor failed"))?;

        let get_dpi_for_monitor: GetDpiForMonitor =
            mem::transmute(get_dpi_for_monitor_proc_address);

        let mut dpi_x = 0;
        let mut dpi_y = 0;

        // https://learn.microsoft.com/zh-cn/windows/win32/api/shellscalingapi/ne-shellscalingapi-monitor_dpi_type
        get_dpi_for_monitor(h_monitor, 0, &mut dpi_x, &mut dpi_y).ok()?;

        Ok(dpi_x as f32 / 96.0)
    }
}

fn get_scale_factor(h_monitor: HMONITOR, monitor_info_ex_w: MONITORINFOEXW) -> XCapResult<f32> {
    let scale_factor = get_hi_dpi_scale_factor(h_monitor).unwrap_or_else(|err| {
        log::info!("{}", err);
        // https://learn.microsoft.com/zh-cn/windows/win32/api/wingdi/nf-wingdi-getdevicecaps
        unsafe {
            let scope_guard_hdc = guard(
                CreateDCW(
                    PCWSTR(monitor_info_ex_w.szDevice.as_ptr()),
                    PCWSTR(monitor_info_ex_w.szDevice.as_ptr()),
                    PCWSTR(ptr::null()),
                    None,
                ),
                |val| {
                    if !DeleteDC(val).as_bool() {
                        log::error!("DeleteDC {:?} failed", val)
                    }
                },
            );

            let physical_width = GetDeviceCaps(Some(*scope_guard_hdc), DESKTOPHORZRES);
            let logical_width = GetDeviceCaps(Some(*scope_guard_hdc), HORZRES);

            physical_width as f32 / logical_width as f32
        }
    });

    Ok(scale_factor)
}

impl ImplMonitor {
    pub fn new(h_monitor: HMONITOR) -> XCapResult<ImplMonitor> {
        let mut monitor_info_ex_w = MONITORINFOEXW::default();
        monitor_info_ex_w.monitorInfo.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;
        let monitor_info_ex_w_ptr =
            &mut monitor_info_ex_w as *mut MONITORINFOEXW as *mut MONITORINFO;

        // https://learn.microsoft.com/zh-cn/windows/win32/api/winuser/nf-winuser-getmonitorinfoa
        unsafe { GetMonitorInfoW(h_monitor, monitor_info_ex_w_ptr).ok()? };

        let name = get_monitor_name(monitor_info_ex_w)
            .unwrap_or(format!("Unknown Monitor {}", h_monitor.0 as u32));

        let dev_mode_w = get_dev_mode_w(&monitor_info_ex_w)?;

        let dm_position = unsafe { dev_mode_w.Anonymous1.Anonymous2.dmPosition };
        let dm_pels_width = dev_mode_w.dmPelsWidth;
        let dm_pels_height = dev_mode_w.dmPelsHeight;

        let dm_display_orientation =
            unsafe { dev_mode_w.Anonymous1.Anonymous2.dmDisplayOrientation };
        let rotation = match dm_display_orientation {
            DMDO_90 => 90.0,
            DMDO_180 => 180.0,
            DMDO_270 => 270.0,
            DMDO_DEFAULT => 0.0,
            _ => 0.0,
        };

        let scale_factor = get_scale_factor(h_monitor, monitor_info_ex_w)?;

        Ok(ImplMonitor {
            h_monitor,
            monitor_info_ex_w,
            id: h_monitor.0 as u32,
            name,
            x: dm_position.x,
            y: dm_position.y,
            width: dm_pels_width,
            height: dm_pels_height,
            rotation,
            scale_factor,
            frequency: dev_mode_w.dmDisplayFrequency as f32,
            is_primary: monitor_info_ex_w.monitorInfo.dwFlags == MONITORINFOF_PRIMARY,
        })
    }

    pub fn all() -> XCapResult<Vec<ImplMonitor>> {
        let hmonitors_mut_ptr: *mut Vec<HMONITOR> = Box::into_raw(Box::default());

        let h_monitors = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(monitor_enum_proc),
                LPARAM(hmonitors_mut_ptr as isize),
            )
            .ok()?;
            Box::from_raw(hmonitors_mut_ptr)
        };

        let mut impl_monitors = Vec::with_capacity(h_monitors.len());

        for &h_monitor in h_monitors.iter() {
            if let Ok(impl_monitor) = ImplMonitor::new(h_monitor) {
                impl_monitors.push(impl_monitor);
            } else {
                log::error!("ImplMonitor::new({:?}) failed", h_monitor);
            }
        }

        Ok(impl_monitors)
    }

    pub fn from_point(x: i32, y: i32) -> XCapResult<ImplMonitor> {
        let point = POINT { x, y };
        let h_monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONULL) };

        if h_monitor.is_invalid() {
            return Err(XCapError::new("Not found monitor"));
        }

        ImplMonitor::new(h_monitor)
    }
}

impl ImplMonitor {
    pub fn capture_image(&self) -> XCapResult<RgbaImage> {
        capture_monitor(self.x, self.y, self.width as i32, self.height as i32)
    }

    pub fn video_recorder(&self) -> XCapResult<ImplVideoRecorder> {
        ImplVideoRecorder::new(self.h_monitor)
    }
}
