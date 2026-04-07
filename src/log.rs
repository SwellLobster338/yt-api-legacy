use actix_web::{
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use chrono::Local;
use colored::*;
use futures_util::future::LocalBoxFuture;
use std::future::{ready, Ready};
use std::io::Write;
use std::task::{Context, Poll};
use std::fs::{OpenOptions, create_dir_all};
use std::sync::Mutex;

// Global file logger
static FILE_LOGGER: Mutex<Option<FileLogger>> = Mutex::new(None);

struct FileLogger {
    file: std::fs::File,
}

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

pub fn init_file_logger(enabled: bool, directory: &str) {
    if !enabled {
        return;
    }

    // Create logs directory if it doesn't exist
    if let Err(e) = create_dir_all(directory) {
        eprintln!("Failed to create log directory '{}': {}", directory, e);
        return;
    }

    // Generate filename with exact startup date and time
    let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
    let filename = format!("{}/yt-api-{}.log", directory, timestamp);

    // Open file for writing (append mode)
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&filename)
    {
        Ok(file) => {
            let mut logger = FILE_LOGGER.lock().unwrap();
            *logger = Some(FileLogger { file });
        }
        Err(e) => {
            eprintln!("Failed to open log file '{}': {}", filename, e);
        }
    }
}

fn write_to_file(message: &str) {
    let mut logger = FILE_LOGGER.lock().unwrap();
    if let Some(ref mut file_logger) = *logger {
        let _ = writeln!(file_logger.file, "{}", message);
        let _ = file_logger.file.flush();
    }
}

fn format_log(buf: &mut env_logger::fmt::Formatter, record: &log::Record) -> std::io::Result<()> {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let level_str = match record.level() {
        log::Level::Error => "[ERROR]".red(),
        log::Level::Warn => "[WARN]".yellow(),
        log::Level::Info => "[INFO]".green(),
        log::Level::Debug => "[DEBUG]".blue(),
        log::Level::Trace => "[TRACE]".purple(),
    };

    // Write to console
    writeln!(
        buf,
        "{} {} {}",
        timestamp.dimmed(),
        level_str,
        record.args()
    )?;

    // Write to file (without colors)
    if FILE_LOGGER.lock().unwrap().is_some() {
        let plain_level = match record.level() {
            log::Level::Error => "[ERROR]",
            log::Level::Warn => "[WARN]",
            log::Level::Info => "[INFO]",
            log::Level::Debug => "[DEBUG]",
            log::Level::Trace => "[TRACE]",
        };
        let log_line = format!("{} {} {}", timestamp, plain_level, record.args());
        write_to_file(&log_line);
    }

    Ok(())
}

pub use log::info;

#[derive(Default)]
pub struct SelectiveLogger;

impl<S, B> Transform<S, ServiceRequest> for SelectiveLogger
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = SelectiveLoggerMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(SelectiveLoggerMiddleware { service }))
    }
}

pub struct SelectiveLoggerMiddleware<S> {
    service: S,
}

impl<S, B> Service<ServiceRequest> for SelectiveLoggerMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        // Get client IP
        let client_ip = req
            .connection_info()
            .peer_addr()
            .map(|addr| {
                // Extract IP without port
                addr.split(':').next().unwrap_or(addr).to_string()
            })
            .unwrap_or_else(|| "unknown".to_string());

        let path = req.path().to_string();
        let method = req.method().to_string();
        
        let fut = self.service.call(req);

        Box::pin(async move {
            let res = fut.await?;
            let status = res.status();

            // Log all requests with IP, method, path and status
            info!(
                "[{}] {} {} - {} {}",
                client_ip,
                method,
                path,
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown")
            );

            Ok(res)
        })
    }
}
