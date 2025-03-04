use fs_extra::dir;
use std::time::Instant;
use xcap::Window;

fn normalized(filename: &str) -> String {
    filename.replace(['|', '\\', ':', '/'], "")
}

fn main() {
    let start = Instant::now();
    let windows = Window::all().unwrap();

    dir::create_all("target/windows", true).unwrap();

    let mut i = 0;
    for window in windows {
        // 最小化的窗口不能截屏
        if window.is_minimized() {
            continue;
        }

        println!(
            "Window: {:?} {:?} {:?}",
            window.title(),
            (window.x(), window.y(), window.width(), window.height()),
            (window.is_minimized(), window.is_maximized())
        );
        // 需要确保x、y、w、h不能超出窗口范围，否则会出现截图不全或者数据为空的情况
        match window.capture_image_region(200,200,200,200) {
            Ok(image) => {
                let filename = format!("target/window-{}-{}.png", i, normalized(&window.title()));
                let screen_shot_time = Instant::now();

                if let Err(e) = image.save(filename) {
                    eprintln!("保存图片失败: {}", e);
                } else {
                    println!("已保存截图: window-{}.png", i);
                }
                println!("screen shot 运行耗时: {:?}", screen_shot_time.elapsed());
            }
            Err(e) => {
                eprintln!("无法捕获窗口 {} 的截图: {:?}", window.title(), e);
            }
        }
        i += 1;
    }

    println!("运行耗时: {:?}", start.elapsed());
}
