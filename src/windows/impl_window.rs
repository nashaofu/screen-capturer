use core::slice;
use std::{ffi::c_void, mem, ptr};

use image::RgbaImage;
use widestring::U16CString;
use windows::{
    core::{HSTRING, PCWSTR},
    Win32::{
        Foundation::{GetLastError, BOOL, HANDLE, HWND, LPARAM, MAX_PATH, RECT, TRUE},
        Graphics::{
            Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS},
            Gdi::{IsRectEmpty, MonitorFromWindow, MONITOR_DEFAULTTONEAREST},
        },
        Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
        System::{
            ProcessStatus::{GetModuleBaseNameW, GetModuleFileNameExW},
            Threading::{
                GetCurrentProcess, GetCurrentProcessId, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW,
            GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
            IsWindowVisible, IsZoomed, GWL_EXSTYLE, WINDOW_EX_STYLE, WS_EX_TOOLWINDOW,
        },
    },
};

use crate::error::XCapResult;

use super::{
    capture::{capture_window,capture_window_region},
    impl_monitor::ImplMonitor,
    utils::{get_process_is_dpi_awareness, get_window_info, open_process},
};

#[derive(Debug, Clone)]
pub(crate) struct ImplWindow {
    pub hwnd: HWND,
}

fn is_window_cloaked(hwnd: HWND) -> bool {
    unsafe {
        let mut cloaked = 0u32;

        let is_dwm_get_window_attribute_fail = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            mem::size_of::<u32>() as u32,
        )
        .is_err();

        if is_dwm_get_window_attribute_fail {
            return false;
        }

        cloaked != 0
    }
}

// https://webrtc.googlesource.com/src.git/+/refs/heads/main/modules/desktop_capture/win/window_capture_utils.cc#52
fn is_valid_window(hwnd: HWND) -> bool {
    unsafe {
        // ignore invisible windows
        if !IsWindow(Some(hwnd)).as_bool() || !IsWindowVisible(hwnd).as_bool() {
            return false;
        }

        // 特别说明，与webrtc中源码有区别，子窗口也枚举进来，所以就不需要下面的代码了：
        // HWND owner = GetWindow(hwnd, GW_OWNER);
        // LONG exstyle = GetWindowLong(hwnd, GWL_EXSTYLE);
        // if (owner && !(exstyle & WS_EX_APPWINDOW)) {
        //   return TRUE;
        // }

        let mut lp_class_name = [0u16; MAX_PATH as usize];
        let lp_class_name_length = GetClassNameW(hwnd, &mut lp_class_name) as usize;
        if lp_class_name_length < 1 {
            return false;
        }

        let class_name = U16CString::from_vec_truncate(&lp_class_name[0..lp_class_name_length])
            .to_string()
            .unwrap_or_default();
        if class_name.is_empty() {
            return false;
        }

        let gwl_ex_style = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);

        // 过滤掉具有 WS_EX_TOOLWINDOW 样式的窗口
        if gwl_ex_style.contains(WS_EX_TOOLWINDOW) {
            let title = get_window_title(hwnd).unwrap_or_default();

            // windows 任务栏可以捕获
            if class_name.ne(&"Shell_TrayWnd") && title.is_empty() {
                return false;
            }
        }

        // GetWindowText* are potentially blocking operations if `hwnd` is
        // owned by the current process. The APIs will send messages to the window's
        // message loop, and if the message loop is waiting on this operation we will
        // enter a deadlock.
        // https://docs.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowtexta#remarks
        //
        // To help consumers avoid this, there is a DesktopCaptureOption to ignore
        // windows owned by the current process. Consumers should either ensure that
        // the thread running their message loop never waits on this operation, or use
        // the option to exclude these windows from the source list.
        let lp_dw_process_id = get_window_pid(hwnd);
        if lp_dw_process_id == GetCurrentProcessId() {
            return false;
        }

        // Skip Program Manager window.
        if class_name.eq("Progman") {
            return false;
        }
        // Skip Start button window on Windows Vista, Windows 7.
        // On Windows 8, Windows 8.1, Windows 10 Start button is not a top level
        // window, so it will not be examined here.
        if class_name.eq("Button") {
            return false;
        }

        if is_window_cloaked(hwnd) {
            return false;
        }

        let mut rect = RECT::default();

        let get_rect_is_err = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut c_void,
            mem::size_of::<RECT>() as u32,
        )
        .is_err();

        if get_rect_is_err {
            return false;
        }

        if IsRectEmpty(&rect).as_bool() {
            return false;
        }
    }

    true
}

unsafe extern "system" fn enum_valid_windows(hwnd: HWND, state: LPARAM) -> BOOL {
    let state = Box::leak(Box::from_raw(state.0 as *mut Vec<HWND>));

    if is_valid_window(hwnd) {
        state.push(hwnd);
    }

    TRUE
}

unsafe extern "system" fn enum_all_windows(hwnd: HWND, state: LPARAM) -> BOOL {
    let state = Box::leak(Box::from_raw(state.0 as *mut Vec<HWND>));

    state.push(hwnd);

    TRUE
}

fn get_window_title(hwnd: HWND) -> XCapResult<String> {
    unsafe {
        let text_length = GetWindowTextLengthW(hwnd);
        let mut wide_buffer = vec![0u16; (text_length + 1) as usize];
        GetWindowTextW(hwnd, &mut wide_buffer);
        let window_title = U16CString::from_vec_truncate(wide_buffer).to_string()?;

        Ok(window_title)
    }
}

#[derive(Debug, Default)]
struct LangCodePage {
    pub w_language: u16,
    pub w_code_page: u16,
}

fn get_module_basename(handle: HANDLE) -> XCapResult<String> {
    unsafe {
        // 默认使用 module_basename
        let mut module_base_name_w = [0; MAX_PATH as usize];
        let result = GetModuleBaseNameW(handle, None, &mut module_base_name_w);

        if result == 0 {
            log::error!(
                "GetModuleBaseNameW({:?}) failed: {:?}",
                handle,
                GetLastError()
            );

            GetModuleFileNameExW(Some(handle), None, &mut module_base_name_w);
        }

        let module_basename = U16CString::from_vec_truncate(module_base_name_w).to_string()?;

        Ok(module_basename)
    }
}

fn get_window_pid(hwnd: HWND) -> u32 {
    unsafe {
        let mut lp_dw_process_id = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut lp_dw_process_id));
        lp_dw_process_id
    }
}

fn get_app_name(pid: u32) -> XCapResult<String> {
    unsafe {
        let scope_guard_handle = match open_process(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(box_handle) => box_handle,
            Err(err) => {
                log::error!("open_process failed: {}", err);
                return Ok(String::new());
            }
        };

        let mut filename = [0; MAX_PATH as usize];
        GetModuleFileNameExW(Some(*scope_guard_handle), None, &mut filename);

        let pcw_filename = PCWSTR::from_raw(filename.as_ptr());

        let file_version_info_size_w = GetFileVersionInfoSizeW(pcw_filename, None);
        if file_version_info_size_w == 0 {
            log::error!(
                "GetFileVersionInfoSizeW({:?}) failed: {:?}",
                pcw_filename,
                GetLastError()
            );

            return get_module_basename(*scope_guard_handle);
        }

        let mut file_version_info = vec![0u16; file_version_info_size_w as usize];

        GetFileVersionInfoW(
            pcw_filename,
            None,
            file_version_info_size_w,
            file_version_info.as_mut_ptr().cast(),
        )?;

        let mut lang_code_pages_ptr = ptr::null_mut();
        let mut lang_code_pages_length = 0;

        VerQueryValueW(
            file_version_info.as_ptr().cast(),
            &HSTRING::from("\\VarFileInfo\\Translation"),
            &mut lang_code_pages_ptr,
            &mut lang_code_pages_length,
        )
        .ok()?;

        let lang_code_pages: &[LangCodePage] =
            slice::from_raw_parts(lang_code_pages_ptr.cast(), lang_code_pages_length as usize);

        // 按照 keys 的顺序读取文件的属性值
        // 优先读取 FileDescription
        let keys = [
            "FileDescription",
            "ProductName",
            "ProductShortName",
            "InternalName",
            "OriginalFilename",
        ];

        for key in keys {
            for lang_code_page in lang_code_pages {
                let query_key = HSTRING::from(format!(
                    "\\StringFileInfo\\{:04x}{:04x}\\{}",
                    lang_code_page.w_language, lang_code_page.w_code_page, key
                ));

                let mut value_ptr = ptr::null_mut();
                let mut value_length: u32 = 0;

                let is_success = VerQueryValueW(
                    file_version_info.as_ptr().cast(),
                    &query_key,
                    &mut value_ptr,
                    &mut value_length,
                )
                .as_bool();

                if !is_success {
                    continue;
                }

                let value = slice::from_raw_parts(value_ptr.cast(), value_length as usize);
                let attr = U16CString::from_vec_truncate(value).to_string()?;
                let attr = attr.trim();

                if !attr.is_empty() {
                    return Ok(attr.to_string());
                }
            }
        }

        get_module_basename(*scope_guard_handle)
    }
}

impl ImplWindow {
    fn new(hwnd: HWND) -> ImplWindow {
        ImplWindow { hwnd }
    }

    pub fn all() -> XCapResult<Vec<ImplWindow>> {
        let hwnds_mut_ptr: *mut Vec<HWND> = Box::into_raw(Box::default());

        let hwnds = unsafe {
            EnumWindows(Some(enum_valid_windows), LPARAM(hwnds_mut_ptr as isize))?;
            Box::from_raw(hwnds_mut_ptr)
        };

        let mut impl_windows = Vec::new();

        for &hwnd in hwnds.iter() {
            impl_windows.push(ImplWindow::new(hwnd));
        }

        Ok(impl_windows)
    }
}

impl ImplWindow {
    pub fn id(&self) -> XCapResult<u32> {
        Ok(self.hwnd.0 as u32)
    }

    pub fn pid(&self) -> XCapResult<u32> {
        let pid = get_window_pid(self.hwnd);
        Ok(pid)
    }

    pub fn app_name(&self) -> XCapResult<String> {
        get_app_name(self.pid()?)
    }

    pub fn title(&self) -> XCapResult<String> {
        get_window_title(self.hwnd)
    }

    pub fn current_monitor(&self) -> XCapResult<ImplMonitor> {
        let h_monitor = unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) };

        Ok(ImplMonitor::new(h_monitor))
    }

    pub fn x(&self) -> XCapResult<i32> {
        let window_info = get_window_info(self.hwnd)?;
        Ok(window_info.rcClient.left)
    }

    pub fn y(&self) -> XCapResult<i32> {
        let window_info = get_window_info(self.hwnd)?;
        Ok(window_info.rcClient.top)
    }

    pub fn z(&self) -> XCapResult<i32> {
        let hwnds_mut_ptr: *mut Vec<HWND> = Box::into_raw(Box::default());

        let hwnds = unsafe {
            // EnumWindows 函数按照 Z 顺序遍历顶层窗口，从最顶层的窗口开始，依次向下遍历。
            EnumWindows(Some(enum_all_windows), LPARAM(hwnds_mut_ptr as isize))?;
            Box::from_raw(hwnds_mut_ptr)
        };

        let mut z = hwnds.len() as i32;
        for &hwnd in hwnds.iter() {
            z -= 1;
            if self.hwnd == hwnd {
                break;
            }
        }

        Ok(z)
    }

    pub fn width(&self) -> XCapResult<u32> {
        let window_info = get_window_info(self.hwnd)?;
        Ok((window_info.rcClient.right - window_info.rcClient.left) as u32)
    }

    pub fn height(&self) -> XCapResult<u32> {
        let window_info = get_window_info(self.hwnd)?;
        Ok((window_info.rcClient.bottom - window_info.rcClient.top) as u32)
    }

    pub fn is_minimized(&self) -> XCapResult<bool> {
        unsafe { Ok(IsIconic(self.hwnd).as_bool()) }
    }

    pub fn is_maximized(&self) -> XCapResult<bool> {
        unsafe { Ok(IsZoomed(self.hwnd).as_bool()) }
    }

    pub fn is_focused(&self) -> XCapResult<bool> {
        unsafe { Ok(GetForegroundWindow() == self.hwnd) }
    }

    pub fn capture_image(&self) -> XCapResult<RgbaImage> {
        // 在win10之后，不同窗口有不同的dpi，所以可能存在截图不全或者截图有较大空白，实际窗口没有填充满图片
        // 如果窗口不感知dpi，那么就不需要缩放，如果当前进程感知dpi，那么也不需要缩放
        let scope_guard_handle =
            open_process(PROCESS_QUERY_LIMITED_INFORMATION, false, self.pid()?)?;
        let window_is_dpi_awareness = get_process_is_dpi_awareness(*scope_guard_handle)?;
        let current_process_is_dpi_awareness =
            unsafe { get_process_is_dpi_awareness(GetCurrentProcess())? };

        let scale_factor = if !window_is_dpi_awareness {
            1.0
        } else if current_process_is_dpi_awareness {
            1.0
        } else {
            self.current_monitor()?.scale_factor()?
        };

        capture_window(self.hwnd, scale_factor)
    }
    pub fn capture_image_region(&self,x:u32,y:u32,width:i32,height:i32) -> XCapResult<RgbaImage> {
        // 在win10之后，不同窗口有不同的dpi，所以可能存在截图不全或者截图有较大空白，实际窗口没有填充满图片
        // 如果窗口不感知dpi，那么就不需要缩放，如果当前进程感知dpi，那么也不需要缩放
        let box_process_handle = BoxProcessHandle::open(PROCESS_ALL_ACCESS, false, self.pid)?;
        let window_is_dpi_awareness = get_process_is_dpi_awareness(*box_process_handle)?;
        let current_process_is_dpi_awareness =
            unsafe { get_process_is_dpi_awareness(GetCurrentProcess())? };

        let scale_factor = if !window_is_dpi_awareness {
            1.0
        } else if current_process_is_dpi_awareness {
            1.0
        } else {
            self.current_monitor.scale_factor
        };

        capture_window_region(self.hwnd, scale_factor, &self.window_info,x,y,width,height)
    }
}
