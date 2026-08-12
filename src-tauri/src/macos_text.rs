#![cfg(target_os = "macos")]

use std::ffi::{c_char, c_void, CString};
use std::ptr;

type AXUIElementRef = *const c_void;
type AXValueRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFIndex = isize;
type CFStringEncoding = u32;
type CFStringRef = *const c_void;
type CFTypeID = usize;
type CFTypeRef = *const c_void;
type Boolean = u8;
type pid_t = i32;
type AXError = i32;
type AXValueType = u32;

const K_CF_STRING_ENCODING_UTF8: CFStringEncoding = 0x0800_0100;
const K_AX_VALUE_CF_RANGE_TYPE: AXValueType = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CFRange {
    location: CFIndex,
    length: CFIndex,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: pid_t) -> AXUIElementRef;
    fn AXUIElementGetTypeID() -> CFTypeID;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementIsAttributeSettable(
        element: AXUIElementRef,
        attribute: CFStringRef,
        settable: *mut Boolean,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXValueCreate(the_type: AXValueType, value_ptr: *const c_void) -> AXValueRef;
    fn AXValueGetTypeID() -> CFTypeID;
    fn AXValueGetValue(value: AXValueRef, the_type: AXValueType, value_ptr: *mut c_void)
        -> Boolean;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    fn CFRelease(cf: CFTypeRef);
    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: CFStringEncoding,
    ) -> CFStringRef;
    fn CFStringGetTypeID() -> CFTypeID;
    fn CFStringGetLength(the_string: CFStringRef) -> CFIndex;
    fn CFStringGetMaximumSizeForEncoding(length: CFIndex, encoding: CFStringEncoding) -> CFIndex;
    fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: CFStringEncoding,
    ) -> Boolean;
}

struct CfType(CFTypeRef);

impl CfType {
    fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for CfType {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

pub fn insert_text_into_focused_element(pid: i32, text: &str) -> Result<(), String> {
    let app = unsafe { AXUIElementCreateApplication(pid as pid_t) };
    if app.is_null() {
        return Err("Could not create accessibility handle for target app.".into());
    }

    let focused = copy_attribute(app, "AXFocusedUIElement")?;
    let element = focused.as_ptr();
    let element_type = unsafe { CFGetTypeID(element) };
    if element_type != unsafe { AXUIElementGetTypeID() } {
        return Err("Focused UI element is not an accessibility text element.".into());
    }

    ensure_settable(element, "AXValue")?;

    let current_value = copy_optional_string_attribute(element, "AXValue")?.unwrap_or_default();
    let selected_range = copy_selected_range(element).ok();
    let insert_range = resolve_insert_range(&current_value, selected_range)?;
    let updated_value = splice_text(&current_value, insert_range, text)?;

    let updated_cf = cfstring(&updated_value)?;
    let set_err = unsafe { AXUIElementSetAttributeValue(element, cfstring_attr("AXValue")?.as_ptr(), updated_cf.as_ptr()) };
    if set_err != 0 {
        return Err(format!(
            "Focused field rejected AX text insertion (AXValue set error {}).",
            set_err
        ));
    }

    let new_cursor = CFRange {
        location: insert_range.location + text.chars().count() as isize,
        length: 0,
    };
    if ensure_settable(element, "AXSelectedTextRange").is_ok() {
        let range_value = unsafe { AXValueCreate(K_AX_VALUE_CF_RANGE_TYPE, &new_cursor as *const _ as *const c_void) };
        if !range_value.is_null() {
            let range_value = CfType(range_value);
            let _ = unsafe {
                AXUIElementSetAttributeValue(
                    element,
                    cfstring_attr("AXSelectedTextRange")?.as_ptr(),
                    range_value.as_ptr(),
                )
            };
        }
    }

    Ok(())
}

fn resolve_insert_range(current_value: &str, selected_range: Option<CFRange>) -> Result<CFRange, String> {
    if let Some(range) = selected_range {
        if range.location < 0 || range.length < 0 {
            return Err("Focused field returned an invalid selection range.".into());
        }
        let char_len = current_value.chars().count() as isize;
        if range.location > char_len || range.location + range.length > char_len {
            return Err("Focused field selection range was outside the current value.".into());
        }
        return Ok(range);
    }

    if current_value.is_empty() {
        return Ok(CFRange { location: 0, length: 0 });
    }

    Err("Focused field does not expose a writable selection range for direct AX insertion.".into())
}

fn splice_text(current_value: &str, range: CFRange, inserted: &str) -> Result<String, String> {
    let start = char_to_byte_index(current_value, range.location as usize)?;
    let end = char_to_byte_index(current_value, (range.location + range.length) as usize)?;
    let mut out = String::with_capacity(current_value.len() + inserted.len());
    out.push_str(&current_value[..start]);
    out.push_str(inserted);
    out.push_str(&current_value[end..]);
    Ok(out)
}

fn char_to_byte_index(text: &str, char_index: usize) -> Result<usize, String> {
    if char_index == text.chars().count() {
        return Ok(text.len());
    }
    text.char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .ok_or_else(|| "Could not map accessibility character range to string index.".into())
}

fn ensure_settable(element: AXUIElementRef, attr: &str) -> Result<(), String> {
    let attr = cfstring_attr(attr)?;
    let mut settable: Boolean = 0;
    let err = unsafe { AXUIElementIsAttributeSettable(element, attr.as_ptr(), &mut settable) };
    if err != 0 {
        return Err(format!(
            "Focused field does not support writable {} accessibility attribute (error {}).",
            attr_name(attr.as_ptr()),
            err
        ));
    }
    if settable == 0 {
        return Err(format!(
            "Focused field exposed {} but it is not writable.",
            attr_name(attr.as_ptr())
        ));
    }
    Ok(())
}

fn copy_selected_range(element: AXUIElementRef) -> Result<CFRange, String> {
    let raw = copy_attribute(element, "AXSelectedTextRange")?;
    if unsafe { CFGetTypeID(raw.as_ptr()) } != unsafe { AXValueGetTypeID() } {
        return Err("Focused field returned a non-range AXSelectedTextRange value.".into());
    }

    let mut range = CFRange { location: 0, length: 0 };
    let ok = unsafe {
        AXValueGetValue(
            raw.as_ptr() as AXValueRef,
            K_AX_VALUE_CF_RANGE_TYPE,
            &mut range as *mut _ as *mut c_void,
        )
    };
    if ok == 0 {
        return Err("Could not decode AXSelectedTextRange.".into());
    }
    Ok(range)
}

fn copy_optional_string_attribute(element: AXUIElementRef, attr: &str) -> Result<Option<String>, String> {
    let attr_cf = cfstring_attr(attr)?;
    let mut value: CFTypeRef = ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr_cf.as_ptr(), &mut value) };
    if err != 0 {
        // kAXErrorNoValue / unsupported fall back to None here.
        return Ok(None);
    }
    let value = CfType(value);
    if value.as_ptr().is_null() {
        return Ok(None);
    }
    if unsafe { CFGetTypeID(value.as_ptr()) } != unsafe { CFStringGetTypeID() } {
        return Ok(None);
    }
    Ok(Some(cfstring_to_string(value.as_ptr() as CFStringRef)?))
}

fn copy_attribute(element: AXUIElementRef, attr: &str) -> Result<CfType, String> {
    let attr_cf = cfstring_attr(attr)?;
    let mut value: CFTypeRef = ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr_cf.as_ptr(), &mut value) };
    if err != 0 || value.is_null() {
        return Err(format!(
            "Accessibility attribute {} was unavailable (error {}).",
            attr,
            err
        ));
    }
    Ok(CfType(value))
}

fn cfstring(value: &str) -> Result<CfType, String> {
    let cstr = CString::new(value).map_err(|_| "Text contained an unexpected NUL byte.".to_string())?;
    let raw = unsafe { CFStringCreateWithCString(ptr::null(), cstr.as_ptr(), K_CF_STRING_ENCODING_UTF8) };
    if raw.is_null() {
        return Err("Could not allocate CoreFoundation string.".into());
    }
    Ok(CfType(raw))
}

fn cfstring_attr(value: &str) -> Result<CfType, String> {
    cfstring(value)
}

fn cfstring_to_string(value: CFStringRef) -> Result<String, String> {
    if value.is_null() {
        return Err("CoreFoundation string was null.".into());
    }
    let len = unsafe { CFStringGetLength(value) };
    let max = unsafe { CFStringGetMaximumSizeForEncoding(len, K_CF_STRING_ENCODING_UTF8) } + 1;
    let mut buf = vec![0u8; max as usize];
    let ok = unsafe {
        CFStringGetCString(
            value,
            buf.as_mut_ptr() as *mut c_char,
            max,
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    if ok == 0 {
        return Err("Could not decode CoreFoundation string as UTF-8.".into());
    }
    let nul = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..nul].to_vec()).map_err(|_| "Focused field value was not valid UTF-8.".into())
}

fn attr_name(_attr: CFTypeRef) -> &'static str {
    "attribute"
}
