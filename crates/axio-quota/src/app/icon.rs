//! Drawing a number into the tray icon.
//!
//! Tauri takes an RGBA buffer and has no opinion about how it was drawn, but it cannot
//! draw one — `Shell_NotifyIconW` wants a bitmap, and no framework changes that. A macOS
//! status item renders "23%" as text; on Windows the percentage has to be rasterised on
//! every refresh, so this stays a real component rather than a resource file.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, CreateFontW,
    DIB_RGB_COLORS, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW,
    FW_BOLD, GetDC, HGDIOBJ, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};

/// How alarming the number is. Colours come from axio's own palette so the tray, the
/// flyout and the site agree about what amber means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Normal,
    Warning,
    Critical,
    Unknown,
}

impl Severity {
    /// From percent *used*. The bands match what a person acts on: under 75 is "fine",
    /// 75-90 is "plan the rest of the day", over 90 is "this will run out".
    pub fn from_used_percent(used: f64) -> Severity {
        match used {
            u if u >= 90.0 => Severity::Critical,
            u if u >= 75.0 => Severity::Warning,
            _ => Severity::Normal,
        }
    }

    /// BGR, as GDI wants it.
    fn colour(self) -> u32 {
        match self {
            // The notification area is dark on most installs but not all, so the normal
            // state is near-white (--fg) rather than a themed colour that vanishes on one.
            Severity::Normal => 0x00FA_FAFA,
            Severity::Warning => 0x0024_BFFB,  // --warn #fbbf24
            Severity::Critical => 0x0044_44EF, // red
            Severity::Unknown => 0x008C_8C8C,  // --muted #8c8c8c
        }
    }
}

/// A square RGBA image, as Tauri's `Image::new` wants it.
pub struct Rgba {
    pub size: u32,
    pub pixels: Vec<u8>,
}

/// Renders up to two characters — "7", "92", "!!" — centred at the given square size.
pub fn render(text: &str, size: u32, severity: Severity) -> Option<Rgba> {
    unsafe { render_inner(text, size.clamp(16, 64) as i32, severity) }
}

unsafe fn render_inner(text: &str, size: i32, severity: Severity) -> Option<Rgba> {
    let screen_dc = unsafe { GetDC(std::ptr::null_mut::<c_void>() as HWND) };
    if screen_dc.is_null() {
        return None;
    }
    let dc = unsafe { CreateCompatibleDC(screen_dc) };
    unsafe { ReleaseDC(std::ptr::null_mut::<c_void>() as HWND, screen_dc) };
    if dc.is_null() {
        return None;
    }

    // Top-down 32bpp so pixel 0 is the top-left and the alpha byte is addressable.
    let mut header: BITMAPINFO = unsafe { std::mem::zeroed() };
    header.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: size,
        biHeight: -size,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..unsafe { std::mem::zeroed() }
    };

    let mut bits: *mut c_void = std::ptr::null_mut();
    let bitmap = unsafe {
        CreateDIBSection(
            dc,
            &header,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        )
    };
    if bitmap.is_null() || bits.is_null() {
        unsafe { DeleteDC(dc) };
        return None;
    }

    let previous = unsafe { SelectObject(dc, bitmap as HGDIOBJ) };

    // GDI text drawing does not write the alpha channel, so the glyph is drawn white on a
    // zeroed buffer and its coverage is recovered afterwards from the colour channels.
    let font = unsafe {
        CreateFontW(
            -(size * 11 / 16),
            0,
            0,
            0,
            FW_BOLD as i32,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            wide("Segoe UI").as_ptr(),
        )
    };
    let previous_font = unsafe { SelectObject(dc, font as HGDIOBJ) };
    unsafe {
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00FF_FFFF);
    }

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: size,
        bottom: size,
    };
    let mut wide_text = wide(text);
    unsafe {
        DrawTextW(
            dc,
            wide_text.as_mut_ptr(),
            -1,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        )
    };

    let pixels = to_rgba(bits.cast::<u8>(), (size * size) as usize, severity.colour());

    unsafe {
        SelectObject(dc, previous_font);
        DeleteObject(font as HGDIOBJ);
        SelectObject(dc, previous);
        DeleteObject(bitmap as HGDIOBJ);
        DeleteDC(dc);
    }

    Some(Rgba {
        size: size as u32,
        pixels,
    })
}

/// Turns white-on-black BGRA from GDI into the straight RGBA Tauri expects.
///
/// Coverage comes from the brightest channel so an anti-aliased edge keeps its softness
/// rather than becoming a hard 1-bit cutout. Straight alpha, not premultiplied:
/// `Image::new` expects the former, and premultiplying would darken every edge pixel.
fn to_rgba(pixels: *const u8, count: usize, colour: u32) -> Vec<u8> {
    let (blue, green, red) = (
        (colour & 0xFF) as u8,
        ((colour >> 8) & 0xFF) as u8,
        ((colour >> 16) & 0xFF) as u8,
    );
    let mut out = Vec::with_capacity(count * 4);
    for index in 0..count {
        // SAFETY: `pixels` points at `count` BGRA quads from CreateDIBSection.
        let pixel = unsafe { pixels.add(index * 4) };
        let (b, g, r) = unsafe { (*pixel, *pixel.add(1), *pixel.add(2)) };
        let coverage = b.max(g).max(r);
        out.extend_from_slice(&[red, green, blue, coverage]);
    }
    out
}

pub(crate) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_bands_match_what_a_person_acts_on() {
        assert_eq!(Severity::from_used_percent(10.0), Severity::Normal);
        assert_eq!(Severity::from_used_percent(74.9), Severity::Normal);
        assert_eq!(Severity::from_used_percent(75.0), Severity::Warning);
        assert_eq!(Severity::from_used_percent(89.9), Severity::Warning);
        assert_eq!(Severity::from_used_percent(90.0), Severity::Critical);
    }

    #[test]
    fn rendering_produces_a_square_rgba_buffer_with_visible_pixels() {
        let image = render("92", 16, Severity::Critical).expect("renders");
        assert_eq!(image.size, 16);
        assert_eq!(image.pixels.len(), 16 * 16 * 4);
        // A glyph was actually drawn: some pixel is not fully transparent.
        assert!(image.pixels.chunks(4).any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn an_empty_label_still_yields_a_buffer() {
        // The tray must have an icon to hand the shell before the first probe returns.
        let image = render("", 16, Severity::Unknown).expect("renders");
        assert!(image.pixels.chunks(4).all(|pixel| pixel[3] == 0));
    }
}
