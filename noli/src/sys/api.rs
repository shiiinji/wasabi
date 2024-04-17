pub use sabi::MouseEvent;

/// impl can be found at:
/// - src/sys/wasabi.rs
/// - src/sys/linux.rs
pub trait SystemApi {
    fn exit(_code: u64) -> ! {
        unimplemented!()
    }
    fn write_string(_s: &str) -> u64 {
        unimplemented!()
    }
    fn draw_point(_x: i64, _y: i64, _c: u32) -> u64 {
        unimplemented!();
    }
    fn noop() -> u64 {
        unimplemented!()
    }
    /// Returns None if no key was in the queue.
    /// This may yield the execution to the OS.
    fn read_key() -> Option<char> {
        unimplemented!()
    }
    /// Returns Some if there is a new event, or None.
    /// This may yield the execution to the OS.
    fn get_mouse_cursor_info() -> Option<MouseEvent> {
        unimplemented!()
    }
}
