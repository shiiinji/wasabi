extern crate alloc;

use crate::boot_info::BootInfo;
use crate::command;
use crate::executor::yield_execution;
use crate::executor::Executor;
use crate::executor::Task;
use crate::executor::TimeoutFuture;
use crate::graphics::draw_rect;
use crate::graphics::Bitmap;
use crate::mutex::Mutex;
use crate::net::network_manager_thread;
use crate::print;
use crate::println;
use crate::ps2::keyboard_task;
use crate::serial::SerialPort;
use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::string::String;

pub struct MouseButtonState {
    pub l: bool,
    pub c: bool,
    pub r: bool,
}

pub struct InputManager {
    input_queue: Mutex<VecDeque<char>>,
    cursor_queue: Mutex<VecDeque<(f32, f32, MouseButtonState)>>,
}
impl InputManager {
    fn new() -> Self {
        Self {
            input_queue: Mutex::new(VecDeque::new(), "InputManager.input_queue"),
            cursor_queue: Mutex::new(VecDeque::new(), "InputManager.cursor_queue"),
        }
    }
    pub fn take() -> Rc<Self> {
        let mut instance = INPUT_MANAGER.lock();
        let instance = instance.get_or_insert_with(|| Rc::new(Self::new()));
        instance.clone()
    }
    pub fn push_input(&self, value: char) {
        self.input_queue.lock().push_back(value)
    }
    pub fn pop_input(&self) -> Option<char> {
        self.input_queue.lock().pop_front()
    }

    // x, y: 0f32..1f32, top left origin
    pub fn push_cursor_input_absolute(&self, cx: f32, cy: f32, b: MouseButtonState) {
        self.cursor_queue.lock().push_back((cx, cy, b))
    }
    pub fn pop_cursor_input_absolute(&self) -> Option<(f32, f32, MouseButtonState)> {
        self.cursor_queue.lock().pop_front()
    }
}
static INPUT_MANAGER: Mutex<Option<Rc<InputManager>>> = Mutex::new(None, "INPUT_MANAGER");

pub fn enqueue_input_tasks(executor: &mut Executor) {
    let serial_task = async {
        let sp = SerialPort::default();
        loop {
            if let Some(c) = sp.try_read() {
                if let Some(c) = char::from_u32(c as u32) {
                    InputManager::take().push_input(c);
                }
            }
            TimeoutFuture::new_ms(20).await;
            yield_execution().await;
        }
    };
    let console_task = async {
        println!("INFO: console_task has started");
        let mut s = String::new();
        loop {
            if let Some(c) = InputManager::take().pop_input() {
                if c == '\r' || c == '\n' {
                    if let Err(e) = command::run(&s) {
                        println!("{e:?}");
                    };
                    s.clear();
                }
                match c {
                    'a'..='z' | 'A'..='Z' | '0'..='9' | ' ' | '.' => {
                        print!("{c}");
                        s.push(c);
                    }
                    c if c as u8 == 0x7f => {
                        print!("{0} {0}", 0x08 as char);
                        s.pop();
                    }
                    _ => {
                        // Do nothing
                    }
                }
            }
            TimeoutFuture::new_ms(20).await;
            yield_execution().await;
        }
    };
    let mouse_cursor_task = async {
        let mut vram = BootInfo::take().vram();
        let iw = vram.width();
        let ih = vram.height();
        let w = iw as f32;
        let h = ih as f32;
        loop {
            if let Some((px, py, b)) = InputManager::take().pop_cursor_input_absolute() {
                let px = (px * w) as i64;
                let py = (py * h) as i64;
                let px = px.clamp(0, iw - 1);
                let py = py.clamp(0, ih - 1);
                let color = (b.l as u32) * 0xff0000;
                let color = !color;

                draw_rect(&mut vram, color, px, py, 1, 1)?;
            }
            TimeoutFuture::new_ms(15).await;
            yield_execution().await;
        }
    };
    executor.spawn(Task::new(async { keyboard_task().await }));
    executor.spawn(Task::new(serial_task));
    executor.spawn(Task::new(console_task));
    executor.spawn(Task::new(mouse_cursor_task));
    executor.spawn(Task::new(async { network_manager_thread().await }));
}
