use std::io::Write;
use chrono::Local;
use colored::*;

pub fn init_logger() {
    std::env::set_var("RUST_LOG", "info");
    env_logger::builder()
        .format_timestamp(None)
        .format_module_path(false)
        .format_target(false)
        .filter_level(log::LevelFilter::Info)
        .format(format_log)
        .init();
}

fn format_log(
    buf: &mut env_logger::fmt::Formatter,
    record: &log::Record,
) -> std::io::Result<()> {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    
    let level_str = match record.level() {
        log::Level::Error => "[ERROR]".red(),
        log::Level::Warn => "[WARN]".yellow(),
        log::Level::Info => "[INFO]".green(),
        log::Level::Debug => "[DEBUG]".blue(),
        log::Level::Trace => "[TRACE]".purple(),
    };
    
    writeln!(
        buf,
        "{} {} {}",
        timestamp.dimmed(),
        level_str,
        record.args()
    )
}

pub use log::info;