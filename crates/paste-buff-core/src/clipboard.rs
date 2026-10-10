//! Thin wrapper over `NSPasteboard.generalPasteboard` for text, file and
//! image clips.

use crate::history::{Clip, Content, Entry};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSPasteboard,
    NSPasteboardType, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeString,
    NSPasteboardTypeTIFF, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString, NSURL};
use std::path::PathBuf;

/// Pasteboard types that password managers and the like use to mark
/// content that history tools should not record.
/// See <http://nspasteboard.org>.
pub const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";
pub const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";

/// Monotonic counter that increments on every pasteboard write.
pub fn change_count() -> i64 {
    NSPasteboard::generalPasteboard().changeCount() as i64
}

/// Current content as a [`Clip`], or `None` when there is nothing we
/// record. Precedence: file URLs, then non-blank text, then an image. Text
/// beats image on purpose: spreadsheets and rich-text editors put a
/// rendered picture alongside the text, and the text is what you wanted.
pub fn read() -> Option<Clip> {
    let pb = NSPasteboard::generalPasteboard();
    if let Some(types) = pb.types() {
        for ty in types.iter() {
            let ty = ty.to_string();
            if ty == CONCEALED_TYPE || ty == TRANSIENT_TYPE {
                return None;
            }
        }
    }

    let files = file_paths(&pb);
    if !files.is_empty() {
        return Some(Clip::Files(files));
    }

    if let Some(text) = pb.stringForType(string_type()) {
        let text = text.to_string();
        if !text.trim().is_empty() {
            return Some(Clip::Text(text));
        }
    }

    image(&pb)
}

/// Replace the pasteboard contents with `entry`'s clip.
pub fn write(entry: &Entry) -> bool {
    match &entry.content {
        Content::Text => write_string(&entry.text),
        Content::Files { paths } => {
            let urls: Vec<Retained<NSURL>> = paths
                .iter()
                .map(|p| NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy())))
                .collect();
            let objects: Vec<&ProtocolObject<dyn NSPasteboardWriting>> = urls
                .iter()
                .map(|u| ProtocolObject::from_ref(&**u))
                .collect();
            let pb = NSPasteboard::generalPasteboard();
            pb.clearContents();
            pb.writeObjects(&NSArray::from_slice(&objects))
        }
        Content::Image { file, .. } => {
            let Ok(png) = std::fs::read(file) else {
                eprintln!("paste-buff: image {} is gone", file.display());
                return false;
            };
            let data = NSData::with_bytes(&png);
            let pb = NSPasteboard::generalPasteboard();
            pb.clearContents();
            let mut ok = pb.setData_forType(Some(&data), unsafe { NSPasteboardTypePNG });
            // Older apps only take TIFF.
            if let Some(tiff) = NSBitmapImageRep::imageRepWithData(&data)
                .and_then(|rep| encode(&rep, NSBitmapImageFileType::TIFF))
            {
                ok |= pb.setData_forType(Some(&tiff), unsafe { NSPasteboardTypeTIFF });
            }
            ok
        }
    }
}

/// Replace the pasteboard contents with plain text.
pub fn write_string(text: &str) -> bool {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    pb.setString_forType(&NSString::from_str(text), string_type())
}

fn string_type() -> &'static NSPasteboardType {
    unsafe { NSPasteboardTypeString }
}

fn file_paths(pb: &NSPasteboard) -> Vec<PathBuf> {
    let Some(items) = pb.pasteboardItems() else {
        return Vec::new();
    };
    let file_url_type = unsafe { NSPasteboardTypeFileURL };
    items
        .iter()
        .filter_map(|item| item.stringForType(file_url_type))
        .filter_map(|s| NSURL::URLWithString(&s))
        .filter_map(|url| url.path())
        .map(|p| PathBuf::from(p.to_string()))
        .collect()
}

fn image(pb: &NSPasteboard) -> Option<Clip> {
    let png_type = unsafe { NSPasteboardTypePNG };
    let tiff_type = unsafe { NSPasteboardTypeTIFF };
    let (data, is_png) = match pb.dataForType(png_type) {
        Some(d) => (d, true),
        None => (pb.dataForType(tiff_type)?, false),
    };
    let rep = NSBitmapImageRep::imageRepWithData(&data)?;
    let width = rep.pixelsWide().max(0) as u32;
    let height = rep.pixelsHigh().max(0) as u32;
    if width == 0 || height == 0 {
        return None;
    }
    let png = if is_png {
        data.to_vec()
    } else {
        encode(&rep, NSBitmapImageFileType::PNG)?.to_vec()
    };
    Some(Clip::Image { png, width, height })
}

fn encode(rep: &NSBitmapImageRep, kind: NSBitmapImageFileType) -> Option<Retained<NSData>> {
    let props: Retained<NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>> = NSDictionary::new();
    unsafe { rep.representationUsingType_properties(kind, &props) }
}

#[cfg(test)]
mod tests {
    //! These replace the real pasteboard contents, so they only run with
    //! `cargo test -p paste-buff-core -- --ignored`.
    use super::*;
    use crate::history::{History, DEFAULT_TTL};

    fn entry_for(clip: Clip) -> Entry {
        let mut h = History::in_memory(DEFAULT_TTL);
        h.push_clip(clip, None).unwrap();
        h.entries()[0].clone()
    }

    #[test]
    #[ignore]
    fn text_roundtrip() {
        assert!(write(&entry_for(Clip::Text("paste-buff text".into()))));
        assert_eq!(read(), Some(Clip::Text("paste-buff text".into())));
    }

    #[test]
    #[ignore]
    fn files_roundtrip() {
        let paths = vec![
            PathBuf::from("/Applications/Safari.app"),
            PathBuf::from("/System/Library/CoreServices/Finder.app"),
        ];
        assert!(write(&entry_for(Clip::Files(paths.clone()))));
        assert_eq!(read(), Some(Clip::Files(paths)));
    }

    #[test]
    #[ignore]
    fn image_roundtrip() {
        // 2×2 red PNG.
        let png: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00,
            0x00, 0xFD, 0xD4, 0x9A, 0x73, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0xC0, 0xC0, 0xF0, 0x1F, 0x88, 0x19, 0x18, 0x00, 0x1D,
            0x89, 0x03, 0xFD, 0xBA, 0xBD, 0x5B, 0xC2, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
            0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let entry = entry_for(Clip::Image { png: png.clone(), width: 2, height: 2 });
        assert!(write(&entry));
        match read() {
            Some(Clip::Image { png: got, width, height }) => {
                assert_eq!((width, height), (2, 2));
                assert_eq!(got, png, "PNG bytes come back verbatim");
            }
            other => panic!("expected image, got {other:?}"),
        }
        let pb = NSPasteboard::generalPasteboard();
        assert!(pb.dataForType(unsafe { NSPasteboardTypeTIFF }).is_some(), "TIFF offered too");
    }
}
