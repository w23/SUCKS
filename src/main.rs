#![allow(non_snake_case)]
use std::env;

mod socks;
mod exit;

/*
#[derive(Debug)]
struct SimpleError {
    what: String
}

impl SimpleError {
    fn new<T: Into<String>>(t: T) -> SimpleError {
        SimpleError{ what: t.into() }
    }
}

impl std::fmt::Display for SimpleError {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.what.fmt(f)
    }
}

impl std::error::Error for SimpleError {}
*/

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    socks::main(&args[1], "")
}
