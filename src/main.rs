//! HoloFan — cross-platform CLI and GUI entry point.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use formatjs_intl::format_message;

#[cfg(windows)]
use holofan::gui_win32;
use holofan::{device, format, gui, i18n, media, polar};

use format::{HologramContainer, PlaylistEntry};
use polar::PolarConfig;

/// Cross-platform 3D hologram fan media encoder and playlist manager.
#[derive(Parser, Debug)]
#[command(
    name = "holofan",
    version = env!("CARGO_PKG_VERSION"),
    about = "3D Holographic LED fan display software",
    long_about = "Cross-platform tool for creating, unpacking, and analyzing holographic fan (.bin) containers and USB HID device communication."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Launch graphical user interface
    #[arg(long, help = "Launch graphical user interface (GUI)")]
    gui: bool,

    /// Input file path (for CLI compatibility)
    #[arg(short, long, value_name = "FILE", global = true)]
    input: Option<PathBuf>,

    /// Output file path (for CLI compatibility)
    #[arg(short, long, value_name = "FILE", global = true)]
    output: Option<PathBuf>,

    /// Verbose debug logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Read raw memory dump from USB HID device
    #[arg(long)]
    dump: bool,

    /// Pre-loaded media or container files to open
    #[arg(value_name = "FILES")]
    files: Vec<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Show binary container file structure and playlist
    #[command(name = "info", alias = "инфо")]
    Info(InfoArgs),

    /// Create .bin container from images, GIF, WebP, or folder
    #[command(name = "create", alias = "создать")]
    Create(CreateArgs),

    /// Unpack .bin container into PNG frames
    #[command(name = "unpack", alias = "распаковать")]
    Unpack(UnpackArgs),

    /// Convert single image to polar frame or preview
    #[command(name = "convert", alias = "преобразовать")]
    Convert(ConvertArgs),

    /// Operations with connected USB HID fan device
    #[command(name = "device", alias = "устройство")]
    Device(DeviceArgs),

    /// Launch graphical user interface
    #[command(name = "gui", alias = "интерфейс")]
    Gui,
}

#[derive(Args, Debug)]
struct InfoArgs {
    /// Target container file path (.bin)
    #[arg(short, long, value_name = "FILE")]
    file: PathBuf,
}

#[derive(Args, Debug)]
struct CreateArgs {
    /// Source image, animated GIF, WebP, or directory of frames
    #[arg(short, long, value_name = "PATH")]
    input: PathBuf,

    /// Output binary container path (default: hologram.bin)
    #[arg(short, long, value_name = "FILE", default_value = "hologram.bin")]
    output: PathBuf,

    /// Playback speed parameter (1..255, default: 50)
    #[arg(short, long, default_value_t = 50)]
    speed: u8,

    /// Repeat count for static images (default: 15). Animated files keep their source frames.
    #[arg(long, default_value_t = 15)]
    repeat: usize,

    /// Playback mode parameter (default: 1)
    #[arg(short, long, default_value_t = 1)]
    mode: u8,
}

#[derive(Args, Debug)]
struct UnpackArgs {
    /// Binary container path (.bin)
    #[arg(short, long, value_name = "FILE")]
    input: PathBuf,

    /// Output directory for unpacked frames
    #[arg(short, long, value_name = "DIR", default_value = "unpacked_frames")]
    output: PathBuf,

    /// Save raw binary polar frames instead of PNG preview images
    #[arg(long)]
    raw: bool,
}

#[derive(Args, Debug)]
struct ConvertArgs {
    /// Input image file (PNG, JPG, BMP)
    #[arg(short, long, value_name = "FILE")]
    input: PathBuf,

    /// Output preview PNG file
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
}

#[derive(Args, Debug)]
struct DeviceArgs {
    #[command(subcommand)]
    action: DeviceSubcommands,
}

#[derive(Subcommand, Debug)]
enum DeviceSubcommands {
    /// Query connected fan status over USB HID
    #[command(name = "status", alias = "статус")]
    Status,

    /// Read raw memory dump from USB HID device
    #[command(name = "dump", alias = "дамп")]
    Dump(DeviceDumpArgs),
}

#[derive(Args, Debug)]
struct DeviceDumpArgs {
    /// Output file path for raw device dump
    #[arg(short, long, value_name = "FILE", default_value = "device_dump.bin")]
    output: PathBuf,
}

#[cfg(windows)]
fn hide_console_window_if_gui() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> *mut std::ffi::c_void;
        fn GetConsoleProcessList(lpdwProcessList: *mut u32, dwProcessCount: u32) -> u32;
        fn FreeConsole() -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(hWnd: *mut std::ffi::c_void, nCmdShow: i32) -> i32;
    }

    unsafe {
        let mut process_list = [0u32; 2];
        let count = GetConsoleProcessList(process_list.as_mut_ptr(), 2);
        let hwnd = GetConsoleWindow();
        if count <= 1 && !hwnd.is_null() {
            const SW_HIDE: i32 = 0;
            ShowWindow(hwnd, SW_HIDE);
        }
        FreeConsole();
    }
}

fn main() -> Result<()> {
    // Default to launching GUI when invoked with no CLI arguments
    if std::env::args().len() <= 1 {
        return run_platform_gui();
    }

    let cli = Cli::parse();

    if cli.gui {
        return run_platform_gui();
    }

    if let Some(cmd) = cli.command {
        match cmd {
            Commands::Info(args) => handle_info(&args.file, cli.verbose)?,
            Commands::Create(args) => handle_create(&args, cli.verbose)?,
            Commands::Unpack(args) => handle_unpack(&args, cli.verbose)?,
            Commands::Convert(args) => handle_convert(&args, cli.verbose)?,
            Commands::Device(args) => handle_device(&args.action, cli.verbose)?,
            Commands::Gui => run_platform_gui()?,
        }
        return Ok(());
    }

    // Backward compatibility for legacy CLI flags
    if cli.dump {
        let output_path = cli
            .output
            .unwrap_or_else(|| PathBuf::from("device_dump.bin"));
        run_legacy_dump(&output_path, cli.verbose)?;
        return Ok(());
    }

    if let (Some(input_path), Some(output_path)) = (cli.input, cli.output) {
        let data = fs::read(&input_path)
            .with_context(|| format!("Failed to read input file: {}", input_path.display()))?;
        fs::write(&output_path, &data)
            .with_context(|| format!("Failed to write output file: {}", output_path.display()))?;
        println!(
            "File copied successfully: {} -> {}",
            input_path.display(),
            output_path.display()
        );
        return Ok(());
    }

    if !cli.files.is_empty() {
        return run_platform_gui_with_files(cli.files);
    }

    // Fallback: launch GUI
    run_platform_gui()
}

#[cfg(windows)]
fn run_platform_gui() -> anyhow::Result<()> {
    hide_console_window_if_gui();
    if let Err(e) = gui::run_gui() {
        eprintln!("Failed to launch hardware eframe/WGPU GUI ({}). Falling back to native Win32 GDI interface...", e);
        let res = gui_win32::run_win32_gui();
        eprintln!("Win32 result: {:?}", res);
        res
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn run_platform_gui_with_files(files: Vec<PathBuf>) -> anyhow::Result<()> {
    hide_console_window_if_gui();
    if let Err(e) = gui::run_gui_with_files(files.clone()) {
        eprintln!("Failed to launch hardware eframe/WGPU GUI ({}). Falling back to native Win32 GDI interface...", e);
        let res = gui_win32::run_win32_gui_with_files(files);
        eprintln!("Win32 result: {:?}", res);
        res
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn run_platform_gui() -> anyhow::Result<()> {
    gui::run_gui()
}

#[cfg(not(windows))]
fn run_platform_gui_with_files(files: Vec<PathBuf>) -> anyhow::Result<()> {
    gui::run_gui_with_files(files)
}

/// Handler for 'info' command.
fn handle_info(path: &Path, verbose: bool) -> Result<()> {
    let intl = crate::i18n::for_language(crate::i18n::Language::process_default());
    let err_open = format_message!(
        &intl,
        default_message: "Failed to open file: {path}",
        description: "CLI error when input file cannot be opened",
        values: { path: path.display().to_string() }
    );
    let data = fs::read(path).with_context(|| err_open)?;

    println!(
        "{}",
        format_message!(&intl, default_message: "=== File Information ===", description: "CLI section header for file info")
    );
    println!(
        "{}",
        format_message!(&intl, default_message: "Path: {path}", description: "CLI info showing file path", values: { path: path.display().to_string() })
    );
    println!(
        "{}",
        format_message!(&intl, default_message: "Size: {bytes} bytes", description: "CLI info showing file size in bytes", values: { bytes: data.len().to_string() })
    );

    // Attempt to parse as HologramContainer
    if let Ok(container) = HologramContainer::parse(&data) {
        println!(
            "{}",
            format_message!(&intl, default_message: "Format: holographic fan container (112 LEDs, 450 lines)", description: "CLI info describing verified container format")
        );
        println!(
            "{}",
            format_message!(&intl, default_message: "Header signature: valid (00 11 22 33 44 55 66 77)", description: "CLI info validating header signature")
        );
        println!(
            "{}",
            format_message!(&intl, default_message: "Playlist signature: valid (77 66 55 44 33 22 11 00)", description: "CLI info validating playlist signature")
        );
        println!(
            "{}",
            format_message!(&intl, default_message: "Total frames: {frames}", description: "CLI info displaying total container frames", values: { frames: container.total_frames.to_string() })
        );
        println!(
            "{}",
            format_message!(&intl, default_message: "Frame size: {bytes} bytes", description: "CLI info displaying single frame size in bytes", values: { bytes: format::HOLOGRAM_FRAME_SIZE.to_string() })
        );
        println!(
            "{}",
            format_message!(&intl, default_message: "Playlist entries: {count}", description: "CLI info displaying count of playlist entries", values: { count: container.playlist.len().to_string() })
        );

        for (idx, entry) in container.playlist.iter().enumerate() {
            println!(
                "{}",
                format_message!(
                    &intl,
                    default_message: "  Entry #{idx}: frames {start}..{end} (total: {total}), speed: {speed}, mode: {mode}",
                    description: "CLI info describing single playlist entry",
                    values: {
                        idx: (idx + 1).to_string(),
                        start: entry.start_frame.to_string(),
                        end: entry.end_frame.to_string(),
                        total: (entry.end_frame.saturating_sub(entry.start_frame) + 1).to_string(),
                        speed: entry.speed.to_string(),
                        mode: entry.mode.to_string(),
                    }
                )
            );
        }

        if verbose {
            println!(
                "{}",
                format_message!(&intl, default_message: "Integrity: structure is fully valid.", description: "CLI info confirming container integrity")
            );
        }
        return Ok(());
    }

    println!(
        "{}",
        format_message!(&intl, default_message: "Format: unrecognized binary file", description: "CLI info when binary format is unknown")
    );
    bail!(
        "{}",
        format_message!(&intl, default_message: "File does not match holographic fan container format", description: "CLI error when file format is unrecognized")
    );
}

/// Handler for 'create' command.
fn handle_create(args: &CreateArgs, verbose: bool) -> Result<()> {
    let intl = crate::i18n::for_language(crate::i18n::Language::process_default());
    let config = PolarConfig::standard();
    if args.repeat == 0 {
        bail!("Static image repeat count must be at least 1");
    }
    let media_options = media::MediaLoadOptions {
        static_frame_count: args.repeat,
        ..media::MediaLoadOptions::default()
    };

    if verbose {
        eprintln!("[debug] Config: {:?}", config);
        eprintln!("[debug] Loading frames from: {}", args.input.display());
    }

    let frames = if args.input.is_dir() {
        let mut loaded = Vec::new();
        let mut paths = fs::read_dir(&args.input)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();

        for path in paths {
            if path.is_file() && is_supported_media_path(&path) {
                loaded.extend(
                    media::load_media_to_polar_frames_with_options(&path, &config, &media_options)
                        .with_context(|| format!("Failed to load media file {}", path.display()))?,
                );
            }
        }
        if loaded.is_empty() {
            bail!(
                "{}",
                format_message!(
                    &intl,
                    default_message: "No suitable images found in directory {path}",
                    description: "CLI error when image directory has no supported files",
                    values: { path: args.input.display().to_string() }
                )
            );
        }
        loaded
    } else {
        media::load_media_to_polar_frames_with_options(&args.input, &config, &media_options)?
    };

    let total_frames = frames.len();
    println!(
        "{}",
        format_message!(
            &intl,
            default_message: "Successfully prepared frames: {count}",
            description: "CLI info showing prepared frames count during creation",
            values: { count: total_frames.to_string() }
        )
    );

    let playlist = vec![PlaylistEntry {
        start_frame: 1,
        end_frame: total_frames as u16,
        speed: args.speed,
        mode: args.mode,
    }];
    let container = HologramContainer::new(frames, playlist);
    container.write_to_file(&args.output)?;
    println!(
        "{}",
        format_message!(
            &intl,
            default_message: "Container .bin successfully created: {path}",
            description: "CLI message when standard container is created",
            values: { path: args.output.display().to_string() }
        )
    );

    Ok(())
}

fn is_supported_media_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp")
    )
}

/// Handler for 'unpack' command.
fn handle_unpack(args: &UnpackArgs, _verbose: bool) -> Result<()> {
    let intl = crate::i18n::for_language(crate::i18n::Language::process_default());
    let err_open = format_message!(
        &intl,
        default_message: "Failed to open file: {path}",
        description: "CLI error when input file cannot be opened",
        values: { path: args.input.display().to_string() }
    );
    let data = fs::read(&args.input).with_context(|| err_open)?;

    let err_create_dir = format_message!(
        &intl,
        default_message: "Failed to create output directory: {path}",
        description: "CLI error when output directory creation fails",
        values: { path: args.output.display().to_string() }
    );
    fs::create_dir_all(&args.output).with_context(|| err_create_dir)?;

    // Check for standard container format
    if let Ok(container) = HologramContainer::parse(&data) {
        println!(
            "{}",
            format_message!(
                &intl,
                default_message: "Unpacking standard container ({frames} frames)...",
                description: "CLI message when unpacking standard container begins",
                values: { frames: container.frames.len().to_string() }
            )
        );
        let config = PolarConfig::standard();

        if args.raw {
            for (idx, frame) in container.frames.iter().enumerate() {
                let path = args.output.join(format!("frame_{:04}.raw", idx + 1));
                fs::write(&path, frame)?;
            }
        } else {
            media::export_polar_frames_to_png(&container.frames, &args.output, &config)?;
        }
        println!(
            "{}",
            format_message!(
                &intl,
                default_message: "Successfully extracted {frames} frames to {path}",
                description: "CLI message when frames extraction succeeds",
                values: {
                    frames: container.frames.len().to_string(),
                    path: args.output.display().to_string(),
                }
            )
        );
        return Ok(());
    }

    bail!(
        "{}",
        format_message!(
            &intl,
            default_message: "File {path} is not a recognized holographic fan container",
            description: "CLI error when input file is not a valid container",
            values: { path: args.input.display().to_string() }
        )
    );
}

/// Handler for 'convert' command.
fn handle_convert(args: &ConvertArgs, _verbose: bool) -> Result<()> {
    let intl = crate::i18n::for_language(crate::i18n::Language::process_default());
    let config = PolarConfig::standard();

    let err_open = format_message!(
        &intl,
        default_message: "Failed to open image: {path}",
        description: "CLI error when image cannot be opened",
        values: { path: args.input.display().to_string() }
    );
    let img = media::open_image(&args.input).with_context(|| err_open)?;

    let polar_data = polar::encode_image_to_polar(&img, &config);
    let reconstructed = polar::decode_polar_to_image(&polar_data, &config)?;

    let err_save = format_message!(
        &intl,
        default_message: "Failed to save preview: {path}",
        description: "CLI error when preview image cannot be saved",
        values: { path: args.output.display().to_string() }
    );
    reconstructed.save(&args.output).with_context(|| err_save)?;

    println!(
        "{}",
        format_message!(
            &intl,
            default_message: "Conversion complete: {input} -> {output}",
            description: "CLI message when image conversion completes",
            values: {
                input: args.input.display().to_string(),
                output: args.output.display().to_string(),
            }
        )
    );
    Ok(())
}

/// Handler for device USB commands.
fn handle_device(action: &DeviceSubcommands, verbose: bool) -> Result<()> {
    let intl = crate::i18n::for_language(crate::i18n::Language::process_default());
    match action {
        DeviceSubcommands::Status => {
            println!(
                "{}",
                format_message!(
                    &intl,
                    default_message: "Searching for fan device (VID: {vid}, PID: {pid})...",
                    description: "CLI message when scanning for USB HID device",
                    values: {
                        vid: format!("0x{:04x}", device::VENDOR_ID),
                        pid: format!("0x{:04x}", device::PRODUCT_ID),
                    }
                )
            );
            let dev = device::FanDevice::connect()?;
            let status = dev.query_status()?;
            println!(
                "{}",
                format_message!(
                    &intl,
                    default_message: "Device successfully detected and connected!",
                    description: "CLI message when fan device is connected"
                )
            );
            if verbose {
                eprintln!(
                    "[debug] Status reply (64 bytes): {:02x?}",
                    &status.raw_response[..]
                );
            }
        }
        DeviceSubcommands::Dump(args) => {
            run_legacy_dump(&args.output, verbose)?;
        }
    }
    Ok(())
}

/// Reads memory dump from connected device.
fn run_legacy_dump(output_path: &Path, verbose: bool) -> Result<()> {
    let intl = crate::i18n::for_language(crate::i18n::Language::process_default());
    println!(
        "{}",
        format_message!(
            &intl,
            default_message: "Connecting to fan...",
            description: "CLI message when dump connection starts"
        )
    );
    let dev = device::FanDevice::connect()?;
    let _ = dev.query_status()?;

    const HEADER_SIG: &[u8; 4] = b"FAN\0";
    let version: u32 = 1;
    let block_count: u32 = 60;
    let mut result: Vec<u8> = Vec::new();
    result.extend_from_slice(HEADER_SIG);
    result.extend_from_slice(&version.to_le_bytes());
    result.extend_from_slice(&block_count.to_le_bytes());

    for idx in 1u8..=60u8 {
        if verbose {
            eprintln!("[debug] Requesting block {}", idx);
        }
        let block_data = dev.read_block(idx)?;
        result.extend_from_slice(&block_data);
    }

    let err_save = format_message!(
        &intl,
        default_message: "Failed to save dump to {path}",
        description: "CLI error when saving device dump fails",
        values: { path: output_path.display().to_string() }
    );
    fs::write(output_path, &result).with_context(|| err_save)?;

    println!(
        "{}",
        format_message!(
            &intl,
            default_message: "Dump successfully saved: {path} ({bytes} bytes)",
            description: "CLI message when device dump is saved",
            values: {
                path: output_path.display().to_string(),
                bytes: result.len().to_string(),
            }
        )
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage};

    #[test]
    fn cli_keeps_positional_media_paths_for_gui_startup() {
        let cli = Cli::try_parse_from(["holofan", "sample.bin", "image.jpg"]).unwrap();
        assert_eq!(
            cli.files,
            vec![PathBuf::from("sample.bin"), PathBuf::from("image.jpg")]
        );
    }

    #[test]
    fn test_hologram_container_roundtrip_and_boundaries() {
        let mut fake_frame = vec![0u8; format::HOLOGRAM_FRAME_SIZE];
        fake_frame[0] = 0x33;
        let playlist = vec![PlaylistEntry {
            start_frame: 1,
            end_frame: 2,
            speed: 1,
            mode: 1,
        }];

        let container = HologramContainer::new(vec![fake_frame.clone(), fake_frame], playlist);
        let bytes = container.to_bytes().unwrap();
        let parsed = HologramContainer::parse(&bytes).expect("Failed to parse container");
        assert_eq!(parsed.total_frames, 2);
        assert_eq!(parsed.frames.len(), 2);
        assert_eq!(parsed.playlist.len(), 1);
        assert_eq!(parsed.to_bytes().unwrap(), bytes);

        // Boundary checks: truncated data
        assert!(HologramContainer::parse(&bytes[..100]).is_err());

        // Boundary checks: corrupted header signature
        let mut corrupted_header = bytes.clone();
        corrupted_header[0..8].fill(0xff);
        assert!(HologramContainer::parse(&corrupted_header).is_err());

        // Boundary checks: corrupted playlist trailer signature
        let mut corrupted_trailer = bytes.clone();
        corrupted_trailer[3024..3032].fill(0x00);
        assert!(HologramContainer::parse(&corrupted_trailer).is_err());

        // Trailing bytes and nonzero frame padding are not accepted.
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(HologramContainer::parse(&trailing).is_err());
        let mut bad_padding = bytes.clone();
        bad_padding[format::HOLOGRAM_HEADER_SIZE + polar::PolarConfig::ACTIVE_FRAME_SIZE] = 1;
        assert!(HologramContainer::parse(&bad_padding).is_err());

        let short_frame = HologramContainer::new(
            vec![vec![0; format::HOLOGRAM_FRAME_SIZE - 1]],
            vec![PlaylistEntry {
                start_frame: 1,
                end_frame: 1,
                speed: 1,
                mode: 1,
            }],
        );
        assert!(short_frame.to_bytes().is_err());
    }

    #[test]
    fn test_polar_encode_decode_roundtrip() {
        let mut img = RgbImage::new(300, 300);
        for x in 100..200 {
            for y in 100..200 {
                img.put_pixel(x, y, Rgb([255, 0, 0]));
            }
        }
        let dyn_img = DynamicImage::ImageRgb8(img);
        let config = PolarConfig::standard();

        let polar_frame = polar::encode_image_to_polar(&dyn_img, &config);
        assert_eq!(polar_frame.len(), config.frame_padded_size());

        let reconstructed = polar::decode_polar_to_image(&polar_frame, &config).unwrap();
        assert_eq!(reconstructed.width(), 450);
        assert_eq!(reconstructed.height(), 450);

        let center_pixel = reconstructed.get_pixel(225, 225);
        assert_eq!(center_pixel[0], 252);
    }

    #[test]
    fn test_hologram_container_file_io_hermetic() {
        use tempfile::NamedTempFile;

        let mut fake_frame = vec![0u8; format::HOLOGRAM_FRAME_SIZE];
        fake_frame[0] = 0x55;
        let playlist = vec![PlaylistEntry {
            start_frame: 1,
            end_frame: 1,
            speed: 1,
            mode: 1,
        }];
        let container = HologramContainer::new(vec![fake_frame], playlist);

        let temp = NamedTempFile::new().unwrap();
        let path = temp.path().with_extension("bin");
        container
            .write_to_file(&path)
            .expect("Failed to write to file");

        let loaded = HologramContainer::read_from_file(&path).expect("Failed to read from file");
        assert_eq!(loaded.total_frames, 1);
        assert_eq!(loaded.to_bytes().unwrap(), container.to_bytes().unwrap());
        let _ = fs::remove_file(path);
    }
}
