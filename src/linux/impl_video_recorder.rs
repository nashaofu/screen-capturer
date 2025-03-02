use super::impl_monitor::ImplMonitor;
use crate::error::{XCapError, XCapResult};
use crate::video_recorder::{Frame, RecorderWaker};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ImplVideoRecorder {
    monitor: ImplMonitor,
    sender: Sender<Frame>,
    running: Arc<Mutex<bool>>,
    recorder_waker: Arc<RecorderWaker>,
}

impl ImplVideoRecorder {
    pub fn new(monitor: ImplMonitor) -> XCapResult<(Self, Receiver<Frame>)> {
        let (sender, receiver) = mpsc::channel();
        let recorder = Self {
            monitor,
            sender,
            running: Arc::new(Mutex::new(false)),
            recorder_waker: Arc::new(RecorderWaker::new()),
        };

        recorder.on_frame()?;

        Ok((recorder, receiver))
    }

    pub fn on_frame(&self) -> XCapResult<()> {
        let monitor = self.monitor.clone();
        let sender = self.sender.clone();
        let running_flag = self.running.clone();
        let recorder_waker = self.recorder_waker.clone();

        thread::spawn(move || loop {
            if let Err(err) = recorder_waker.wait() {
                eprintln!("Recorder waker error: {:?}", err);
                break Err(err);
            }

            let is_running = match running_flag.lock() {
                Ok(guard) => *guard,
                Err(e) => {
                    eprintln!("Failed to lock running flag: {:?}", e);
                    break Err(XCapError::from(e));
                }
            };

            if !is_running {
                break Ok(());
            }

            match monitor.capture_image() {
                Ok(image) => {
                    let width = image.width();
                    let height = image.height();
                    let raw = image.into_raw();

                    let frame = Frame::new(width, height, raw);
                    if let Err(e) = sender.send(frame) {
                        eprintln!("Failed to send frame: {:?}", e);
                        break Err(XCapError::new(&format!("Failed to send frame: {}", e)));
                    }
                }
                Err(e) => {
                    eprintln!("Failed to capture frame: {:?}", e);
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
            }

            thread::sleep(Duration::from_millis(1));
        });

        Ok(())
    }

    pub fn start(&self) -> XCapResult<()> {
        let mut running = self.running.lock().map_err(XCapError::from)?;
        if *running {
            return Ok(());
        }
        *running = true;

        self.recorder_waker.wake()?;

        Ok(())
    }

    pub fn stop(&self) -> XCapResult<()> {
        let mut running = self.running.lock().map_err(XCapError::from)?;
        *running = false;

        self.recorder_waker.sleep()?;

        Ok(())
    }
}
