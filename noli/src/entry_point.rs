#[macro_export]
macro_rules! entry_point {
    ($path:path) => {
        #[no_mangle]
        pub extern "C" fn _start() -> ! {
            // Call the user's entry point function
            let result = $path();
            // Exit with the result
            Api::exit(result.into());
            loop {}
        }
    };
}