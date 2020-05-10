#![macro_use]
use std::cell::RefCell;
use log::{self, Record, Metadata};

pub struct Logger;

thread_local!(static PREFIX: RefCell<String> = RefCell::new(String::new()));

#[macro_export]
macro_rules! log_scope {
    ($string:expr) => (
        let _very_unique_log_scope = log::Context::new($string);
    )
}

pub struct Context {
    prevfix: String,
}

impl Context {
    // TODO optimize
    pub fn new<S: Into<String>>(prefix: S) -> Context {
        let prefix = prefix.into();
        let prevfix = PREFIX.with(|p| {
            let prefix = p.borrow().to_string() + &prefix;
            return p.replace(prefix);
        });
        return Context{ prevfix };
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        PREFIX.with(|p| {
            std::mem::swap(&mut self.prevfix, &mut p.borrow_mut());
        });
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let now = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false);
            PREFIX.with(|p| {
                println!("{} [{}] {} - {}", now, p.borrow(), record.level(), record.args());
            });
        }
    }

    fn flush(&self) {}
}
