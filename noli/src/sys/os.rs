pub struct Api;

impl Api {
    pub fn exit(code: i32) -> ! {
        // システム終了のシステムコール
        // 実際のシステムコールの実装に応じて修正が必要
        unsafe {
            syscall::sys_exit(code);
        }
        loop {}
    }

    pub fn write_string(s: &str) {
        // 文字列出力のシステムコール
        // 実際のシステムコールの実装に応じて修正が必要
        unsafe {
            syscall::sys_write(1, s.as_bytes());
        }
    }
}

// システムコールの内部実装
mod syscall {
    #[allow(dead_code)]
    pub(crate) unsafe fn sys_exit(code: i32) {
        // ここにシステムコールの実装を追加
        // 例: アセンブリでシステムコールを呼び出す
    }

    #[allow(dead_code)]
    pub(crate) unsafe fn sys_write(fd: i32, data: &[u8]) {
        // ここにシステムコールの実装を追加
        // 例: アセンブリでシステムコールを呼び出す
    }
}