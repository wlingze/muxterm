//! 剪贴板图片 → PNG。
//!
//! Core 的 `muxterm_image_paste_start_json` 只收 PNG（传输、远端落盘、路径
//! 回写都以 PNG 为契约）。macOS 的剪贴板几乎都是 TIFF（截图、Preview、
//! 浏览器），直接用 `ImageFormat::Tiff` 的字节调 Core 只会失败，而且失败
//! 之前用户什么提示都看不到 —— 这就是「复制图片粘贴没反应」的根因。
//!
//! 这里统一在**前端边界**转成 PNG，转换用 AppKit 自己的解码器，格式覆盖
//! TIFF / JPEG / GIF / BMP / WebP。其它平台把原样 PNG 透传。

use gpui::ImageFormat;

/// 把剪贴板图片字节转成 PNG。已经是 PNG 的原样返回。
pub fn to_png(format: ImageFormat, bytes: &[u8]) -> Option<Vec<u8>> {
    if format == ImageFormat::Png {
        return Some(bytes.to_vec());
    }
    convert(format, bytes)
}

/// 直接读系统剪贴板里的 PNG / TIFF，返回转换后的 PNG 或可展示的错误。
///
/// 为什么不用 gpui 的 `read_from_clipboard`：它**先看 UTF-8 文本**，剪贴板
/// 同时带文本时（浏览器复制图片、Keynote / Pages 等）只返回字符串，图片就
/// 被当成文本粘进终端。Swift `MuxTerminalView.paste(from:)` 的顺序相反：
/// 先 `availableType(from: [.png, .tiff])`，有图就粘图。这里按同一优先级。
#[cfg(target_os = "macos")]
pub fn clipboard_image_png() -> Option<Result<Vec<u8>, String>> {
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::NSPasteboard;

    // `NSPasteboard` 不是线程安全的：无头测试会并行跑到这条路径，不加锁
    // 会偶发 SIGSEGV（真实 app 只有主线程读，锁成本可以忽略）。
    let _guard = PASTEBOARD_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    autoreleasepool(|_| read_image_png_from(NSPasteboard::generalPasteboard()))
}

#[cfg(target_os = "macos")]
static PASTEBOARD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// AppKit 图片解码锁。与 `PASTEBOARD_LOCK` 互不嵌套（解码不再去读 pasteboard）。
#[cfg(target_os = "macos")]
static DECODE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 从指定 pasteboard 读 PNG / TIFF（测试用独立 pasteboard，不动用户剪贴板）。
#[cfg(target_os = "macos")]
pub fn read_image_png_from(
    pasteboard: objc2::rc::Retained<objc2_app_kit::NSPasteboard>,
) -> Option<Result<Vec<u8>, String>> {
    use objc2_app_kit::{NSPasteboardTypePNG, NSPasteboardTypeTIFF};

    // 外部静态必须放在 unsafe 里取一次。
    let png_type = unsafe { NSPasteboardTypePNG };
    let tiff_type = unsafe { NSPasteboardTypeTIFF };

    // PNG 优先：已是 Core 要的格式，不用再编码。
    if let Some(data) = pasteboard.dataForType(png_type) {
        if data.is_empty() {
            return Some(Err("clipboard image is empty".into()));
        }
        return Some(Ok(data.to_vec()));
    }
    let data = pasteboard.dataForType(tiff_type)?;
    if data.is_empty() {
        return Some(Err("clipboard image is empty".into()));
    }
    Some(
        to_png(ImageFormat::Tiff, &data.to_vec())
            .ok_or_else(|| "clipboard image could not be decoded".to_string()),
    )
}

#[cfg(target_os = "macos")]
fn convert(_format: ImageFormat, bytes: &[u8]) -> Option<Vec<u8>> {
    use objc2::{rc::autoreleasepool, AnyThread};
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
    use objc2_foundation::{NSData, NSDictionary};

    if bytes.is_empty() {
        return None;
    }
    // `NSBitmapImageRep` 解码同样不能并行跑：AppKit 的 image rep 缓存不是
    // 线程安全的。这里用**另一把**锁：`clipboard_image_png` 已经持有
    // `PASTEBOARD_LOCK` 时还会走到这里转码，共用一把非重入锁会自死锁。
    let _guard = DECODE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    autoreleasepool(|_| {
        let data = NSData::with_bytes(bytes);
        let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &data)?;
        // 空字典：让 AppKit 用默认的 PNG 编码参数。
        let properties = NSDictionary::new();
        let png = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
        }?;
        Some(png.to_vec())
    })
}

#[cfg(not(target_os = "macos"))]
fn convert(_format: ImageFormat, _bytes: &[u8]) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PNG 必须原样透传（Core 侧不做二次编码）。
    #[test]
    fn png_bytes_pass_through_untouched() {
        let bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
        assert_eq!(to_png(ImageFormat::Png, &bytes), Some(bytes));
    }

    /// 空字节不能当成图片。
    #[test]
    fn empty_input_is_rejected() {
        assert_eq!(to_png(ImageFormat::Tiff, &[]), None);
    }

    /// 截图的 TIFF 必须真的转成 PNG（这是 mac 上最常见的剪贴板格式）。
    #[cfg(target_os = "macos")]
    #[test]
    fn tiff_from_the_clipboard_becomes_png() {
        let tiff = tiny_tiff();
        let png = to_png(ImageFormat::Tiff, &tiff).expect("appkit 应该能解码 TIFF");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert!(png.len() > 20, "PNG 至少有头 + IHDR");
    }

    /// 坏字节不能 panic，返回 None 让上层给失败横幅。
    #[cfg(target_os = "macos")]
    #[test]
    fn garbage_bytes_fail_without_panicking() {
        assert_eq!(to_png(ImageFormat::Tiff, &[1, 2, 3, 4]), None);
    }
}

#[cfg(test)]
/// 手写一个 1x1 的 RGB TIFF：不依赖 AppKit 编码器，测的是**解码**路径。
///
/// 布局：8 字节头 → IFD（10 个标签）→ BitsPerSample 数组 → 像素。
fn tiny_tiff() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&8u32.to_le_bytes());
    let entries: u16 = 10;
    out.extend_from_slice(&entries.to_le_bytes());
    let tag = |out: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32| {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    };
    // 134 起是 3 个 BitsPerSample（8,8,8），140 起是 3 个像素字节。
    tag(&mut out, 0x0100, 4, 1, 1); // ImageWidth
    tag(&mut out, 0x0101, 4, 1, 1); // ImageLength
    tag(&mut out, 0x0102, 3, 3, 134); // BitsPerSample
    tag(&mut out, 0x0103, 3, 1, 1); // Compression = none
    tag(&mut out, 0x0106, 3, 1, 2); // Photometric = RGB
    tag(&mut out, 0x0111, 4, 1, 140); // StripOffsets
    tag(&mut out, 0x0115, 3, 1, 3); // SamplesPerPixel
    tag(&mut out, 0x0116, 4, 1, 1); // RowsPerStrip
    tag(&mut out, 0x0117, 4, 1, 3); // StripByteCounts
    tag(&mut out, 0x011C, 3, 1, 1); // PlanarConfiguration
    out.extend_from_slice(&0u32.to_le_bytes()); // next IFD
    assert_eq!(out.len(), 134);
    out.extend_from_slice(&[8, 0, 8, 0, 8, 0]); // BitsPerSample 数组
    out.extend_from_slice(&[0x11, 0x22, 0x33]); // 一个像素
    out
}

#[cfg(all(test, target_os = "macos"))]
mod pasteboard_tests {
    use super::*;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF,
    };
    use objc2_foundation::{NSData, NSString};

    /// 独立 pasteboard：测试不能清用户的真实剪贴板。
    fn unique_pasteboard() -> Retained<NSPasteboard> {
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        pasteboard.clearContents();
        pasteboard
    }

    fn png_header() -> Vec<u8> {
        vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]
    }

    /// 回归：复制图片时 pasteboard 往往同时带文本（浏览器、Keynote）。
    /// 图片必须优先，不能把 URL / 文件名当文本粘进终端。
    #[test]
    fn an_image_wins_over_text_on_the_same_pasteboard() {
        let pasteboard = unique_pasteboard();
        let png = png_header();
        assert!(
            pasteboard.setData_forType(Some(&NSData::with_bytes(&png)), unsafe {
                NSPasteboardTypePNG
            })
        );
        assert!(pasteboard
            .setString_forType(&NSString::from_str("https://example.com/a.png"), unsafe {
                NSPasteboardTypeString
            }));

        let read = read_image_png_from(pasteboard).expect("pasteboard 里有图");
        assert_eq!(read.expect("PNG 必须原样透传"), png);
    }

    /// TIFF（截图 / Preview）与文本共存时也要选图，并转成 PNG。
    #[test]
    fn tiff_with_text_becomes_png() {
        let pasteboard = unique_pasteboard();
        assert!(
            pasteboard.setData_forType(Some(&NSData::with_bytes(&tiny_tiff())), unsafe {
                NSPasteboardTypeTIFF
            })
        );
        assert!(
            pasteboard.setString_forType(&NSString::from_str("screenshot.png"), unsafe {
                NSPasteboardTypeString
            })
        );

        let png = read_image_png_from(pasteboard)
            .expect("pasteboard 里有图")
            .expect("TIFF 必须能转 PNG");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    /// 只有文本时不能把任意文本当成图片。
    #[test]
    fn text_only_has_no_image() {
        let pasteboard = unique_pasteboard();
        assert!(
            pasteboard.setString_forType(&NSString::from_str("just text"), unsafe {
                NSPasteboardTypeString
            })
        );
        assert!(read_image_png_from(pasteboard).is_none());
    }

    /// 空的 PNG 数据要给出错误，而不是发一个 0 字节的图给 Core。
    #[test]
    fn an_empty_image_reports_an_error() {
        let pasteboard = unique_pasteboard();
        assert!(
            pasteboard.setData_forType(Some(&NSData::with_bytes(&[])), unsafe {
                NSPasteboardTypePNG
            })
        );
        let read = read_image_png_from(pasteboard).expect("pasteboard 里有 PNG 类型");
        assert!(read.is_err(), "空图必须报错");
    }
}
