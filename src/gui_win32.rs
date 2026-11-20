//! Native Windows GDI GUI fallback implementation.
//! Operates without requiring hardware 3D GPU acceleration (compatible with VMs, RDP, and legacy environments).
//! Supports playlist sequencing, media addition, cached POV preview, and removable media export.

#![cfg(windows)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::device::{FanDevice, PRODUCT_ID, VENDOR_ID};
use crate::format::HologramContainer;
use crate::media;
use crate::polar::{decode_polar_to_image, PolarConfig};
use crate::project::FanProject;

#[allow(dead_code)]
type HWND = *mut std::ffi::c_void;
#[allow(dead_code)]
type HDC = *mut std::ffi::c_void;
#[allow(dead_code)]
type HMENU = *mut std::ffi::c_void;
#[allow(dead_code)]
type HINSTANCE = *mut std::ffi::c_void;
#[allow(dead_code)]
type HBRUSH = *mut std::ffi::c_void;
#[allow(dead_code)]
type LPARAM = isize;
#[allow(dead_code)]
type WPARAM = usize;
#[allow(dead_code)]
type LRESULT = isize;

#[repr(C)]
struct WNDCLASSW {
    style: u32,
    lpfn_wnd_proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: HINSTANCE,
    h_icon: *mut std::ffi::c_void,
    h_cursor: *mut std::ffi::c_void,
    hbr_background: HBRUSH,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
}

#[repr(C)]
struct POINT {
    x: i32,
    y: i32,
}

#[repr(C)]
struct MSG {
    hwnd: HWND,
    message: u32,
    w_param: WPARAM,
    l_param: LPARAM,
    time: u32,
    pt: POINT,
}

#[repr(C)]
struct RECT {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct PAINTSTRUCT {
    hdc: HDC,
    f_erase: i32,
    rc_paint: RECT,
    f_restore: i32,
    f_inc_update: i32,
    rgb_reserved: [u8; 32],
}

#[repr(C)]
struct BITMAPINFOHEADER {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

#[repr(C)]
struct BITMAPINFO {
    bmi_header: BITMAPINFOHEADER,
    bmi_colors: [u32; 1],
}

#[repr(C)]
struct OPENFILENAMEW {
    l_struct_size: u32,
    hwnd_owner: HWND,
    h_instance: HINSTANCE,
    lpstr_filter: *const u16,
    lpstr_custom_filter: *mut u16,
    n_max_cust_filter: u32,
    n_filter_index: u32,
    lpstr_file: *mut u16,
    n_max_file: u32,
    lpstr_file_title: *mut u16,
    n_max_file_title: u32,
    lpstr_initial_dir: *const u16,
    lpstr_title: *const u16,
    flags: u32,
    n_file_offset: u16,
    n_file_extension: u16,
    lpstr_def_ext: *const u16,
    l_cust_data: LPARAM,
    lpfn_hook: *mut std::ffi::c_void,
    lp_template_name: *const u16,
    pv_reserved: *mut std::ffi::c_void,
    dw_reserved: u32,
    flags_ex: u32,
}

#[allow(dead_code)]
#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(lpWndClass: *const WNDCLASSW) -> u16;
    fn CreateWindowExW(
        dwExStyle: u32,
        lpClassName: *const u16,
        lpWindowName: *const u16,
        dwStyle: u32,
        X: i32,
        Y: i32,
        nWidth: i32,
        nHeight: i32,
        hWndParent: HWND,
        hMenu: HMENU,
        hInstance: HINSTANCE,
        lpParam: *mut std::ffi::c_void,
    ) -> HWND;
    fn ShowWindow(hWnd: HWND, nCmdShow: i32) -> i32;
    fn UpdateWindow(hWnd: HWND) -> i32;
    fn GetMessageW(lpMsg: *mut MSG, hWnd: HWND, wMsgFilterMin: u32, wMsgFilterMax: u32) -> i32;
    fn TranslateMessage(lpMsg: *const MSG) -> i32;
    fn DispatchMessageW(lpMsg: *const MSG) -> LRESULT;
    fn PostQuitMessage(nExitCode: i32);
    fn DefWindowProcW(hWnd: HWND, Msg: u32, wParam: WPARAM, lParam: LPARAM) -> LRESULT;
    fn BeginPaint(hWnd: HWND, lpPaint: *mut PAINTSTRUCT) -> HDC;
    fn EndPaint(hWnd: HWND, lpPaint: *const PAINTSTRUCT) -> i32;
    fn InvalidateRect(hWnd: HWND, lpRect: *const RECT, bErase: i32) -> i32;
    fn SetWindowTextW(hWnd: HWND, lpString: *const u16) -> i32;
    fn SendMessageW(hWnd: HWND, Msg: u32, wParam: WPARAM, lParam: LPARAM) -> LRESULT;
    fn SetTimer(
        hWnd: HWND,
        nIDEvent: usize,
        uElapse: u32,
        lpTimerFunc: *mut std::ffi::c_void,
    ) -> usize;
    fn KillTimer(hWnd: HWND, uIDEvent: usize) -> i32;
    fn GetModuleHandleW(lpModuleName: *const u16) -> HINSTANCE;
    fn MessageBoxW(hWnd: HWND, lpText: *const u16, lpCaption: *const u16, uType: u32) -> i32;
    fn LoadCursorW(hInstance: HINSTANCE, lpCursorName: *const u16) -> *mut std::ffi::c_void;
    fn LoadIconW(hInstance: HINSTANCE, lpIconName: *const u16) -> *mut std::ffi::c_void;
    fn LoadImageW(
        hInst: HINSTANCE,
        name: *const u16,
        type_: u32,
        cx: i32,
        cy: i32,
        fuLoad: u32,
    ) -> *mut std::ffi::c_void;
    fn GetSystemMetrics(nIndex: i32) -> i32;
}

#[allow(dead_code)]
#[link(name = "gdi32")]
extern "system" {
    fn CreateSolidBrush(color: u32) -> HBRUSH;
    fn DeleteObject(ho: *mut std::ffi::c_void) -> i32;
    fn SetDIBitsToDevice(
        hdc: HDC,
        xDest: i32,
        yDest: i32,
        w: u32,
        h: u32,
        xSrc: i32,
        ySrc: i32,
        StartScan: u32,
        cLines: u32,
        lpvBits: *const std::ffi::c_void,
        lpbmi: *const BITMAPINFO,
        ColorUse: u32,
    ) -> i32;
    fn TextOutW(hdc: HDC, x: i32, y: i32, lpString: *const u16, c: i32) -> i32;
    fn SetBkMode(hdc: HDC, mode: i32) -> i32;
}

#[link(name = "comdlg32")]
extern "system" {
    fn GetOpenFileNameW(lpofn: *mut OPENFILENAMEW) -> i32;
    fn GetSaveFileNameW(lpofn: *mut OPENFILENAMEW) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(pvReserved: *mut std::ffi::c_void, dwCoInit: u32) -> i32;
    fn CoUninitialize();
}

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF0000;
const WS_VISIBLE: u32 = 0x10000000;
const WS_CHILD: u32 = 0x40000000;
const WS_BORDER: u32 = 0x00800000;
const WS_VSCROLL: u32 = 0x00200000;
const BS_PUSHBUTTON: u32 = 0x00000000;
const SS_LEFT: u32 = 0x00000000;
const LBS_NOTIFY: u32 = 0x0001;
const IDC_ARROW: usize = 32512;
const WM_SETICON: u32 = 0x0080;
const ICON_SMALL: usize = 0;
const ICON_BIG: usize = 1;
const IMAGE_ICON: u32 = 1;
const LR_DEFAULTCOLOR: u32 = 0x0000;
const SM_CXSMICON: i32 = 49;
const SM_CYSMICON: i32 = 50;
#[allow(dead_code)]
const SM_CXICON: i32 = 11;
#[allow(dead_code)]
const SM_CYICON: i32 = 12;

const LB_ADDSTRING: u32 = 0x0180;
const LB_RESETCONTENT: u32 = 0x0184;
#[allow(dead_code)]
const LB_SETCURSEL: u32 = 0x0186;
const LB_GETCURSEL: u32 = 0x0188;
const LBN_SELCHANGE: usize = 1;

const ID_BTN_ADD_MEDIA: usize = 1001;
const ID_BTN_OPEN_BIN: usize = 1002;
const ID_BTN_SAVE_BIN: usize = 1003;
const ID_BTN_DOWN_USB: usize = 1004;
const ID_BTN_EXPORT_PNG: usize = 1005;
const ID_BTN_CHECK_USB: usize = 1006;
const ID_BTN_DEL_ITEM: usize = 1007;
const ID_BTN_PLAY_PAUSE: usize = 1008;
const ID_BTN_PREV_FRAME: usize = 1009;
const ID_BTN_NEXT_FRAME: usize = 1010;
const ID_LIST_PLAYLIST: usize = 1011;

const TIMER_ANIMATION: usize = 1;

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Application state for native Win32 GUI
#[allow(dead_code)]
struct AppState {
    hwnd_main: HWND,
    hwnd_status: HWND,
    hwnd_info: HWND,
    hwnd_list: HWND,
    project: FanProject,
    current_frame: usize,
    is_playing: bool,
    status_text: String,
    // Cached decoded raster frame for flicker-free rendering
    cached_frame_idx: Option<usize>,
    cached_bgra: Vec<u8>,
}

unsafe impl Send for AppState {}
unsafe impl Sync for AppState {}

static STATE: Mutex<Option<AppState>> = Mutex::new(None);

pub fn run_win32_gui() -> anyhow::Result<()> {
    run_win32_gui_with_files(Vec::new())
}

pub fn run_win32_gui_with_files(initial_files: Vec<PathBuf>) -> anyhow::Result<()> {
    unsafe {
        // Initialize COM for Shell dialogs in STA mode
        CoInitializeEx(std::ptr::null_mut(), 2 /* COINIT_APARTMENTTHREADED */);

        let instance = GetModuleHandleW(std::ptr::null());
        let class_name = to_wide("HoloFanRustWindowClass");
        let title = to_wide("HoloFan — 3D Holographic LED Display Controller");

        // Load application icon from embedded PE resources (resource ID 1)
        let h_icon_big = LoadIconW(instance, 1usize as *const u16);
        let h_icon_sm = LoadImageW(
            instance,
            1usize as *const u16,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        );

        let wnd_class = WNDCLASSW {
            style: 0x0003, // CS_HREDRAW | CS_VREDRAW
            lpfn_wnd_proc: wnd_proc,
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: instance,
            h_icon: if !h_icon_big.is_null() {
                h_icon_big
            } else {
                std::ptr::null_mut()
            },
            h_cursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16),
            hbr_background: CreateSolidBrush(0x00F0F0F0),
            lpsz_menu_name: std::ptr::null(),
            lpsz_class_name: class_name.as_ptr(),
        };

        RegisterClassW(&wnd_class);

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            100,
            100,
            980,
            660,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        );

        if !hwnd.is_null() {
            if !h_icon_sm.is_null() {
                SendMessageW(hwnd, WM_SETICON, ICON_SMALL as WPARAM, h_icon_sm as LPARAM);
            }
            if !h_icon_big.is_null() {
                SendMessageW(hwnd, WM_SETICON, ICON_BIG as WPARAM, h_icon_big as LPARAM);
            }
        }

        if hwnd.is_null() {
            anyhow::bail!("Не удалось создать главное окно Win32");
        }

        // Initialize application state
        {
            let mut state = STATE.lock().unwrap();
            let mut app = AppState {
                hwnd_main: hwnd,
                hwnd_status: std::ptr::null_mut(),
                hwnd_info: std::ptr::null_mut(),
                hwnd_list: std::ptr::null_mut(),
                project: FanProject::new(),
                current_frame: 0,
                is_playing: false,
                status_text: "Готово к работе. Добавьте медиафайлы или откройте файл .bin."
                    .to_string(),
                cached_frame_idx: None,
                cached_bgra: Vec::new(),
            };

            if initial_files.is_empty() {
                // Auto-load the default container only when no command-line files were given.
                let default_bin = PathBuf::from("hologram.bin");
                if default_bin.exists() {
                    match HologramContainer::read_from_file(&default_bin) {
                        Ok(container) => match app.project.load_from_container(container) {
                            Ok(()) => {
                                app.status_text = format!(
                                    "Загружен hologram.bin (кадров: {})",
                                    app.project.total_frames()
                                );
                            }
                            Err(error) => {
                                app.status_text =
                                    format!("Ошибка декодирования hologram.bin: {error:#}");
                            }
                        },
                        Err(error) => {
                            app.status_text = format!("Ошибка чтения hologram.bin: {error:#}");
                        }
                    }
                }
            } else {
                for path in &initial_files {
                    let extension = path
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let result = if extension == "bin" {
                        HologramContainer::read_from_file(path).and_then(|container| {
                            let count = container.frames.len();
                            app.project.load_from_container(container)?;
                            app.current_frame = 0;
                            app.project.output_filename = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            app.status_text =
                                format!("Успешно загружен {}: {} кадров", path.display(), count);
                            Ok(())
                        })
                    } else {
                        app.project.add_media_file(path, 15).map(|count| {
                            if app.project.item_count() == 1 {
                                if let Some(stem) = path.file_stem() {
                                    app.project.output_filename =
                                        format!("{}.bin", stem.to_string_lossy());
                                }
                            }
                            app.status_text = format!(
                                "Добавлен файл: +{} кадров (всего: {})",
                                count,
                                app.project.total_frames()
                            );
                        })
                    };
                    if let Err(error) = result {
                        app.status_text = format!("Ошибка загрузки {}: {error:#}", path.display());
                    }
                }
            }

            *state = Some(app);
        }

        // Create child control windows
        create_controls(hwnd, instance);

        // Update initial preview cache and playlist listbox
        {
            let mut state = STATE.lock().unwrap();
            if let Some(s) = state.as_mut() {
                update_cache_and_list(s);
            }
        }

        ShowWindow(hwnd, 1);
        UpdateWindow(hwnd);

        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        CoUninitialize();
        Ok(())
    }
}

unsafe fn create_controls(parent: HWND, instance: HINSTANCE) {
    let btn_class = to_wide("BUTTON");
    let static_class = to_wide("STATIC");
    let list_class = to_wide("LISTBOX");

    // Top action toolbar buttons
    create_btn(
        parent,
        instance,
        &btn_class,
        "➕ Добавить медиа (PNG/JPG/GIF/WebP)",
        20,
        15,
        230,
        34,
        ID_BTN_ADD_MEDIA,
    );
    create_btn(
        parent,
        instance,
        &btn_class,
        "📂 Открыть .bin",
        260,
        15,
        150,
        34,
        ID_BTN_OPEN_BIN,
    );
    create_btn(
        parent,
        instance,
        &btn_class,
        "💾 Сохранить .bin...",
        420,
        15,
        160,
        34,
        ID_BTN_SAVE_BIN,
    );
    create_btn(
        parent,
        instance,
        &btn_class,
        "⚡ Записать на SD/USB",
        590,
        15,
        180,
        34,
        ID_BTN_DOWN_USB,
    );
    create_btn(
        parent,
        instance,
        &btn_class,
        "🔌 Связь (HID)",
        780,
        15,
        160,
        34,
        ID_BTN_CHECK_USB,
    );

    // Left column: Playlist controls
    let playlist_label = to_wide("Элементы плейлиста:");
    CreateWindowExW(
        0,
        static_class.as_ptr(),
        playlist_label.as_ptr(),
        WS_CHILD | WS_VISIBLE,
        20,
        60,
        260,
        20,
        parent,
        std::ptr::null_mut(),
        instance,
        std::ptr::null_mut(),
    );

    let hwnd_list = CreateWindowExW(
        0,
        list_class.as_ptr(),
        std::ptr::null(),
        WS_CHILD | WS_VISIBLE | WS_BORDER | WS_VSCROLL | LBS_NOTIFY,
        20,
        85,
        260,
        280,
        parent,
        ID_LIST_PLAYLIST as HMENU,
        instance,
        std::ptr::null_mut(),
    );

    create_btn(
        parent,
        instance,
        &btn_class,
        "🗑 Удалить элемент",
        20,
        375,
        130,
        30,
        ID_BTN_DEL_ITEM,
    );
    create_btn(
        parent,
        instance,
        &btn_class,
        "🖼 Экспорт в PNG",
        155,
        375,
        125,
        30,
        ID_BTN_EXPORT_PNG,
    );

    // Static text info label
    let hwnd_info = CreateWindowExW(
        0,
        static_class.as_ptr(),
        to_wide("Информация:\nНет данных").as_ptr(),
        WS_CHILD | WS_VISIBLE | SS_LEFT | WS_BORDER,
        20,
        415,
        260,
        155,
        parent,
        std::ptr::null_mut(),
        instance,
        std::ptr::null_mut(),
    );

    // Playback transport buttons (below preview canvas)
    create_btn(
        parent,
        instance,
        &btn_class,
        "◀ Пред.",
        360,
        420,
        80,
        34,
        ID_BTN_PREV_FRAME,
    );
    create_btn(
        parent,
        instance,
        &btn_class,
        "▶/⏸ Пуск/Пауза",
        450,
        420,
        140,
        34,
        ID_BTN_PLAY_PAUSE,
    );
    create_btn(
        parent,
        instance,
        &btn_class,
        "След. ▶",
        600,
        420,
        80,
        34,
        ID_BTN_NEXT_FRAME,
    );

    // Bottom status bar
    let hwnd_status = CreateWindowExW(
        0,
        static_class.as_ptr(),
        to_wide("Готово к работе.").as_ptr(),
        WS_CHILD | WS_VISIBLE | SS_LEFT | WS_BORDER,
        10,
        585,
        945,
        28,
        parent,
        std::ptr::null_mut(),
        instance,
        std::ptr::null_mut(),
    );

    let mut state = STATE.lock().unwrap();
    if let Some(s) = state.as_mut() {
        s.hwnd_list = hwnd_list;
        s.hwnd_info = hwnd_info;
        s.hwnd_status = hwnd_status;
    }
}

unsafe fn create_btn(
    parent: HWND,
    inst: HINSTANCE,
    cls: &[u16],
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: usize,
) {
    let text_w = to_wide(text);
    CreateWindowExW(
        0,
        cls.as_ptr(),
        text_w.as_ptr(),
        WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
        x,
        y,
        w,
        h,
        parent,
        id as HMENU,
        inst,
        std::ptr::null_mut(),
    );
}

/// Updates decoded preview cache and refreshes ListBox entries
fn update_cache_and_list(s: &mut AppState) {
    let total = s.project.total_frames();
    if total == 0 {
        s.cached_frame_idx = None;
        s.cached_bgra.clear();
    } else {
        if s.current_frame >= total {
            s.current_frame = 0;
        }

        if s.cached_frame_idx != Some(s.current_frame) {
            let frame_data = &s.project.frames[s.current_frame];
            let reconstructed = match decode_polar_to_image(frame_data, &s.project.config) {
                Ok(image) => image,
                Err(error) => {
                    s.cached_bgra.clear();
                    s.cached_frame_idx = None;
                    s.status_text = format!("Ошибка декодирования предпросмотра: {error:#}");
                    return;
                }
            };
            const PREVIEW_SIZE: u32 = 300;
            let display = image::imageops::resize(
                &reconstructed,
                PREVIEW_SIZE,
                PREVIEW_SIZE,
                image::imageops::FilterType::Triangle,
            );
            let width = PREVIEW_SIZE as usize;
            let height = PREVIEW_SIZE as usize;
            let mut bgra = vec![0u8; width * height * 4];
            for y in 0..height {
                for x in 0..width {
                    let p = display.get_pixel(x as u32, (height - 1 - y) as u32);
                    let idx = (y * width + x) * 4;
                    bgra[idx] = p[2]; // Blue
                    bgra[idx + 1] = p[1]; // Green
                    bgra[idx + 2] = p[0]; // Red
                    bgra[idx + 3] = 255;
                }
            }
            s.cached_bgra = bgra;
            s.cached_frame_idx = Some(s.current_frame);
        }
    }

    // Populate ListBox entries
    unsafe {
        if !s.hwnd_list.is_null() {
            SendMessageW(s.hwnd_list, LB_RESETCONTENT, 0, 0);
            for (idx, item) in s.project.items.iter().enumerate() {
                let text = format!(
                    "#{}: {} [кадры {}-{}]",
                    idx + 1,
                    item.title,
                    item.frame_start + 1,
                    item.frame_start + item.frame_count
                );
                let text_w = to_wide(&text);
                SendMessageW(s.hwnd_list, LB_ADDSTRING, 0, text_w.as_ptr() as LPARAM);
            }
        }
    }

    update_info_label(s);
}

fn update_info_label(s: &AppState) {
    let total = s.project.total_frames();
    let text = format!(
        "Всего кадров: {}\nТекущий кадр: {} / {}\nЭлементов в плейлисте: {}\nФормат: 112 LED x 450 линий\nСъемные диски: {:?}",
        total,
        if total == 0 { 0 } else { s.current_frame + 1 },
        total,
        s.project.item_count(),
        FanProject::find_removable_drives()
    );
    unsafe {
        if !s.hwnd_info.is_null() {
            let text_w = to_wide(&text);
            SetWindowTextW(s.hwnd_info, text_w.as_ptr());
        }
    }
}

fn set_status(text: &str) {
    let mut state = STATE.lock().unwrap();
    if let Some(s) = state.as_mut() {
        s.status_text = text.to_string();
        unsafe {
            if !s.hwnd_status.is_null() {
                let text_w = to_wide(text);
                SetWindowTextW(s.hwnd_status, text_w.as_ptr());
            }
        }
    }
}

/// Native Win32 OpenFileDialog via comdlg32
unsafe fn win32_open_file(owner: HWND, filter: &str, title: &str) -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    let filter_w = to_wide_filter(filter);
    let title_w = to_wide(title);

    let mut ofn: OPENFILENAMEW = std::mem::zeroed();
    ofn.l_struct_size = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwnd_owner = owner;
    ofn.lpstr_filter = filter_w.as_ptr();
    ofn.lpstr_file = buf.as_mut_ptr();
    ofn.n_max_file = buf.len() as u32;
    ofn.lpstr_title = title_w.as_ptr();
    ofn.flags = 0x00080000 | 0x00001000 | 0x00000800; // OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST

    if GetOpenFileNameW(&mut ofn) != 0 {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let path_str = String::from_utf16_lossy(&buf[..len]);
        Some(PathBuf::from(path_str))
    } else {
        None
    }
}

/// Native Win32 SaveFileDialog via comdlg32
unsafe fn win32_save_file(
    owner: HWND,
    default_name: &str,
    filter: &str,
    title: &str,
) -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    let def_w = to_wide(default_name);
    for (i, &c) in def_w.iter().enumerate().take(buf.len() - 1) {
        buf[i] = c;
    }

    let filter_w = to_wide_filter(filter);
    let title_w = to_wide(title);

    let mut ofn: OPENFILENAMEW = std::mem::zeroed();
    ofn.l_struct_size = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwnd_owner = owner;
    ofn.lpstr_filter = filter_w.as_ptr();
    ofn.lpstr_file = buf.as_mut_ptr();
    ofn.n_max_file = buf.len() as u32;
    ofn.lpstr_title = title_w.as_ptr();
    ofn.flags = 0x00080000 | 0x00000002 | 0x00000800; // OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST

    if GetSaveFileNameW(&mut ofn) != 0 {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let path_str = String::from_utf16_lossy(&buf[..len]);
        Some(PathBuf::from(path_str))
    } else {
        None
    }
}

fn to_wide_filter(filter: &str) -> Vec<u16> {
    // Format Win32 filter string: "Description\0*.extension\0\0"
    let mut out = Vec::new();
    for part in filter.split('|') {
        out.extend(part.encode_utf16());
        out.push(0);
    }
    out.push(0);
    out
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    match msg {
        0x000F => {
            // WM_PAINT: instant render of cached frame to DC
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);

            let preview_title = to_wide("Предпросмотр дисплея вентилятора (450×450):");
            SetBkMode(hdc, 1);
            TextOutW(
                hdc,
                350,
                65,
                preview_title.as_ptr(),
                preview_title.len() as i32 - 1,
            );

            let state = STATE.lock().unwrap();
            if let Some(s) = state.as_ref() {
                if !s.cached_bgra.is_empty() {
                    let width = 300;
                    let height = 300;
                    let bmi = BITMAPINFO {
                        bmi_header: BITMAPINFOHEADER {
                            bi_size: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            bi_width: width as i32,
                            bi_height: height as i32,
                            bi_planes: 1,
                            bi_bit_count: 32,
                            bi_compression: 0,
                            bi_size_image: (width * height * 4) as u32,
                            bi_x_pels_per_meter: 0,
                            bi_y_pels_per_meter: 0,
                            bi_clr_used: 0,
                            bi_clr_important: 0,
                        },
                        bmi_colors: [0],
                    };

                    SetDIBitsToDevice(
                        hdc,
                        350,
                        95,
                        width as u32,
                        height as u32,
                        0,
                        0,
                        0,
                        height as u32,
                        s.cached_bgra.as_ptr() as *const _,
                        &bmi,
                        0,
                    );
                } else {
                    let empty_text = to_wide("[ Нет кадров. Добавьте медиафайлы или файл .bin ]");
                    TextOutW(
                        hdc,
                        360,
                        220,
                        empty_text.as_ptr(),
                        empty_text.len() as i32 - 1,
                    );
                }
            }

            EndPaint(hwnd, &ps);
            0
        }
        0x0111 => {
            // WM_COMMAND: button clicks and listbox selection change
            let id = w_param & 0xFFFF;
            let code = (w_param >> 16) & 0xFFFF;

            if id == ID_LIST_PLAYLIST && code == LBN_SELCHANGE {
                let mut state = STATE.lock().unwrap();
                if let Some(s) = state.as_mut() {
                    let sel = SendMessageW(s.hwnd_list, LB_GETCURSEL, 0, 0);
                    if sel >= 0 {
                        let idx = sel as usize;
                        if let Some(item) = s.project.items.get(idx) {
                            s.current_frame = item.frame_start;
                            update_cache_and_list(s);
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                    }
                }
                return 0;
            }

            handle_button_click(hwnd, id);
            0
        }
        0x0113 => {
            // WM_TIMER: animation frame tick
            if w_param == TIMER_ANIMATION {
                let mut state = STATE.lock().unwrap();
                if let Some(s) = state.as_mut() {
                    let total = s.project.total_frames();
                    if s.is_playing && total > 0 {
                        s.current_frame = (s.current_frame + 1) % total;
                        update_cache_and_list(s);
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
            }
            0
        }
        0x0002 => {
            // WM_DESTROY
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w_param, l_param),
    }
}

unsafe fn handle_button_click(hwnd: HWND, btn_id: usize) {
    match btn_id {
        ID_BTN_ADD_MEDIA => {
            let filter = "Медиафайлы (*.png;*.jpg;*.gif;*.webp)|*.png;*.jpg;*.jpeg;*.bmp;*.gif;*.webp|Все файлы (*.*)|*.*";
            if let Some(path) = win32_open_file(
                hwnd,
                filter,
                "Выберите изображение, GIF или WebP для добавления",
            ) {
                let mut state = STATE.lock().unwrap();
                if let Some(s) = state.as_mut() {
                    match s.project.add_media_file(&path, 15) {
                        Ok(count) => {
                            let total = s.project.total_frames();
                            if s.project.item_count() == 1 {
                                if let Some(stem) = path.file_stem() {
                                    s.project.output_filename =
                                        format!("{}.bin", stem.to_string_lossy());
                                }
                            }
                            s.status_text =
                                format!("Добавлен файл: +{} кадров (всего: {})", count, total);
                            update_cache_and_list(s);
                            InvalidateRect(hwnd, std::ptr::null(), 1);
                        }
                        Err(e) => {
                            s.status_text = format!("Ошибка добавления: {}", e);
                        }
                    }
                }
            }
        }
        ID_BTN_OPEN_BIN => {
            let filter = "Файлы вентилятора (*.bin)|*.bin|Все файлы (*.*)|*.*";
            if let Some(path) = win32_open_file(hwnd, filter, "Открыть файл .bin") {
                match HologramContainer::read_from_file(&path) {
                    Ok(c) => {
                        let mut state = STATE.lock().unwrap();
                        if let Some(s) = state.as_mut() {
                            let count = c.frames.len();
                            let filename = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            s.project.output_filename = filename.clone();
                            match s.project.load_from_container(c) {
                                Ok(()) => {
                                    s.current_frame = 0;
                                    s.status_text =
                                        format!("Успешно загружен {}: {} кадров", filename, count);
                                    update_cache_and_list(s);
                                    InvalidateRect(hwnd, std::ptr::null(), 1);
                                }
                                Err(error) => {
                                    s.status_text =
                                        format!("Ошибка декодирования {}: {error:#}", filename);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        set_status(&format!("Ошибка чтения .bin: {}", e));
                    }
                }
            }
        }
        ID_BTN_SAVE_BIN => {
            let state = STATE.lock().unwrap();
            let (export_res, cur_name) = state
                .as_ref()
                .map(|s| {
                    (
                        s.project.export_container(),
                        s.project.output_filename.clone(),
                    )
                })
                .unwrap_or((
                    Err(anyhow::anyhow!("Нет данных")),
                    "hologram.bin".to_string(),
                ));
            drop(state);

            match export_res {
                Ok(container) => {
                    let filter = "Файлы вентилятора (*.bin)|*.bin|Все файлы (*.*)|*.*";
                    if let Some(path) =
                        win32_save_file(hwnd, &cur_name, filter, "Сохранить файл .bin")
                    {
                        match container.write_to_file(&path) {
                            Ok(_) => {
                                if let Some(name) = path.file_name() {
                                    let mut state = STATE.lock().unwrap();
                                    if let Some(s) = state.as_mut() {
                                        s.project.output_filename =
                                            name.to_string_lossy().to_string();
                                    }
                                }
                                set_status(&format!("Файл успешно сохранен: {}", path.display()));
                            }
                            Err(e) => {
                                set_status(&format!("Ошибка сохранения: {}", e));
                            }
                        }
                    }
                }
                Err(e) => {
                    set_status(&format!("Ошибка подготовки проекта: {}", e));
                }
            }
        }
        ID_BTN_DOWN_USB => {
            let state = STATE.lock().unwrap();
            let (export_res, filename) = state
                .as_ref()
                .map(|s| {
                    (
                        s.project.export_container(),
                        s.project.output_filename.clone(),
                    )
                })
                .unwrap_or((
                    Err(anyhow::anyhow!("Нет данных")),
                    "hologram.bin".to_string(),
                ));
            drop(state);

            match export_res {
                Ok(container) => {
                    let drives = FanProject::find_removable_drives();
                    if let Some(first) = drives.first() {
                        let target = first.join(&filename);
                        match container.write_to_file(&target) {
                            Ok(_) => {
                                set_status(&format!(
                                    "Успешно записано на SD-карту: {}",
                                    target.display()
                                ));
                            }
                            Err(e) => {
                                set_status(&format!("Ошибка записи на SD: {}", e));
                            }
                        }
                    } else {
                        let filter = "Файлы вентилятора (*.bin)|*.bin|Все файлы (*.*)|*.*";
                        if let Some(path) = win32_save_file(
                            hwnd,
                            &filename,
                            filter,
                            "Выберите корень SD-карты для записи",
                        ) {
                            let _ = container.write_to_file(&path);
                            set_status(&format!("Успешно записано в: {}", path.display()));
                        }
                    }
                }
                Err(e) => {
                    set_status(&format!("Ошибка экспорта: {}", e));
                }
            }
        }
        ID_BTN_DEL_ITEM => {
            let mut state = STATE.lock().unwrap();
            if let Some(s) = state.as_mut() {
                let sel = SendMessageW(s.hwnd_list, LB_GETCURSEL, 0, 0);
                if sel >= 0 {
                    let idx = sel as usize;
                    s.project.remove_item(idx);
                    s.status_text = "Элемент удален из плейлиста".to_string();
                    update_cache_and_list(s);
                    InvalidateRect(hwnd, std::ptr::null(), 1);
                }
            }
        }
        ID_BTN_EXPORT_PNG => {
            let state = STATE.lock().unwrap();
            let (frames, config) = if let Some(s) = state.as_ref() {
                (s.project.frames.clone(), s.project.config.clone())
            } else {
                (Vec::new(), PolarConfig::standard())
            };
            drop(state);

            if frames.is_empty() {
                set_status("Нет кадров для экспорта!");
                return;
            }

            let export_dir = PathBuf::from("exported_frames");
            match media::export_polar_frames_to_png(&frames, &export_dir, &config) {
                Ok(n) => {
                    set_status(&format!(
                        "Экспортировано {} кадров в папку 'exported_frames'",
                        n
                    ));
                }
                Err(e) => {
                    set_status(&format!("Ошибка экспорта: {}", e));
                }
            }
        }
        ID_BTN_PLAY_PAUSE => {
            let mut state = STATE.lock().unwrap();
            if let Some(s) = state.as_mut() {
                s.is_playing = !s.is_playing;
                if s.is_playing {
                    SetTimer(hwnd, TIMER_ANIMATION, 66, std::ptr::null_mut()); // ~15 FPS
                    s.status_text = "Воспроизведение запущено...".to_string();
                } else {
                    KillTimer(hwnd, TIMER_ANIMATION);
                    s.status_text = "Воспроизведение приостановлено.".to_string();
                }
                update_info_label(s);
            }
        }
        ID_BTN_PREV_FRAME => {
            let mut state = STATE.lock().unwrap();
            if let Some(s) = state.as_mut() {
                let total = s.project.total_frames();
                if total > 0 && s.current_frame > 0 {
                    s.current_frame -= 1;
                    update_cache_and_list(s);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
        }
        ID_BTN_NEXT_FRAME => {
            let mut state = STATE.lock().unwrap();
            if let Some(s) = state.as_mut() {
                let total = s.project.total_frames();
                if total > 0 && s.current_frame + 1 < total {
                    s.current_frame += 1;
                    update_cache_and_list(s);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
        }
        ID_BTN_CHECK_USB => match FanDevice::connect() {
            Ok(dev) => match dev.query_status() {
                Ok(_) => {
                    set_status(&format!(
                        "Вентилятор подключен по USB HID (VID: {:#06x}, PID: {:#06x})",
                        VENDOR_ID, PRODUCT_ID
                    ));
                }
                Err(e) => {
                    set_status(&format!("Ответ вентилятора: {}", e));
                }
            },
            Err(e) => {
                set_status(&format!("Устройство не подключено: {}", e));
            }
        },
        _ => {}
    }
}
