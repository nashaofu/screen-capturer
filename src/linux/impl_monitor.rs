use std::{ffi::CStr, sync::mpsc::Receiver};

use image::RgbaImage;
use xcb::{
    randr::{
        GetCrtcInfo, GetMonitors, GetOutputInfo, GetOutputProperty, GetScreenResources, Mode,
        ModeFlag, ModeInfo, Output, Rotation,
    },
    x::{GetProperty, ATOM_ANY, ATOM_RESOURCE_MANAGER, ATOM_STRING, CURRENT_TIME},
    Xid,
};

use crate::{
    error::{XCapError, XCapResult},
    video_recorder::Frame,
};

use super::{
    capture::capture_monitor,
    impl_video_recorder::ImplVideoRecorder,
    utils::{get_atom, get_current_screen_buf, get_monitor_info_buf, get_xcb_connection_and_index},
};

#[derive(Debug, Clone)]
pub(crate) struct ImplMonitor {
    pub output: Output,
}

// per https://gitlab.freedesktop.org/xorg/app/xrandr/-/blob/master/xrandr.c#L576
fn get_current_frequency(mode_infos: Vec<ModeInfo>, mode: Mode) -> f32 {
    let mode_info = match mode_infos.iter().find(|m| m.id == mode.resource_id()) {
        Some(mode_info) => mode_info,
        None => return 0.0,
    };

    let vtotal = {
        let mut val = mode_info.vtotal;
        if mode_info.mode_flags.contains(ModeFlag::DOUBLE_SCAN) {
            val *= 2;
        }
        if mode_info.mode_flags.contains(ModeFlag::INTERLACE) {
            val /= 2;
        }
        val
    };

    if vtotal != 0 && mode_info.htotal != 0 {
        (mode_info.dot_clock as f32) / (vtotal as f32 * mode_info.htotal as f32)
    } else {
        0.0
    }
}

fn get_scale_factor() -> XCapResult<f32> {
    let (conn, _) = get_xcb_connection_and_index()?;

    let screen_buf = get_current_screen_buf()?;

    let xft_dpi_prefix = "Xft.dpi:\t";

    let get_property_cookie = conn.send_request(&GetProperty {
        delete: false,
        window: screen_buf.root(),
        property: ATOM_RESOURCE_MANAGER,
        r#type: ATOM_STRING,
        long_offset: 0,
        long_length: 60,
    });

    let get_property_reply = conn.wait_for_reply(get_property_cookie)?;

    let resource_manager = String::from_utf8(get_property_reply.value().to_vec())?;

    let xft_dpi = resource_manager
        .split('\n')
        .find(|s| s.starts_with(xft_dpi_prefix))
        .ok_or_else(|| XCapError::new("Xft.dpi parse failed"))?
        .strip_prefix(xft_dpi_prefix)
        .ok_or_else(|| XCapError::new("Xft.dpi parse failed"))?;

    let dpi = xft_dpi.parse::<f32>().map_err(XCapError::new)?;

    Ok(dpi / 96.0)
}

fn get_rotation_frequency(mode_infos: Vec<ModeInfo>, output: &Output) -> XCapResult<(f32, f32)> {
    let (conn, _) = get_xcb_connection_and_index()?;
    let get_output_info_cookie = conn.send_request(&GetOutputInfo {
        output: *output,
        config_timestamp: CURRENT_TIME,
    });

    let get_output_info_reply = conn.wait_for_reply(get_output_info_cookie)?;

    let get_crtc_info_cookie = conn.send_request(&GetCrtcInfo {
        crtc: get_output_info_reply.crtc(),
        config_timestamp: CURRENT_TIME,
    });

    let get_crtc_info_reply = conn.wait_for_reply(get_crtc_info_cookie)?;

    let mode = get_crtc_info_reply.mode();

    let rotation = match get_crtc_info_reply.rotation() {
        Rotation::ROTATE_0 => 0.0,
        Rotation::ROTATE_90 => 90.0,
        Rotation::ROTATE_180 => 180.0,
        Rotation::ROTATE_270 => 270.0,
        _ => 0.0,
    };

    let frequency = get_current_frequency(mode_infos, mode);

    Ok((rotation, frequency))
}

fn get_mode_infos() -> XCapResult<Vec<ModeInfo>> {
    let (conn, _) = get_xcb_connection_and_index()?;

    let screen_buf = get_current_screen_buf()?;

    let get_screen_resources_cookie = conn.send_request(&GetScreenResources {
        window: screen_buf.root(),
    });

    let get_screen_resources_reply = conn.wait_for_reply(get_screen_resources_cookie)?;

    let mode_infos = get_screen_resources_reply.modes().to_vec();

    Ok(mode_infos)
}

fn get_output_edid(output: Output) -> XCapResult<Vec<u8>> {
    let (conn, _) = get_xcb_connection_and_index()?;
    let atom = get_atom("EDID")?;

    let get_output_property_cookie = conn.send_request(&GetOutputProperty {
        output,
        property: atom,
        r#type: ATOM_ANY,
        long_offset: 0,
        long_length: 128,
        delete: false,
        pending: false,
    });
    let get_output_property_reply = conn.wait_for_reply(get_output_property_cookie)?;

    let edid = get_output_property_reply.data::<u8>().to_vec();

    Ok(edid)
}

fn is_builtin_edid(edid: &[u8]) -> bool {
    const DESCRIPTOR_OFFSET: usize = 0x36;

    // 遍历 EDID 描述符块
    for i in 0..4 {
        let offset = DESCRIPTOR_OFFSET + i * 18;
        if offset + 5 >= edid.len() {
            break;
        }

        // 检查描述符类型 (0xFC 为显示器名称)
        if edid[offset] == 0xFC {
            let text = &edid[offset + 5..offset + 18];
            if let Ok(name) = CStr::from_bytes_until_nul(text) {
                if name.to_string_lossy().contains("Internal") {
                    return true;
                }
            }
        }
    }

    false
}

impl ImplMonitor {
    fn new(output: Output) -> ImplMonitor {
        ImplMonitor { output }
    }

    pub fn all() -> XCapResult<Vec<ImplMonitor>> {
        let (conn, _) = get_xcb_connection_and_index()?;

        let screen_buf = get_current_screen_buf()?;

        let get_monitors_cookie = conn.send_request(&GetMonitors {
            window: screen_buf.root(),
            get_active: true,
        });

        let get_monitors_reply = conn.wait_for_reply(get_monitors_cookie)?;

        let monitor_info_iterator = get_monitors_reply.monitors();

        let mut impl_monitors = Vec::new();

        for monitor_info in monitor_info_iterator {
            for &output in monitor_info.outputs() {
                impl_monitors.push(ImplMonitor::new(output));
            }
        }

        Ok(impl_monitors)
    }

    pub fn from_point(x: i32, y: i32) -> XCapResult<ImplMonitor> {
        let (conn, _) = get_xcb_connection_and_index()?;

        let screen_buf = get_current_screen_buf()?;

        let scale_factor = get_scale_factor().unwrap_or(1.0);

        let get_monitors_cookie = conn.send_request(&GetMonitors {
            window: screen_buf.root(),
            get_active: true,
        });

        let get_monitors_reply = conn.wait_for_reply(get_monitors_cookie)?;

        let monitor_info_iterator = get_monitors_reply.monitors();

        let x = (x as f32 * scale_factor) as i32;
        let y = (y as f32 * scale_factor) as i32;

        for monitor_info in monitor_info_iterator {
            let left = monitor_info.x() as i32;
            let right = monitor_info.x() as i32 + monitor_info.width() as i32;
            let top = monitor_info.y() as i32;
            let bottom = monitor_info.y() as i32 + monitor_info.height() as i32;

            if x >= left && x < right && y >= top && y < bottom {
                if let Some(&output) = monitor_info.outputs().first() {
                    return Ok(ImplMonitor::new(output));
                }
            }
        }

        Err(XCapError::new("Not found monitor"))
    }
}

impl ImplMonitor {
    pub fn id(&self) -> XCapResult<u32> {
        Ok(self.output.resource_id())
    }

    pub fn name(&self) -> XCapResult<String> {
        let (conn, _) = get_xcb_connection_and_index()?;
        let get_output_info_cookie = conn.send_request(&GetOutputInfo {
            output: self.output,
            config_timestamp: CURRENT_TIME,
        });
        let get_output_info_reply = conn.wait_for_reply(get_output_info_cookie)?;

        let name = String::from_utf8(get_output_info_reply.name().to_vec())?;
        Ok(name)
    }

    pub fn x(&self) -> XCapResult<i32> {
        let x = get_monitor_info_buf(self.output)?.x();
        let scale_factor = self.scale_factor()?;

        Ok(((x as f32) / scale_factor) as i32)
    }

    pub fn y(&self) -> XCapResult<i32> {
        let y = get_monitor_info_buf(self.output)?.y();
        let scale_factor = self.scale_factor()?;

        Ok(((y as f32) / scale_factor) as i32)
    }

    pub fn width(&self) -> XCapResult<u32> {
        let width = get_monitor_info_buf(self.output)?.width();
        let scale_factor = self.scale_factor()?;

        Ok(((width as f32) / scale_factor) as u32)
    }

    pub fn height(&self) -> XCapResult<u32> {
        let height = get_monitor_info_buf(self.output)?.height();
        let scale_factor = self.scale_factor()?;

        Ok(((height as f32) / scale_factor) as u32)
    }

    pub fn rotation(&self) -> XCapResult<f32> {
        let mode_infos = get_mode_infos()?;
        let (rotation, _) = get_rotation_frequency(mode_infos, &self.output).unwrap_or((0.0, 0.0));

        Ok(rotation)
    }

    pub fn scale_factor(&self) -> XCapResult<f32> {
        let scale_factor = get_scale_factor().unwrap_or(1.0);

        Ok(scale_factor)
    }

    pub fn frequency(&self) -> XCapResult<f32> {
        let mode_infos = get_mode_infos()?;
        let (_, frequency) = get_rotation_frequency(mode_infos, &self.output).unwrap_or((0.0, 0.0));
        Ok(frequency)
    }

    pub fn is_primary(&self) -> XCapResult<bool> {
        let primary = get_monitor_info_buf(self.output)?.primary();

        Ok(primary)
    }

    pub fn is_builtin(&self) -> XCapResult<bool> {
        let name = self.name()?;

        if name.starts_with("eDP") || name.starts_with("LVDS") {
            return Ok(true);
        }

        let edid = get_output_edid(self.output)?;

        Ok(is_builtin_edid(&edid))
    }

    pub fn capture_image(&self) -> XCapResult<RgbaImage> {
        capture_monitor(self)
    }

    pub fn video_recorder(&self) -> XCapResult<(ImplVideoRecorder, Receiver<Frame>)> {
        ImplVideoRecorder::new(self.clone())
    }
}
