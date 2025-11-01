use std::fs;
use std::path::Path;
use std::process::Command;
use std::io::{self, Write};

pub fn perform_startup_checks() {
    log::info!("Performing startup checks...");
    check_and_generate_config();
    check_and_download_yt_dlp();
    log::info!("Startup checks completed.");
}

fn check_and_generate_config() {
    if !Path::new("config.json").exists() {
        log::warn!("config.json not found. Generating default config...");
        
        let default_config = r#"{
    "api_key": "YOUR_API_KEY_HERE",
    "available_qualities": ["144", "240", "360", "480", "720", "1080", "1440", "2160"],
    "default_quality": "360",
    "fetch_channel_thumbnails": false,
    "mainurl": "http://localhost:2823/",
    "oauth_client_id": "YOUR_OAUTH_CLIENT_ID",
    "oauth_client_secret": "YOUR_OAUTH_CLIENT_SECRET",
    "port": 2823,
    "request_timeout": 30,
    "secretkey": "YOUR_SECRET_KEY",
    "use_channel_thumbnail_proxy": false,
    "use_cookies": true,
    "use_thumbnail_proxy": true,
    "use_video_proxy": true,
    "video_source": "direct"
}"#;
        
        if let Err(e) = fs::write("config.json", default_config) {
            log::error!("Failed to create default config.json: {}", e);
            std::process::exit(1);
        }
        
        log::info!("Default config.json created. Please update it with your actual values.");
    } else {
        log::info!("CHECK: config.json found.");
    }
}

fn check_and_download_yt_dlp() {
    let yt_dlp_exists = Command::new("yt-dlp")
        .arg("--version")
        .output()
        .is_ok() || 
        Path::new("assets/yt-dlp").exists() || 
        Path::new("assets/yt-dlp.exe").exists();
    
    if yt_dlp_exists {
        log::info!("CHECK: yt-dlp found.");
        return;
    }
    
    log::error!("yt-dlp not found!");
    print!("Would you like to download the latest version of yt-dlp from GitHub? (y/n): ");
    io::stdout().flush().unwrap();
    
    let mut input = String::new();
    io::stdin().read_line(&mut input).expect("Failed to read input");
    
    if input.trim().to_lowercase() == "y" || input.trim().to_lowercase() == "yes" {
        log::info!("Downloading latest yt-dlp...");
        
        match download_yt_dlp() {
            Ok(_) => {
                log::info!("yt-dlp downloaded successfully!");
                log::info!("Please restart the application.");
                std::process::exit(0);
            },
            Err(e) => {
                log::error!("Failed to download yt-dlp: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        log::error!("yt-dlp is required to run this application. Exiting...");
        std::process::exit(1);
    }
}

fn download_yt_dlp() -> Result<(), Box<dyn std::error::Error>> {
    if !Path::new("assets").exists() {
        fs::create_dir("assets")?;
    }
    
    let binary_name = if cfg!(target_os = "windows") {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    };
    
    let client = reqwest::blocking::Client::new();
    
    let url = if cfg!(target_os = "windows") {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
    } else {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp"
    };
    
    let response = client.get(url).send()?;
    let content = response.bytes()?;
    
    let file_path = format!("assets/{}", binary_name);
    fs::write(&file_path, content)?;
    
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&file_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&file_path, perms)?;
    }
    
    Ok(())
}