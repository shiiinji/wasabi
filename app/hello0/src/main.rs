#![no_std]
#![cfg_attr(not(target_os = "linux"), no_main)]

use noli::prelude::*;

fn main() {
    Api::write_string("**** Hello from an app!\n");
    Api::exit(42);
}

entry_point!(main);
