use std::future::Future;

use anyhow::{Result, anyhow};
use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobPropertyBag, Clipboard as BrowserClipboard, ClipboardItem as BrowserClipboardItem,
    Window,
};

use crate::{
    ClipboardEntry, ClipboardItem, ClipboardString, Image, ImageFormat, RawClipboardEntry,
    RawClipboardItem,
};

enum StartedRawRead {
    Raw(JsFuture),
    Text(JsFuture),
}

enum StartedWrite {
    Text(JsFuture),
    Raw(JsFuture),
}

pub fn read_raw(
    window: Window,
) -> Result<impl Future<Output = Result<Option<RawClipboardItem>>> + 'static> {
    let clipboard = browser_clipboard(&window)?;
    let read = if has_function(clipboard.as_ref(), "read") {
        StartedRawRead::Raw(JsFuture::from(clipboard.read()))
    } else {
        StartedRawRead::Text(start_read_text_with_clipboard(&clipboard)?)
    };

    Ok(async move { finish_read_raw(read).await })
}

pub fn read(
    window: Window,
) -> Result<impl Future<Output = Result<Option<ClipboardItem>>> + 'static> {
    let raw = read_raw(window)?;

    Ok(async move {
        let raw = raw.await?;
        Ok(raw.as_ref().and_then(raw_to_clipboard_item))
    })
}

pub fn write(
    window: Window,
    item: ClipboardItem,
) -> Result<impl Future<Output = Result<()>> + 'static> {
    let clipboard = browser_clipboard(&window)?;
    let write = start_write_with_clipboard(&clipboard, item)?;

    Ok(async move { finish_write(write).await })
}

async fn finish_read_raw(read: StartedRawRead) -> Result<Option<RawClipboardItem>> {
    match read {
        StartedRawRead::Raw(read) => finish_read_raw_items(read).await,
        StartedRawRead::Text(read_text) => finish_read_text_raw(read_text).await,
    }
}

async fn finish_read_raw_items(read: JsFuture) -> Result<Option<RawClipboardItem>> {
    let items = read
        .await
        .map(|items| Array::from(&items))
        .map_err(|error| js_error("navigator.clipboard.read failed", error))?;

    let mut entries = Vec::new();

    for item in items.iter() {
        let item = item
            .dyn_into::<BrowserClipboardItem>()
            .map_err(|error| js_error("clipboard.read returned a non-ClipboardItem", error))?;

        for mime_type in item.types().iter().filter_map(|value| value.as_string()) {
            let blob = JsFuture::from(item.get_type(&mime_type))
                .await
                .map_err(|error| {
                    js_error(
                        &format!("ClipboardItem.getType({mime_type:?}) failed"),
                        error,
                    )
                })?
                .dyn_into::<Blob>()
                .map_err(|error| {
                    js_error(
                        &format!("ClipboardItem.getType({mime_type:?}) did not return a Blob"),
                        error,
                    )
                })?;
            let bytes = blob_bytes(&blob).await?;

            entries.push(RawClipboardEntry {
                format: mime_type,
                bytes,
            });
        }
    }

    if !entries.is_empty() {
        log::debug!(
            "web clipboard read MIME types: {}",
            entries
                .iter()
                .map(|entry| entry.format.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(non_empty_raw(entries))
}

fn start_write_with_clipboard(
    clipboard: &BrowserClipboard,
    item: ClipboardItem,
) -> Result<StartedWrite> {
    if let Some(text) = text_only_item(&item) {
        return Ok(StartedWrite::Text(start_write_text_with_clipboard(
            clipboard, &text,
        )?));
    }

    if !has_function(clipboard.as_ref(), "write") {
        return Err(anyhow!(
            "navigator.clipboard.write is not available in this browser context"
        ));
    }

    let raw = RawClipboardItem::from_clipboard_item(&item);
    let record = Object::new();
    let mut has_entry = false;

    for entry in raw.entries {
        if entry.format.is_empty() {
            continue;
        }

        let blob = blob_from_bytes(&entry.format, &entry.bytes)?;
        Reflect::set(
            record.as_ref(),
            &JsValue::from_str(&entry.format),
            blob.as_ref(),
        )
        .map_err(|error| js_error("failed to build ClipboardItem MIME record", error))?;
        has_entry = true;
    }

    if !has_entry {
        return Ok(StartedWrite::Text(start_write_text_with_clipboard(
            clipboard, "",
        )?));
    }

    let browser_item = BrowserClipboardItem::new_with_record_from_str_to_blob_promise(&record)
        .map_err(|error| js_error("failed to construct ClipboardItem", error))?;
    let items = Array::new();
    items.push(browser_item.as_ref());

    Ok(StartedWrite::Raw(JsFuture::from(
        clipboard.write(items.as_ref()),
    )))
}

async fn finish_write(write: StartedWrite) -> Result<()> {
    match write {
        StartedWrite::Text(write_text) => write_text
            .await
            .map(|_| ())
            .map_err(|error| js_error("navigator.clipboard.writeText failed", error)),
        StartedWrite::Raw(write) => write
            .await
            .map(|_| ())
            .map_err(|error| js_error("navigator.clipboard.write failed", error)),
    }
}

fn browser_clipboard(window: &Window) -> Result<BrowserClipboard> {
    let clipboard = window.navigator().clipboard();
    let clipboard_value: &JsValue = clipboard.as_ref();

    if clipboard_value.is_null() || clipboard_value.is_undefined() {
        return Err(anyhow!(
            "navigator.clipboard is not available; clipboard access requires a supported browser and secure context"
        ));
    }

    Ok(clipboard)
}

async fn finish_read_text_raw(read_text: JsFuture) -> Result<Option<RawClipboardItem>> {
    let text = finish_read_text(read_text).await?;
    if text.is_empty() {
        return Ok(None);
    }

    Ok(Some(RawClipboardItem {
        entries: vec![RawClipboardEntry {
            format: "text/plain".to_string(),
            bytes: text.into_bytes(),
        }],
    }))
}

fn start_read_text_with_clipboard(clipboard: &BrowserClipboard) -> Result<JsFuture> {
    if !has_function(clipboard.as_ref(), "readText") {
        return Err(anyhow!(
            "navigator.clipboard.readText is not available in this browser context"
        ));
    }

    Ok(JsFuture::from(clipboard.read_text()))
}

async fn finish_read_text(read_text: JsFuture) -> Result<String> {
    read_text
        .await
        .map_err(|error| js_error("navigator.clipboard.readText failed", error))?
        .as_string()
        .ok_or_else(|| anyhow!("navigator.clipboard.readText did not return a string"))
}

fn start_write_text_with_clipboard(clipboard: &BrowserClipboard, text: &str) -> Result<JsFuture> {
    if !has_function(clipboard.as_ref(), "writeText") {
        return Err(anyhow!(
            "navigator.clipboard.writeText is not available in this browser context"
        ));
    }

    Ok(JsFuture::from(clipboard.write_text(text)))
}

async fn blob_bytes(blob: &Blob) -> Result<Vec<u8>> {
    let buffer = JsFuture::from(blob.array_buffer())
        .await
        .map_err(|error| js_error("Blob.arrayBuffer failed", error))?;
    Ok(Uint8Array::new(&buffer).to_vec())
}

fn blob_from_bytes(mime_type: &str, bytes: &[u8]) -> Result<Blob> {
    let parts = Array::new();
    parts.push(&Uint8Array::from(bytes));

    let options = BlobPropertyBag::new();
    options.set_type(mime_type);

    Blob::new_with_u8_array_sequence_and_options(parts.as_ref(), &options)
        .map_err(|error| js_error("failed to construct Blob", error))
}

fn raw_to_clipboard_item(raw: &RawClipboardItem) -> Option<ClipboardItem> {
    let mut entries = Vec::new();

    for entry in &raw.entries {
        if entry.format == "text/plain" {
            if let Ok(text) = String::from_utf8(entry.bytes.clone()) {
                entries.push(ClipboardEntry::String(ClipboardString::new(text)));
            }
        } else if let Some(format) = ImageFormat::from_mime_type(&entry.format) {
            entries.push(ClipboardEntry::Image(Image::from_bytes(
                format,
                entry.bytes.clone(),
            )));
        }
    }

    if entries.is_empty() {
        None
    } else {
        Some(ClipboardItem { entries })
    }
}

fn text_only_item(item: &ClipboardItem) -> Option<String> {
    if item.entries().iter().all(|entry| {
        matches!(
            entry,
            ClipboardEntry::String(_) | ClipboardEntry::ExternalPaths(_)
        )
    }) {
        Some(item.text().unwrap_or_default())
    } else {
        None
    }
}

fn non_empty_raw(entries: Vec<RawClipboardEntry>) -> Option<RawClipboardItem> {
    if entries.is_empty() {
        None
    } else {
        Some(RawClipboardItem { entries })
    }
}

fn has_function(value: &JsValue, name: &str) -> bool {
    Reflect::get(value, &JsValue::from_str(name))
        .ok()
        .is_some_and(|value| value.is_function())
}

fn js_error(action: &str, error: JsValue) -> anyhow::Error {
    let message = if let Some(message) = error.as_string() {
        message
    } else if let Ok(message) = Reflect::get(&error, &JsValue::from_str("message")) {
        message.as_string().unwrap_or_else(|| format!("{error:?}"))
    } else {
        format!("{error:?}")
    };

    anyhow!("{action}: {message}")
}
