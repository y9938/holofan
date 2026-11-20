use formatjs_intl::format_message;
use holofan::i18n::{for_language, Language};

#[test]
fn test_all_languages_render_translated_text() {
    let en = Language::parse("en").expect("en locale exists");
    let ru = Language::parse("ru").expect("ru locale exists");
    let zh = Language::parse("zh").expect("zh locale exists");

    let intl_en = for_language(en);
    let intl_ru = for_language(ru);
    let intl_zh = for_language(zh);

    let speed_en = format_message!(
        &intl_en,
        id: "item-controller-speed",
        default_message: "Controller speed:",
        description: "Label for per-item controller playback speed"
    );
    let speed_ru = format_message!(
        &intl_ru,
        id: "item-controller-speed",
        default_message: "Controller speed:",
        description: "Label for per-item controller playback speed"
    );
    let speed_zh = format_message!(
        &intl_zh,
        id: "item-controller-speed",
        default_message: "Controller speed:",
        description: "Label for per-item controller playback speed"
    );
    assert_eq!(speed_en, "Controller speed:");
    assert_eq!(speed_ru, "Скорость контроллера:");
    assert_eq!(speed_zh, "控制器速度：");

    // 1. Initial status
    let msg_en = format_message!(
        &intl_en,
        default_message: "Ready. Add an image, GIF, WebP, or open a container file (.bin).",
        description: "Initial status bar message on application startup"
    );
    let msg_ru = format_message!(
        &intl_ru,
        default_message: "Ready. Add an image, GIF, WebP, or open a container file (.bin).",
        description: "Initial status bar message on application startup"
    );
    let msg_zh = format_message!(
        &intl_zh,
        default_message: "Ready. Add an image, GIF, WebP, or open a container file (.bin).",
        description: "Initial status bar message on application startup"
    );

    assert_eq!(
        msg_en,
        "Ready. Add an image, GIF, WebP, or open a container file (.bin)."
    );
    assert_eq!(
        msg_ru,
        "Готово к работе. Добавьте изображение, GIF, WebP или откройте файл контейнера (.bin)."
    );
    assert_eq!(msg_zh, "就绪。添加图像、GIF、WebP或打开容器文件 (.bin)。");

    // 2. Button add media
    let btn_en = format_message!(
        &intl_en,
        default_message: "➕ Add media (PNG/JPG/GIF/WebP)",
        description: "Button to open file picker for images or animations"
    );
    let btn_ru = format_message!(
        &intl_ru,
        default_message: "➕ Add media (PNG/JPG/GIF/WebP)",
        description: "Button to open file picker for images or animations"
    );
    let btn_zh = format_message!(
        &intl_zh,
        default_message: "➕ Add media (PNG/JPG/GIF/WebP)",
        description: "Button to open file picker for images or animations"
    );

    assert_eq!(btn_en, "➕ Add media (PNG/JPG/GIF/WebP)");
    assert_eq!(btn_ru, "➕ Добавить медиа (PNG/JPG/GIF/WebP)");
    assert_eq!(btn_zh, "➕ 添加媒体 (PNG/JPG/GIF/WebP)");

    // 3. Formatted message with values
    let val_en = format_message!(
        &intl_en,
        default_message: "File loaded: {title} (frames: {frames}, items: {items})",
        description: "Status message after loading a binary container file",
        values: {
            title: "demo.bin".to_string(),
            frames: "100".to_string(),
            items: "5".to_string(),
        }
    );
    let val_ru = format_message!(
        &intl_ru,
        default_message: "File loaded: {title} (frames: {frames}, items: {items})",
        description: "Status message after loading a binary container file",
        values: {
            title: "demo.bin".to_string(),
            frames: "100".to_string(),
            items: "5".to_string(),
        }
    );
    let val_zh = format_message!(
        &intl_zh,
        default_message: "File loaded: {title} (frames: {frames}, items: {items})",
        description: "Status message after loading a binary container file",
        values: {
            title: "demo.bin".to_string(),
            frames: "100".to_string(),
            items: "5".to_string(),
        }
    );

    assert_eq!(val_en, "File loaded: demo.bin (frames: 100, items: 5)");
    assert_eq!(
        val_ru,
        "Успешно загружен файл: demo.bin (кадров: 100, элементов: 5)"
    );
    assert_eq!(val_zh, "文件已加载: demo.bin (帧数: 100, 项目数: 5)");

    // 4. POV display preview and inspector headings
    let prev_en = format_message!(
        &intl_en,
        default_message: "🌀 POV display preview",
        description: "Heading for circular POV display preview area"
    );
    let prev_ru = format_message!(
        &intl_ru,
        default_message: "🌀 POV display preview",
        description: "Heading for circular POV display preview area"
    );
    let prev_zh = format_message!(
        &intl_zh,
        default_message: "🌀 POV display preview",
        description: "Heading for circular POV display preview area"
    );

    assert_eq!(prev_en, "🌀 POV display preview");
    assert_eq!(prev_ru, "🌀 Предпросмотр POV-дисплея");
    assert_eq!(prev_zh, "🌀 POV显示预览");

    // 5. The supported controller profile is fixed by the verified frame format.
    let profile_en = format_message!(
        &intl_en,
        id: "9eUNvMvjnj",
        default_message: "112 radial LEDs · 450 angular lines · 114,688 bytes per frame",
        description: "Verified controller geometry and encoded frame size"
    );
    let profile_ru = format_message!(
        &intl_ru,
        id: "9eUNvMvjnj",
        default_message: "112 radial LEDs · 450 angular lines · 114,688 bytes per frame",
        description: "Verified controller geometry and encoded frame size"
    );
    let profile_zh = format_message!(
        &intl_zh,
        id: "9eUNvMvjnj",
        default_message: "112 radial LEDs · 450 angular lines · 114,688 bytes per frame",
        description: "Verified controller geometry and encoded frame size"
    );

    assert_eq!(
        profile_en,
        "112 radial LEDs · 450 angular lines · 114,688 bytes per frame"
    );
    assert_eq!(
        profile_ru,
        "112 радиальных светодиодов · 450 линий · 114 688 байт на кадр"
    );
    assert_eq!(
        profile_zh,
        "112 个径向 LED · 450 条扫描线 · 每帧 114,688 字节"
    );
}
