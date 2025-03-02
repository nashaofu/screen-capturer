use image::RgbaImage;

use crate::{error::XCapResult, platform::impl_window::ImplWindow, Monitor};

#[derive(Debug, Clone)]
pub struct Window {
    pub(crate) impl_window: ImplWindow,
}

impl Window {
    pub(crate) fn new(impl_window: ImplWindow) -> Window {
        Window { impl_window }
    }
}

impl Window {
    /// List all windows, sorted by z coordinate.
    pub fn all() -> XCapResult<Vec<Window>> {
        let windows = ImplWindow::all()?
            .iter()
            .map(|impl_window| Window::new(impl_window.clone()))
            .collect();

        Ok(windows)
    }
}

impl Window {
    /// The window id
    pub fn id(&self) -> XCapResult<u32> {
        self.impl_window.id()
    }
    /// The window process id
    pub fn pid(&self) -> XCapResult<u32> {
        self.impl_window.pid()
    }
    /// The window app name
    pub fn app_name(&self) -> XCapResult<String> {
        self.impl_window.app_name()
    }
    /// The window title
    pub fn title(&self) -> XCapResult<String> {
        self.impl_window.title()
    }
    /// The window current monitor
    pub fn current_monitor(&self) -> XCapResult<Monitor> {
        Ok(Monitor::new(self.impl_window.current_monitor()?))
    }
    /// The window x coordinate.
    pub fn x(&self) -> XCapResult<i32> {
        self.impl_window.x()
    }
    /// The window y coordinate.
    pub fn y(&self) -> XCapResult<i32> {
        self.impl_window.y()
    }
    /// The window z coordinate.
    pub fn z(&self) -> XCapResult<i32> {
        self.impl_window.z()
    }
    /// The window pixel width.
    pub fn width(&self) -> XCapResult<u32> {
        self.impl_window.width()
    }
    /// The window pixel height.
    pub fn height(&self) -> XCapResult<u32> {
        self.impl_window.height()
    }
    /// The window is minimized.
    pub fn is_minimized(&self) -> XCapResult<bool> {
        self.impl_window.is_minimized()
    }
    /// The window is maximized.
    pub fn is_maximized(&self) -> XCapResult<bool> {
        self.impl_window.is_maximized()
    }
    /// The window is focused.
    pub fn is_focused(&self) -> XCapResult<bool> {
        self.impl_window.is_focused()
    }
}

impl Window {
    pub fn capture_image(&self) -> XCapResult<RgbaImage> {
        self.impl_window.capture_image()
    }
    pub fn capture_image_region(&self, x: u32, y: u32, w: i32, h: i32) -> XCapResult<RgbaImage> {
        self.impl_window.capture_image_region(x, y, w, h)
    }
}
