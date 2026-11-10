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

#[cfg(target_os = "macos")]
fn convert(_format: ImageFormat, bytes: &[u8]) -> Option<Vec<u8>> {
    use objc2::{rc::autoreleasepool, AnyThread};
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
    use objc2_foundation::{NSData, NSDictionary};

    if bytes.is_empty() {
        return None;
    }
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
